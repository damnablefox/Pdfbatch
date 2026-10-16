//! 书签草稿，以及两种模式共用的输出与读取选项。

use std::path::PathBuf;

use pdfbatch_core::report::LogFormat;

use super::mode::{Mode, OutputChoice};

/// 书签模式的参数草稿。
#[derive(Clone, Debug, Default)]
pub struct BookmarkDraft {
    /// 选中的文本文件（仅作记录；实际用的是 `text`）。
    pub source: Option<PathBuf>,
    pub text: String,
    pub zero_based: bool,
    pub strict: bool,
    pub replace: bool,
}

/// 两种模式共用的输出与读取选项。
#[derive(Clone, Debug)]
pub struct CommonOptions {
    pub output: OutputChoice,
    /// 指定输出目录（`OutputChoice::Dir` 时需要）。
    pub dir: Option<PathBuf>,
    /// 输出文件名前缀（同目录另存与指定目录都适用）。
    pub prefix: String,
    /// 输出文件名后缀。
    pub suffix: String,
    /// 加密 PDF 的密码；空字符串表示没有。
    pub password: String,
    /// 整体重写文件（体积更小，但会让已有数字签名失效）。
    pub full_save: bool,
    /// 处理日志格式；`None` 表示不产出日志。位置固定为可执行文件同级目录。
    pub log: Option<LogFormat>,
    /// 某个文件失败后是否停止整批。
    pub stop_on_error: bool,
}

impl Default for CommonOptions {
    fn default() -> Self {
        Self {
            output: OutputChoice::SameFolder,
            dir: None,
            prefix: String::new(),
            suffix: Mode::Fill.suffix().to_string(),
            password: String::new(),
            full_save: false,
            log: None,
            stop_on_error: false,
        }
    }
}
