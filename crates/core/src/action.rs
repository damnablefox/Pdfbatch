//! 对单个文档执行的动作，以及把它应用到一批文件的执行器。
//!
//! 命令行与图形界面都走这里：一处实现，两处使用，行为不会漂移。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use pdfcraft_cos::Document;

use crate::batch::{self, Input, Job, OutputMode};
use crate::error::{BatchError, Result};
use crate::fill::{FillChange, FillStyle, Matcher};
use crate::pagesize::{self, ScaleSpec};
use crate::scan::{self, ScanOutcome, ScanSpec};
use crate::{bookmarks, fill, pages, pdf, spec};

/// 书签文本的来源：所有文件共用一份，还是按文件分段。
#[derive(Debug, Clone)]
pub enum BookmarkSpec {
    /// 所有文件用同一份文本。
    Shared(String),
    /// 每个文件用它自己那段（按输入路径）。
    PerFile(Vec<(PathBuf, String)>),
}

/// 填充区域上的三种操作：增、删、改。
#[derive(Debug, Clone)]
pub enum FillOp {
    /// 新增填充层；`replace` 为真时先摘掉本工具此前加在该页的层再重画。
    Add {
        style: FillStyle,
        replace: bool,
    },
    /// 按条件删除命中的填充层；条件为空表示「所有本工具加的层」。
    Remove(Matcher),
    /// 按条件修改命中的填充层；`change` 里为 `None` 的维度保持原值。
    Modify {
        matcher: Matcher,
        change: FillChange,
    },
}

impl FillOp {
    /// 操作的中文名，用于摘要与界面提示。
    pub fn label(&self) -> &'static str {
        match self {
            FillOp::Add { .. } => "新增",
            FillOp::Remove(_) => "删除",
            FillOp::Modify { .. } => "修改",
        }
    }
}

/// 「页面尺寸」的两种做法。
#[derive(Debug, Clone)]
pub enum PageSizeMethod {
    /// 按目标尺寸缩放内容。
    Scale(ScaleSpec),
    /// 按扫描件图像适配（页面贴合图像）。
    Scan(ScanSpec),
}

impl PageSizeMethod {
    /// 中文名，用于摘要。
    pub fn label(&self) -> &'static str {
        match self {
            PageSizeMethod::Scale(_) => "缩放",
            PageSizeMethod::Scan(_) => "扫描件适配",
        }
    }
}

/// 一个文档上要做的动作。
#[derive(Debug, Clone)]
pub enum Action {
    /// 按文本规格写书签。
    Bookmarks {
        /// 书签文本来源（共用一份，或按文件分段）。
        spec: BookmarkSpec,
        zero_based: bool,
        strict: bool,
        replace: bool,
    },
    /// 在指定页增/删/改纯色填充层。
    Fill {
        op: FillOp,
        /// 页范围原文（`1-5,8,10-` 或 `all`）。
        pages: String,
    },
    /// 调整指定页的尺寸：按目标尺寸缩放，或按扫描件图像适配。
    PageSize {
        method: PageSizeMethod,
        /// 页范围原文（`1-5,8,10-` 或 `all`）。
        pages: String,
    },
}

/// 一页的处理结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordResult {
    Done,
    Skipped,
    Failed,
}

impl RecordResult {
    pub fn label(self) -> &'static str {
        match self {
            RecordResult::Done => "成功",
            RecordResult::Skipped => "跳过",
            RecordResult::Failed => "失败",
        }
    }
}

/// 一页的处理记录（目前由「页面尺寸」动作填写，供处理日志使用）。
#[derive(Debug, Clone)]
pub struct PageRecord {
    /// 页码（从 1 开始）。
    pub page: usize,
    pub result: RecordResult,
    /// 原因或摘要。
    pub reason: String,
    /// 处理后的页面尺寸（点），没有就是 `None`。
    pub size: Option<(f64, f64)>,
}

/// 一次动作的结果说明：一句摘要，加上不致命的警告与逐页记录。
#[derive(Debug, Clone, Default)]
pub struct RunNote {
    pub summary: String,
    pub warnings: Vec<String>,
    /// 逐页记录；不需要细分的动作留空。
    pub records: Vec<PageRecord>,
}

impl Action {
    /// 在已打开的文档上执行。
    pub fn run(&self, doc: &mut Document, path: &Path) -> Result<RunNote> {
        match self {
            Action::Bookmarks {
                spec: source,
                zero_based,
                strict,
                replace,
            } => {
                // 分段模式：取出属于这个文件的那段文本；没有就报错（正常路径下已被 applies_to 挡掉）。
                let text = match source {
                    BookmarkSpec::Shared(text) => text.clone(),
                    BookmarkSpec::PerFile(entries) => entries
                        .iter()
                        .find(|(candidate, _)| candidate.as_path() == path)
                        .map(|(_, text)| text.clone())
                        .ok_or_else(|| BatchError::Arg(format!("{} 没有对应的书签段", path.display())))?,
                };
                let page_count = pdfcraft_organize::page_count(doc)?;
                let parsed = spec::parse_spec(&text, *zero_based, *strict, page_count)?;
                let added = bookmarks::apply(doc, &parsed.entries, *replace)?;
                let verb = if *replace { "替换为" } else { "追加" };
                Ok(RunNote {
                    summary: format!("{verb} {added} 条书签"),
                    warnings: parsed.warnings,
                    ..Default::default()
                })
            }
            Action::Fill { op, pages: range } => {
                let page_count = pdfcraft_organize::page_count(doc)?;
                let targets = pages::parse_pages(range, page_count).map_err(BatchError::Arg)?;
                Ok(match op {
                    FillOp::Add { style, replace } => {
                        let painted = fill::apply(doc, &targets, style, *replace)?;
                        RunNote {
                            summary: format!(
                                "在第 {painted} 页上盖了填充层（{}，{}，{}）",
                                style.blend.canonical(),
                                style.layer.canonical(),
                                if *replace { "已替换旧层" } else { "新增" }
                            ),
                            warnings: Vec::new(),
                            ..Default::default()
                        }
                    }
                    FillOp::Remove(matcher) => {
                        // 空条件会被 fill::remove 当作「所有本工具的层」，此处提醒一句更安全。
                        let removed = fill::remove(doc, &targets, matcher)?;
                        RunNote {
                            summary: format!("删除了 {removed} 个填充层"),
                            warnings: no_hit_warning(removed),
                            ..Default::default()
                        }
                    }
                    FillOp::Modify { matcher, change } => {
                        if change.is_empty() {
                            return Err(BatchError::Arg(
                                "修改操作没有指定任何要改的属性".to_string(),
                            ));
                        }
                        let changed = fill::modify(doc, &targets, matcher, change)?;
                        RunNote {
                            summary: format!("修改了 {changed} 个填充层"),
                            warnings: no_hit_warning(changed),
                            ..Default::default()
                        }
                    }
                })
            }
            Action::PageSize { method, pages: range } => {
                let page_count = pdfcraft_organize::page_count(doc)?;
                let targets = pages::parse_pages(range, page_count).map_err(BatchError::Arg)?;
                let all = pdfcraft_model::pages(doc);
                let mut records: Vec<PageRecord> = Vec::with_capacity(targets.len());
                for &index in &targets {
                    let page = all
                        .get(index)
                        .cloned()
                        .ok_or_else(|| BatchError::Arg(format!("文档没有第 {} 页", index + 1)))?;
                    let number = index + 1;
                    match method {
                        PageSizeMethod::Scale(spec) => match pagesize::apply_page(doc, &page, spec, true) {
                            Ok((width, height)) => records.push(PageRecord {
                                page: number,
                                result: RecordResult::Done,
                                reason: format!(
                                    "{} → {}×{}",
                                    spec.mode.label(),
                                    round2(width),
                                    round2(height)
                                ),
                                size: Some((width, height)),
                            }),
                            Err(error) => records.push(PageRecord {
                                page: number,
                                result: RecordResult::Failed,
                                reason: error.to_string(),
                                size: None,
                            }),
                        },
                        PageSizeMethod::Scan(spec) => match scan::apply_page(doc, &page, spec) {
                            Ok(ScanOutcome::Done {
                                width,
                                height,
                                images,
                            }) => records.push(PageRecord {
                                page: number,
                                result: RecordResult::Done,
                                reason: format!("贴合 {images} 张图像"),
                                size: Some((width, height)),
                            }),
                            Ok(ScanOutcome::Skipped(why)) => records.push(PageRecord {
                                page: number,
                                result: RecordResult::Skipped,
                                reason: why,
                                size: None,
                            }),
                            Err(error) => records.push(PageRecord {
                                page: number,
                                result: RecordResult::Failed,
                                reason: error.to_string(),
                                size: None,
                            }),
                        },
                    }
                }
                let done = records.iter().filter(|r| r.result == RecordResult::Done).count();
                let skipped = records.iter().filter(|r| r.result == RecordResult::Skipped).count();
                let failed = records.iter().filter(|r| r.result == RecordResult::Failed).count();
                let warnings: Vec<String> = records
                    .iter()
                    .filter(|r| r.result != RecordResult::Done)
                    .map(|r| format!("第 {} 页{}：{}", r.page, r.result.label(), r.reason))
                    .collect();
                Ok(RunNote {
                    summary: format!(
                        "{}：成功 {done} 页、跳过 {skipped} 页、失败 {failed} 页",
                        method.label()
                    ),
                    warnings,
                    records,
                })
            }
        }
    }

    /// 只统计会命中多少填充层，不改文档（界面上的「查找」用）。
    pub fn probe(&self, doc: &mut Document) -> Result<usize> {
        match self {
            Action::Fill { op, pages: range } => {
                let page_count = pdfcraft_organize::page_count(doc)?;
                let targets = pages::parse_pages(range, page_count).map_err(BatchError::Arg)?;
                match op {
                    FillOp::Add { .. } => Ok(0),
                    FillOp::Remove(matcher) | FillOp::Modify { matcher, .. } => {
                        fill::count(doc, &targets, matcher)
                    }
                }
            }
            Action::Bookmarks { .. } | Action::PageSize { .. } => Ok(0),
        }
    }

    /// 这个文件是否需要处理。分段书签下，没有对应段的文件会被跳过。
    pub fn applies_to(&self, path: &Path) -> bool {
        match self {
            Action::Bookmarks { spec: BookmarkSpec::PerFile(entries), .. } => {
                entries.iter().any(|(candidate, _)| candidate.as_path() == path)
            }
            _ => true,
        }
    }
}

/// 保留两位小数（日志里显示尺寸用）。
fn round2(value: f64) -> f64 {
    (value * 100.0).round() / 100.0
}

/// 删除/修改命中数为 0 时的提示（不致命，只是提醒条件可能写窄了）。
fn no_hit_warning(hits: usize) -> Vec<String> {
    if hits == 0 {
        vec!["没有匹配到任何填充层".to_string()]
    } else {
        Vec::new()
    }
}

/// 把书签文本整理成动作规格：没有段头 → 所有文件共用一份；有段头 → 按文件分配。
pub fn bookmark_spec(text: &str, inputs: &[Input]) -> std::result::Result<BookmarkSpec, String> {
    match spec::split_sections(text).map_err(|error| error.to_string())? {
        spec::SpecSections::Shared(text) => Ok(BookmarkSpec::Shared(text)),
        spec::SpecSections::Sections(sections) => {
            let binding = spec::bind_sections(&sections, inputs)?;
            let entries = binding
                .per_file
                .into_iter()
                .enumerate()
                .filter_map(|(index, text)| text.map(|text| (inputs[index].path.clone(), text)))
                .collect();
            Ok(BookmarkSpec::PerFile(entries))
        }
    }
}

/// 一个文件的处理结果。
#[derive(Debug)]
pub struct Outcome {
    pub input: PathBuf,
    pub output: PathBuf,
    pub status: OutcomeStatus,
}

#[derive(Debug)]
pub enum OutcomeStatus {
    /// 处理完成。
    Done(RunNote),
    /// 输入本身就带后缀，按约定跳过（让命令可以重复运行）。
    Skipped(String),
    /// 处理失败。
    Failed(BatchError),
}

impl Outcome {
    pub fn is_failure(&self) -> bool {
        matches!(self.status, OutcomeStatus::Failed(_))
    }
}

/// 逐个文件执行动作；每完成一个就回调一次（命令行立即打印，图形界面更新列表）。
///
/// `cancel` 一旦为真，剩下的文件不再处理（已经处理完的结果保留）。
pub fn run_jobs<F>(
    jobs: &[Job],
    output: &OutputMode,
    password: Option<&str>,
    full_save: bool,
    stop_on_error: bool,
    action: &Action,
    cancel: &AtomicBool,
    mut on_outcome: F,
) -> Vec<Outcome>
where
    F: FnMut(&Outcome),
{
    let mut outcomes = Vec::with_capacity(jobs.len());
    for job in jobs {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        let status = if !action.applies_to(&job.input) {
            // 分段书签：这个文件没有对应的段，跳过。
            OutcomeStatus::Skipped("没有对应的书签段".to_string())
        } else {
            match output {
                // 同目录另存 / 指定目录时，输入已经带上本次命名就跳过，让命令可以重复运行而不叠加。
                OutputMode::SameFolder { naming } | OutputMode::Dir { naming, .. }
                    if batch::already_named(&job.input, naming) =>
                {
                    OutcomeStatus::Skipped(format!("文件名已含{}", naming.describe()))
                }
                _ => process_one(job, password, full_save, action),
            }
        };
        let outcome = Outcome {
            input: job.input.clone(),
            output: job.output.clone(),
            status,
        };
        on_outcome(&outcome);
        let failed = outcome.is_failure();
        outcomes.push(outcome);
        // 「失败即停」：把取消标志立起来，下一轮循环就会退出（已处理的文件保留）。
        if stop_on_error && failed {
            cancel.store(true, Ordering::Relaxed);
        }
    }
    outcomes
}

/// 打开一个文件、按需修改、原子写回。
fn process_one(job: &Job, password: Option<&str>, full_save: bool, action: &Action) -> OutcomeStatus {
    let work = || -> Result<RunNote> {
        let mut doc = pdf::open(&job.input, password).map_err(|e| e.in_file(&job.input))?;
        let note = action.run(&mut doc, &job.input).map_err(|e| e.in_file(&job.input))?;
        let bytes = pdf::save(&mut doc, full_save).map_err(|e| e.in_file(&job.input))?;
        batch::write_atomic(&job.output, &bytes).map_err(|e| e.in_file(&job.output))?;
        Ok(note)
    };
    match work() {
        Ok(note) => OutcomeStatus::Done(note),
        Err(error) => OutcomeStatus::Failed(error),
    }
}

/// 加密 PDF 的密码来自一个文件：文件内容即密码，去掉行尾的换行符。
pub fn read_password_file(path: &Path) -> Result<String> {
    let text = std::fs::read_to_string(path)?;
    Ok(text.trim_end_matches(['\r', '\n']).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fill_style() -> FillStyle {
        FillStyle {
            rect: Some([10.0, 10.0, 50.0, 20.0]),
            color: [1.0, 0.0, 0.0],
            opacity: 0.5,
            blend: crate::Blend::Multiply,
            layer: crate::Layer::Top,
        }
    }

    fn fill_action(pages: &str) -> Action {
        Action::Fill {
            op: FillOp::Add {
                style: fill_style(),
                replace: false,
            },
            pages: pages.to_string(),
        }
    }

    /// 一个两页的最小文档，用于给动作一个真实的落点。
    fn two_page_document() -> Document {
        use pdfcraft_cos::{Dict, Object};
        let mut doc = Document::new_empty();
        let pages_ref = doc.add(Dict::new());
        let page1 = doc.add(Dict::new());
        let page2 = doc.add(Dict::new());
        let root = doc.root().expect("new_empty 有根节点");
        doc.update_dict(root, |d| {
            d.set(b"Type".to_vec(), Object::name("Catalog"));
            d.set(b"Pages".to_vec(), Object::Ref(pages_ref));
        })
        .expect("写根字典");
        doc.update_dict(pages_ref, |d| {
            d.set(b"Type".to_vec(), Object::name("Pages"));
            d.set(b"Kids".to_vec(), Object::Array(vec![Object::Ref(page1), Object::Ref(page2)]));
            d.set(b"Count".to_vec(), Object::Int(2));
        })
        .expect("写 Pages 字典");
        for page in [page1, page2] {
            doc.update_dict(page, |d| {
                d.set(b"Type".to_vec(), Object::name("Page"));
                d.set(b"Parent".to_vec(), Object::Ref(pages_ref));
                d.set(
                    b"MediaBox".to_vec(),
                    Object::Array(vec![Object::Int(0), Object::Int(0), Object::Int(200), Object::Int(300)]),
                );
            })
            .expect("写 Page 字典");
        }
        doc
    }

    #[test]
    fn fill_action_paints_the_requested_pages() {
        let mut doc = two_page_document();
        let note = fill_action("1").run(&mut doc, Path::new("in-memory.pdf")).unwrap();
        assert!(note.summary.contains("在第 1 页"), "{}", note.summary);
        assert!(note.warnings.is_empty());
    }

    #[test]
    fn bookmark_action_reports_counts_and_warnings() {
        let mut doc = two_page_document();
        let action = Action::Bookmarks {
            // 第二行故意跳级：非严格模式下应被钳制并给出警告。
            spec: BookmarkSpec::Shared("第一章 | 1\n\t\t\t1.1 | 2\n".to_string()),
            zero_based: false,
            strict: false,
            replace: true,
        };
        let note = action.run(&mut doc, Path::new("in-memory.pdf")).unwrap();
        assert!(note.summary.contains("2 条书签"), "{}", note.summary);
        assert_eq!(note.warnings.len(), 1);
    }

    #[test]
    fn fill_remove_matches_then_reports_no_hit() {
        let mut doc = two_page_document();
        fill_action("all").run(&mut doc, Path::new("in-memory.pdf")).unwrap();

        let remove = Action::Fill {
            op: FillOp::Remove(Matcher {
                color: Some([1.0, 0.0, 0.0]),
                ..Matcher::default()
            }),
            pages: "all".to_string(),
        };
        let note = remove.run(&mut doc, Path::new("in-memory.pdf")).unwrap();
        assert!(note.summary.contains("删除了 2 个"), "{}", note.summary);
        assert!(note.warnings.is_empty());

        // 再删一次：已经没有可命中的层，给出提示而不是报错。
        let note = remove.run(&mut doc, Path::new("in-memory.pdf")).unwrap();
        assert!(note.summary.contains("删除了 0 个"), "{}", note.summary);
        assert_eq!(note.warnings.len(), 1);
    }

    #[test]
    fn fill_modify_changes_matched_layers() {
        let mut doc = two_page_document();
        fill_action("all").run(&mut doc, Path::new("in-memory.pdf")).unwrap();

        let modify = Action::Fill {
            op: FillOp::Modify {
                matcher: Matcher {
                    opacity: Some(0.5),
                    ..Matcher::default()
                },
                change: FillChange {
                    color: Some([0.0, 1.0, 0.0]),
                    ..FillChange::default()
                },
            },
            pages: "all".to_string(),
        };
        let note = modify.run(&mut doc, Path::new("in-memory.pdf")).unwrap();
        assert!(note.summary.contains("修改了 2 个"), "{}", note.summary);

        // 改后新颜色成了可匹配条件，旧颜色不再命中。
        let by_new = Action::Fill {
            op: FillOp::Remove(Matcher {
                color: Some([0.0, 1.0, 0.0]),
                ..Matcher::default()
            }),
            pages: "all".to_string(),
        };
        assert!(by_new.run(&mut doc, Path::new("in-memory.pdf")).unwrap().summary.contains("删除了 2 个"));
    }

    #[test]
    fn modify_without_any_change_is_rejected() {
        let mut doc = two_page_document();
        let modify = Action::Fill {
            op: FillOp::Modify {
                matcher: Matcher::default(),
                change: FillChange::default(),
            },
            pages: "all".to_string(),
        };
        let error = modify.run(&mut doc, Path::new("in-memory.pdf")).unwrap_err();
        assert!(matches!(error, BatchError::Arg(_)), "{error}");
    }

    #[test]
    fn page_size_scale_reports_one_record_per_page() {
        let mut doc = two_page_document();
        let action = Action::PageSize {
            method: PageSizeMethod::Scale(ScaleSpec {
                size: (300.0, 400.0),
                orientation: crate::pagesize::Orientation::Portrait,
                mode: crate::pagesize::ScaleMode::Fit,
                percent: 100.0,
                rotation: crate::pagesize::Rotation::Keep,
            }),
            pages: "1".to_string(),
        };
        let note = action.run(&mut doc, Path::new("in-memory.pdf")).unwrap();
        assert!(note.summary.contains("成功 1 页"), "{}", note.summary);
        assert_eq!(note.records.len(), 1);
        assert_eq!(note.records[0].page, 1);
        assert_eq!(note.records[0].result, RecordResult::Done);
        assert_eq!(note.records[0].size, Some((300.0, 400.0)));
        assert!(note.warnings.is_empty());
    }

    #[test]
    fn page_size_scan_skips_pages_without_images() {
        let mut doc = two_page_document();
        let action = Action::PageSize {
            method: PageSizeMethod::Scan(ScanSpec::default()),
            pages: "all".to_string(),
        };
        let note = action.run(&mut doc, Path::new("in-memory.pdf")).unwrap();
        assert!(note.summary.contains("跳过 2 页"), "{}", note.summary);
        assert_eq!(note.records.len(), 2);
        assert!(note.records.iter().all(|r| r.result == RecordResult::Skipped));
        assert_eq!(note.warnings.len(), 2, "跳过应当各给一条提示");
    }

    #[test]
    fn out_of_range_page_is_an_error_per_document() {
        let mut doc = two_page_document();
        let error = fill_action("1-9").run(&mut doc, Path::new("in-memory.pdf")).unwrap_err();
        assert!(matches!(error, BatchError::Arg(_)), "{error}");
    }

    #[test]
    fn per_file_spec_only_applies_to_its_own_file() {
        let action = Action::Bookmarks {
            spec: BookmarkSpec::PerFile(vec![(PathBuf::from("D:/a.pdf"), "甲 | 1\n".to_string())]),
            zero_based: false,
            strict: false,
            replace: true,
        };
        assert!(action.applies_to(Path::new("D:/a.pdf")));
        assert!(!action.applies_to(Path::new("D:/b.pdf")));

        let shared = Action::Bookmarks {
            spec: BookmarkSpec::Shared("甲 | 1\n".to_string()),
            zero_based: false,
            strict: false,
            replace: true,
        };
        assert!(shared.applies_to(Path::new("D:/whatever.pdf")));
    }

    #[test]
    fn bookmark_spec_splits_and_binds_by_file_name() {
        let inputs = vec![
            Input {
                path: PathBuf::from("D:/x/a.pdf"),
                relative: PathBuf::from("a.pdf"),
            },
            Input {
                path: PathBuf::from("D:/y/b.pdf"),
                relative: PathBuf::from("b.pdf"),
            },
        ];
        match bookmark_spec("# 文件：a.pdf\n甲 | 1\n# 文件：b.pdf\n乙 | 2\n", &inputs).unwrap() {
            BookmarkSpec::PerFile(entries) => {
                assert_eq!(entries.len(), 2);
                assert_eq!(entries[0].0, PathBuf::from("D:/x/a.pdf"));
                assert!(entries[1].1.contains("乙"), "{:?}", entries[1].1);
            }
            other => panic!("应当是分段：{other:?}"),
        }
        // 没有段头 → 公共规格；找不到文件 → 报错。
        assert!(matches!(bookmark_spec("甲 | 1\n", &inputs).unwrap(), BookmarkSpec::Shared(_)));
        assert!(bookmark_spec("# 文件：missing.pdf\n甲 | 1\n", &inputs).is_err());
    }
}