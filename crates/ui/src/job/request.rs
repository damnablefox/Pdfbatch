//! 请求构造：把界面参数整理成一次不可变的执行请求，并做全部静态校验。

use pdfbatch_core::batch::{self, Input, Job, Naming, OutputMode};
use pdfbatch_core::report::LogFormat;
use pdfbatch_core::spec::{self, SpecSections};
use pdfbatch_core::{Action, FillOp, FillStyle, PageSizeMethod};

use super::bookmark_draft::{BookmarkDraft, CommonOptions};
use super::fill_draft::{fill_change, fill_matcher, pages_text, rect_pt, srgb, FillDraft};
use super::mode::{FillAction, FillEditMode, Mode, OutputChoice};
use super::pagesize_draft::{scan_spec, scale_spec, PageSizeDraft, SizeMethod};

/// 一次任务的不可变快照：执行期间改界面参数不会影响它。
#[derive(Clone, Debug)]
pub struct Request {
    pub action: Action,
    pub inputs: Vec<Input>,
    pub output: OutputMode,
    pub password: Option<String>,
    pub full_save: bool,
    pub stop_on_error: bool,
    /// 处理日志格式；`None` 表示不产出。位置是可执行文件同级目录。
    pub log: Option<LogFormat>,
}

impl Request {
    /// 把输入规划成逐个文件的输出任务。
    pub fn jobs(&self) -> Result<Vec<Job>, String> {
        batch::plan(&self.inputs, &self.output).map_err(|error| error.to_string())
    }
}

/// 参数是否已经可以执行。返回给用户看的消息（空表示可以）。
///
/// 不碰文件系统，因此可以在每一帧调用（执行按钮的可用状态与原因提示都靠它）。
pub fn preflight(
    mode: Mode,
    fill: &FillDraft,
    pagesize: &PageSizeDraft,
    bookmarks: &BookmarkDraft,
    common: &CommonOptions,
    inputs: &[Input],
) -> Result<(), String> {
    if inputs.is_empty() {
        return Err("请先添加 PDF 文件或文件夹".to_string());
    }
    if common.output == OutputChoice::Dir && common.dir.is_none() {
        return Err("请先选择输出目录".to_string());
    }
    // 覆盖原文件时不改名，前后缀不参与，不必校验。
    if common.output != OutputChoice::InPlace {
        validate_naming(&common.prefix, &common.suffix)?;
    }
    match mode {
        Mode::Fill => match fill.action {
            FillAction::Add => {
                if !fill.whole_page && !(fill.rect[2] > 0.0 && fill.rect[3] > 0.0) {
                    return Err("填充矩形的宽和高必须大于 0".to_string());
                }
                validate_page_syntax(&fill.pages)?;
            }
            FillAction::Edit => {
                if fill.edit_mode == FillEditMode::Modify && fill_change(fill).is_empty() {
                    return Err("修改模式至少要勾选一项要改的属性".to_string());
                }
                validate_page_syntax(&fill.pages)?;
            }
        },
        Mode::PageSize => {
            validate_page_syntax(&pagesize.pages)?;
            match pagesize.method {
                SizeMethod::Scale => {
                    scale_spec(pagesize)?;
                }
                SizeMethod::Scan => {
                    scan_spec(pagesize)?;
                }
            }
        }
        Mode::Bookmarks => {
            if bookmarks.text.trim().is_empty() {
                return Err("书签文本是空的".to_string());
            }
            // 结构校验用极大的页数，避免把"页码越界"误判成错误——越界在运行时按各文档裁定。
            match spec::split_sections(&bookmarks.text).map_err(|error| error.to_string())? {
                SpecSections::Shared(text) => {
                    spec::parse_spec(&text, bookmarks.zero_based, bookmarks.strict, usize::MAX)
                        .map_err(|error| error.to_string())?;
                }
                SpecSections::Sections(sections) => {
                    // 段头与文件的绑定必须先成立（找不到 / 冲突都会报错）。
                    spec::bind_sections(&sections, inputs)?;
                    for section in &sections {
                        spec::parse_spec(&section.text, bookmarks.zero_based, bookmarks.strict, usize::MAX)
                            .map_err(|error| error.to_string())?;
                    }
                }
            }
        }
    }
    Ok(())
}

/// 输出文件名的合法性检查：Windows 不允许的字符，以及结尾的空格与点。
pub fn validate_naming(prefix: &str, suffix: &str) -> Result<(), String> {
    const ILLEGAL: [char; 9] = ['\\', '/', ':', '*', '?', '"', '<', '>', '|'];
    for (label, text) in [("前缀", prefix), ("后缀", suffix)] {
        if let Some(ch) = text.chars().find(|c| ILLEGAL.contains(c) || c.is_control()) {
            return Err(format!("文件名{label}不能包含字符 `{ch}`"));
        }
        if text.ends_with(' ') || text.ends_with('.') {
            return Err(format!("文件名{label}不能以空格或点结尾"));
        }
    }
    Ok(())
}

/// 实际生效的命名。
///
/// 同目录另存时前后缀都为空会回退到模式默认后缀，保证绝不覆盖原文件；
/// 指定目录时前后缀都为空则保持原名（与命令行一致）。
pub fn effective_naming(mode: Mode, common: &CommonOptions) -> Naming {
    let prefix = common.prefix.clone();
    let suffix = common.suffix.clone();
    let both_empty = prefix.is_empty() && suffix.trim().is_empty();
    if common.output == OutputChoice::SameFolder && both_empty {
        Naming {
            prefix,
            suffix: mode.suffix().to_string(),
        }
    } else {
        Naming { prefix, suffix }
    }
}

/// 把界面参数整理成一次请求；所有非法输入在这里被拒绝（返回给用户看的消息）。
pub fn build_request(
    mode: Mode,
    fill: &FillDraft,
    pagesize: &PageSizeDraft,
    bookmarks: &BookmarkDraft,
    common: &CommonOptions,
    inputs: Vec<Input>,
) -> Result<Request, String> {
    preflight(mode, fill, pagesize, bookmarks, common, &inputs)?;

    let output = match common.output {
        OutputChoice::SameFolder => OutputMode::SameFolder {
            naming: effective_naming(mode, common),
        },
        OutputChoice::Dir => OutputMode::Dir {
            dir: common.dir.clone().expect("preflight 已确认目录存在"),
            naming: effective_naming(mode, common),
        },
        OutputChoice::InPlace => OutputMode::InPlace,
    };

    let action = match mode {
        Mode::Fill => {
            let op = match fill.action {
                FillAction::Add => {
                    let rect = if fill.whole_page { None } else { Some(rect_pt(fill.unit, fill.rect)) };
                    FillOp::Add {
                        style: FillStyle {
                            rect,
                            color: srgb(fill.color),
                            opacity: fill.opacity.clamp(0.0, 1.0),
                            blend: fill.blend,
                            layer: fill.layer,
                        },
                        // 界面不再提供「先摘掉已有填充层」，一律按追加处理。
                        replace: false,
                    }
                }
                FillAction::Edit => match fill.edit_mode {
                    FillEditMode::Erase => FillOp::Remove(fill_matcher(fill)),
                    FillEditMode::Modify => FillOp::Modify {
                        matcher: fill_matcher(fill),
                        change: fill_change(fill),
                    },
                },
            };
            Action::Fill {
                op,
                pages: pages_text(fill),
            }
        }
        Mode::PageSize => {
            let method = match pagesize.method {
                SizeMethod::Scale => PageSizeMethod::Scale(scale_spec(pagesize)?),
                SizeMethod::Scan => PageSizeMethod::Scan(scan_spec(pagesize)?),
            };
            Action::PageSize {
                method,
                pages: if pagesize.pages.trim().is_empty() {
                    "all".to_string()
                } else {
                    pagesize.pages.clone()
                },
            }
        }
        Mode::Bookmarks => Action::Bookmarks {
            // 有段头就按文件分配；没有段头则所有文件共用这份文本。
            spec: pdfbatch_core::bookmark_spec(&bookmarks.text, &inputs)?,
            zero_based: bookmarks.zero_based,
            strict: bookmarks.strict,
            replace: bookmarks.replace,
        },
    };

    let password = {
        let trimmed = common.password.trim();
        (!trimmed.is_empty()).then(|| trimmed.to_string())
    };

    Ok(Request {
        action,
        inputs,
        output,
        password,
        full_save: common.full_save,
        stop_on_error: common.stop_on_error,
        log: common.log,
    })
}

/// 构造一次「只查找」的请求：删/改模式下，执行前先统计会命中多少填充层。
///
/// 与 `build_request` 不同，这里不要求配置更改要求——查找只关心匹配条件本身。
pub fn build_probe_request(
    mode: Mode,
    fill: &FillDraft,
    common: &CommonOptions,
    inputs: Vec<Input>,
) -> Result<Request, String> {
    if inputs.is_empty() {
        return Err("请先添加 PDF 文件或文件夹".to_string());
    }
    if mode != Mode::Fill {
        return Err("只有填充区域需要查找".to_string());
    }
    validate_page_syntax(&fill.pages)?;
    let action = Action::Fill {
        // 用「删除」包裹匹配条件：probe 只看 matcher，不看操作种类。
        op: FillOp::Remove(fill_matcher(fill)),
        pages: pages_text(fill),
    };
    let password = {
        let trimmed = common.password.trim();
        (!trimmed.is_empty()).then(|| trimmed.to_string())
    };
    Ok(Request {
        action,
        inputs,
        // 查找不写文件，输出位置只作占位。
        output: OutputMode::SameFolder {
            naming: Naming {
                prefix: String::new(),
                suffix: String::new(),
            },
        },
        password,
        full_save: false,
        stop_on_error: false,
        log: None,
    })
}

/// 页范围的**语法**校验：只查写法对不对、起止是否颠倒，不查是否越界
/// （越界要按每个文档的实际页数裁定，运行时逐文件处理）。
pub(crate) fn validate_page_syntax(spec: &str) -> Result<(), String> {
    // 用一个足够大的假页数只做语法检查：first/last、负索引、`..`、except 都能解析；
    // 越不越界留给运行时按各文件的实际页数裁定（例如 999999 在这里不算错）。
    pdfbatch_core::pages::parse_pages(spec, 1_000_000).map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::job::MatchRect;
    use pdfbatch_core::batch::Input;
    use pdfbatch_core::pagesize::{ScaleMode, Target};
    use pdfbatch_core::units::Unit;
    use pdfbatch_core::RectMatch;
    use std::path::PathBuf;

    fn input(path: &str) -> Input {
        Input {
            path: PathBuf::from(path),
            relative: PathBuf::from(PathBuf::from(path).file_name().unwrap()),
        }
    }

    /// 测试用的 build_request：页面尺寸草稿取默认值，省得每个用例都写一遍。
    ///
    /// 显式定义会遮蔽 `use super::*` 引入的同名函数，因此测试里的调用无需改动。
    fn build_request(
        mode: Mode,
        fill: &FillDraft,
        bookmarks: &BookmarkDraft,
        common: &CommonOptions,
        inputs: Vec<Input>,
    ) -> Result<Request, String> {
        super::build_request(mode, fill, &PageSizeDraft::default(), bookmarks, common, inputs)
    }

    fn fill_request(rect_ok: bool, pages: &str) -> Result<Request, String> {
        let fill = FillDraft {
            whole_page: false,
            rect: if rect_ok { [10.0, 10.0, 100.0, 50.0] } else { [10.0, 10.0, 0.0, 50.0] },
            pages: pages.to_string(),
            ..FillDraft::default()
        };
        build_request(Mode::Fill, &fill, &BookmarkDraft::default(), &CommonOptions::default(), vec![input("D:/a.pdf")])
    }

    #[test]
    fn empty_input_is_rejected() {
        let error = build_request(
            Mode::Fill,
            &FillDraft::default(),
            &BookmarkDraft::default(),
            &CommonOptions::default(),
            vec![],
        )
        .unwrap_err();
        assert!(error.contains("请先添加"), "{error}");
    }

    #[test]
    fn a_zero_sized_rect_is_rejected() {
        assert!(fill_request(false, "all").unwrap_err().contains("宽和高"));
    }

    #[test]
    fn page_syntax_is_checked_before_running() {
        assert!(fill_request(true, "all").is_ok());
        assert!(fill_request(true, "1-5,8").is_ok());
        assert!(fill_request(true, "10-").is_ok());
        assert!(fill_request(true, "5-3").unwrap_err().contains("起点大于终点"));
        assert!(fill_request(true, "0").unwrap_err().contains("从 1 开始"));
        assert!(fill_request(true, "x").unwrap_err().contains("不是数字"));
        // 越界不在这里拦下（没有文档页数），留给运行时按各文件裁定。
        assert!(fill_request(true, "999999").is_ok());
    }

    #[test]
    fn bookmark_text_must_parse() {
        let bad = BookmarkDraft {
            text: "只有标题没有页码\n".to_string(),
            ..BookmarkDraft::default()
        };
        let error =
            build_request(Mode::Bookmarks, &FillDraft::default(), &bad, &CommonOptions::default(), vec![input("D:/a.pdf")])
                .unwrap_err();
        assert!(error.contains("第 1 行"), "{error}");

        let empty = BookmarkDraft::default();
        assert!(
            build_request(Mode::Bookmarks, &FillDraft::default(), &empty, &CommonOptions::default(), vec![input("D:/a.pdf")])
                .unwrap_err()
                .contains("空的")
        );

        let good = BookmarkDraft {
            text: "第一章 | 1\n\t1.1 | 2\n".to_string(),
            ..BookmarkDraft::default()
        };
        assert!(
            build_request(Mode::Bookmarks, &FillDraft::default(), &good, &CommonOptions::default(), vec![input("D:/a.pdf")])
                .is_ok()
        );
    }

    #[test]
    fn bookmark_sections_are_bound_to_files() {
        let bookmarks = BookmarkDraft {
            text: "# 文件：a.pdf\n甲 | 1\n# 文件：b.pdf\n乙 | 2\n".to_string(),
            ..BookmarkDraft::default()
        };
        let request = build_request(
            Mode::Bookmarks,
            &FillDraft::default(),
            &bookmarks,
            &CommonOptions::default(),
            vec![input("D:/x/a.pdf"), input("D:/y/b.pdf")],
        )
        .unwrap();
        match request.action {
            Action::Bookmarks {
                spec: pdfbatch_core::BookmarkSpec::PerFile(entries),
                ..
            } => assert_eq!(entries.len(), 2),
            other => panic!("应当是分段书签：{other:?}"),
        }
    }

    #[test]
    fn a_section_target_without_a_file_is_rejected() {
        let bookmarks = BookmarkDraft {
            text: "# 文件：missing.pdf\n甲 | 1\n".to_string(),
            ..BookmarkDraft::default()
        };
        let error = build_request(
            Mode::Bookmarks,
            &FillDraft::default(),
            &bookmarks,
            &CommonOptions::default(),
            vec![input("D:/x/a.pdf")],
        )
        .unwrap_err();
        assert!(error.contains("找不到"), "{error}");
    }

    #[test]
    fn out_file_mode_is_not_offered_but_dir_requires_a_dir() {
        let common = CommonOptions {
            output: OutputChoice::Dir,
            dir: None,
            ..CommonOptions::default()
        };
        let error =
            build_request(Mode::Fill, &FillDraft::default(), &BookmarkDraft::default(), &common, vec![input("D:/a.pdf")])
                .unwrap_err();
        assert!(error.contains("输出目录"), "{error}");
    }

    #[test]
    fn prefix_only_does_not_fall_back_to_a_default_suffix() {
        // 只填前缀、清空后缀：同目录另存也不该再补默认后缀。
        let common = CommonOptions {
            prefix: "new_".to_string(),
            suffix: String::new(),
            ..CommonOptions::default()
        };
        let request = build_request(
            Mode::Fill,
            &FillDraft::default(),
            &BookmarkDraft::default(),
            &common,
            vec![input("D:/a.pdf")],
        )
        .unwrap();
        match request.output {
            OutputMode::SameFolder { naming } => {
                assert_eq!(naming.prefix, "new_");
                assert!(naming.suffix.is_empty(), "不该补默认后缀");
            }
            other => panic!("应当是同目录另存：{other:?}"),
        }
    }

    #[test]
    fn both_empty_falls_back_to_the_default_suffix_in_same_folder() {
        let common = CommonOptions {
            prefix: String::new(),
            suffix: String::new(),
            ..CommonOptions::default()
        };
        let request = build_request(
            Mode::Fill,
            &FillDraft::default(),
            &BookmarkDraft::default(),
            &common,
            vec![input("D:/a.pdf")],
        )
        .unwrap();
        match request.output {
            OutputMode::SameFolder { naming } => assert_eq!(naming.suffix, "_filled", "回退默认后缀避免覆盖"),
            other => panic!("应当是同目录另存：{other:?}"),
        }
    }

    #[test]
    fn dir_naming_keeps_an_empty_suffix() {
        let common = CommonOptions {
            output: OutputChoice::Dir,
            dir: Some(PathBuf::from("D:/out")),
            prefix: "new_".to_string(),
            suffix: String::new(),
            ..CommonOptions::default()
        };
        let request = build_request(
            Mode::Fill,
            &FillDraft::default(),
            &BookmarkDraft::default(),
            &common,
            vec![input("D:/a.pdf")],
        )
        .unwrap();
        match request.output {
            OutputMode::Dir { naming, .. } => {
                assert_eq!(naming.prefix, "new_");
                assert!(naming.suffix.is_empty(), "指定目录不做默认后缀回退");
            }
            other => panic!("应当是指定目录：{other:?}"),
        }
    }

    #[test]
    fn illegal_naming_is_rejected() {
        let common = CommonOptions {
            prefix: "a/b".to_string(),
            ..CommonOptions::default()
        };
        let error = build_request(
            Mode::Fill,
            &FillDraft::default(),
            &BookmarkDraft::default(),
            &common,
            vec![input("D:/a.pdf")],
        )
        .unwrap_err();
        assert!(error.contains("不能包含"), "{error}");
    }

    /// 取色控件给的是 sRGB 分量，必须原样写进 PDF（不能当线性值处理）。
    #[test]
    fn fill_color_is_written_as_srgb() {
        let fill = FillDraft {
            color: egui::Color32::from_rgb(227, 237, 203),
            ..FillDraft::default()
        };
        let request = build_request(
            Mode::Fill,
            &fill,
            &BookmarkDraft::default(),
            &CommonOptions::default(),
            vec![input("D:/a.pdf")],
        )
        .unwrap();
        match request.action {
            Action::Fill { op: FillOp::Add { style, .. }, .. } => {
                assert!((style.color[0] - 227.0 / 255.0).abs() < 1e-9, "{:?}", style.color);
                assert!((style.color[1] - 237.0 / 255.0).abs() < 1e-9, "{:?}", style.color);
                assert!((style.color[2] - 203.0 / 255.0).abs() < 1e-9, "{:?}", style.color);
            }
            other => panic!("应当是填充动作：{other:?}"),
        }
    }

    #[test]
    fn pagesize_scale_request_carries_the_target_size() {
        let pagesize = PageSizeDraft {
            method: SizeMethod::Scale,
            preset: "A4".to_string(),
            scale_mode: ScaleMode::Fill,
            ..PageSizeDraft::default()
        };
        let request = super::build_request(
            Mode::PageSize,
            &FillDraft::default(),
            &pagesize,
            &BookmarkDraft::default(),
            &CommonOptions::default(),
            vec![input("D:/a.pdf")],
        )
        .unwrap();
        match request.action {
            Action::PageSize {
                method: PageSizeMethod::Scale(spec),
                pages,
            } => {
                assert_eq!(spec.mode, ScaleMode::Fill);
                match spec.target {
                    Target::Paper {
                        long_side,
                        short_side,
                        ..
                    } => {
                        assert!((short_side - 595.2756).abs() < 0.01, "{short_side}");
                        assert!((long_side - 841.8898).abs() < 0.01, "{long_side}");
                    }
                    other => panic!("应当是预设纸张：{other:?}"),
                }
                assert_eq!(pages, "all");
            }
            other => panic!("应当是页面尺寸：{other:?}"),
        }
    }

    #[test]
    fn pagesize_scan_request_carries_three_ppis() {
        let pagesize = PageSizeDraft {
            method: SizeMethod::Scan,
            color_ppi: 200.0,
            gray_ppi: 400.0,
            mono_ppi: 800.0,
            ..PageSizeDraft::default()
        };
        let request = super::build_request(
            Mode::PageSize,
            &FillDraft::default(),
            &pagesize,
            &BookmarkDraft::default(),
            &CommonOptions::default(),
            vec![input("D:/a.pdf")],
        )
        .unwrap();
        match request.action {
            Action::PageSize {
                method: PageSizeMethod::Scan(spec),
                ..
            } => {
                assert_eq!(spec.color_ppi, 200.0);
                assert_eq!(spec.gray_ppi, 400.0);
                assert_eq!(spec.mono_ppi, 800.0);
            }
            other => panic!("应当是页面尺寸：{other:?}"),
        }
    }

    #[test]
    fn pagesize_preflight_rejects_bad_input() {
        let zero_percent = PageSizeDraft {
            scale_mode: ScaleMode::CenterScaled,
            percent: 0.0,
            ..PageSizeDraft::default()
        };
        let error = super::build_request(
            Mode::PageSize,
            &FillDraft::default(),
            &zero_percent,
            &BookmarkDraft::default(),
            &CommonOptions::default(),
            vec![input("D:/a.pdf")],
        )
        .unwrap_err();
        assert!(error.contains("百分比"), "{error}");

        let zero_ppi = PageSizeDraft {
            method: SizeMethod::Scan,
            mono_ppi: 0.0,
            ..PageSizeDraft::default()
        };
        let error = super::build_request(
            Mode::PageSize,
            &FillDraft::default(),
            &zero_ppi,
            &BookmarkDraft::default(),
            &CommonOptions::default(),
            vec![input("D:/a.pdf")],
        )
        .unwrap_err();
        assert!(error.contains("PPI"), "{error}");
    }

    #[test]
    fn erase_request_carries_the_match_conditions() {
        let fill = FillDraft {
            action: FillAction::Edit,
            edit_mode: FillEditMode::Erase,
            match_rect: MatchRect::FullPage,
            match_color_on: true,
            match_color: egui::Color32::from_rgb(255, 224, 140),
            pages: "all except first,last".to_string(),
            ..FillDraft::default()
        };
        let request = build_request(
            Mode::Fill,
            &fill,
            &BookmarkDraft::default(),
            &CommonOptions::default(),
            vec![input("D:/a.pdf")],
        )
        .unwrap();
        match request.action {
            Action::Fill { op: FillOp::Remove(m), pages } => {
                assert_eq!(m.rect, Some(RectMatch::FullPage));
                assert!(m.color.is_some());
                assert_eq!(pages, "all except first,last");
            }
            other => panic!("应当是删除：{other:?}"),
        }
    }

    #[test]
    fn modify_requires_at_least_one_change() {
        let empty = FillDraft {
            action: FillAction::Edit,
            edit_mode: FillEditMode::Modify,
            ..FillDraft::default()
        };
        let error = build_request(
            Mode::Fill,
            &empty,
            &BookmarkDraft::default(),
            &CommonOptions::default(),
            vec![input("D:/a.pdf")],
        )
        .unwrap_err();
        assert!(error.contains("勾选"), "{error}");

        let filled = FillDraft {
            action: FillAction::Edit,
            edit_mode: FillEditMode::Modify,
            change_color_on: true,
            change_color: egui::Color32::from_rgb(0, 255, 0),
            ..FillDraft::default()
        };
        let request = build_request(
            Mode::Fill,
            &filled,
            &BookmarkDraft::default(),
            &CommonOptions::default(),
            vec![input("D:/a.pdf")],
        )
        .unwrap();
        match request.action {
            Action::Fill { op: FillOp::Modify { change, .. }, .. } => {
                assert_eq!(change.color, Some([0.0, 1.0, 0.0]));
                assert_eq!(change.opacity, None, "没勾的维度不该动");
                assert_eq!(change.full_page, None);
            }
            other => panic!("应当是修改：{other:?}"),
        }
    }

    #[test]
    fn fill_rect_is_converted_from_the_chosen_unit() {
        let fill = FillDraft {
            whole_page: false,
            unit: Unit::Mm,
            rect: [20.0, 20.0, 150.0, 50.0],
            ..FillDraft::default()
        };
        let request = build_request(
            Mode::Fill,
            &fill,
            &BookmarkDraft::default(),
            &CommonOptions::default(),
            vec![input("D:/a.pdf")],
        )
        .unwrap();
        match request.action {
            Action::Fill { op: FillOp::Add { style, .. }, .. } => {
                let rect = style.rect.expect("自定义矩形");
                // 150 mm = 150 ÷ 25.4 × 72 pt ≈ 425.197 pt；50 mm ≈ 141.732 pt。
                assert!((rect[2] - 425.1968).abs() < 0.01, "{rect:?}");
                assert!((rect[3] - 141.7323).abs() < 0.01, "{rect:?}");
            }
            other => panic!("应当是新增：{other:?}"),
        }
    }

    #[test]
    fn size_tolerance_shares_the_chosen_unit() {
        let fill = FillDraft {
            action: FillAction::Edit,
            edit_mode: FillEditMode::Erase,
            match_rect: MatchRect::Size,
            match_size: [150.0, 50.0],
            rect_tolerance: 1.0,
            unit: Unit::Mm,
            ..FillDraft::default()
        };
        let request = build_request(
            Mode::Fill,
            &fill,
            &BookmarkDraft::default(),
            &CommonOptions::default(),
            vec![input("D:/a.pdf")],
        )
        .unwrap();
        match request.action {
            Action::Fill { op: FillOp::Remove(m), .. } => {
                match m.rect {
                    Some(RectMatch::Size([w, h])) => {
                        assert!((w - 425.1968).abs() < 0.01, "{w}");
                        assert!((h - 141.7323).abs() < 0.01, "{h}");
                    }
                    other => panic!("应当是宽高匹配：{other:?}"),
                }
                // 1 mm 容差 ≈ 2.8346 点：容差与尺寸同单位，内部统一换算成点。
                assert!((m.rect_tolerance - 2.8346).abs() < 0.001, "{}", m.rect_tolerance);
            }
            other => panic!("应当是删除：{other:?}"),
        }
    }
}
