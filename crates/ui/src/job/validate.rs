//! 静态校验：参数能否执行、输出文件名与页范围语法是否合法（都不碰文件系统）。

use pdfbatch_core::batch::Input;
use pdfbatch_core::spec::{self, SpecSections};

use super::bookmark_draft::{BookmarkDraft, CommonOptions};
use super::fill_draft::{fill_change, FillDraft};
use super::mode::{FillAction, FillEditMode, Mode, OutputChoice};
use super::pagesize_draft::{scan_spec, scale_spec, PageSizeDraft, SizeMethod};

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
    use pdfbatch_core::batch::Input;
    use pdfbatch_core::pagesize::ScaleMode;
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
    ) -> Result<crate::job::Request, String> {
        crate::job::build_request(mode, fill, &PageSizeDraft::default(), bookmarks, common, inputs)
    }

    fn fill_request(rect_ok: bool, pages: &str) -> Result<crate::job::Request, String> {
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

    #[test]
    fn pagesize_preflight_rejects_bad_input() {
        let zero_percent = PageSizeDraft {
            scale_mode: ScaleMode::CenterScaled,
            percent: 0.0,
            ..PageSizeDraft::default()
        };
        let error = crate::job::build_request(
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
        let error = crate::job::build_request(
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
}
