//! pdfbatch-core —— 批量处理 PDF 的核心逻辑。
//!
//! 图形界面（`pdfbatch`）与命令行（`pdfbatch-cli`）共享这一份实现，避免两套行为漂移。
//! 引擎来自 PdfCraft 的库 crate（对象层、书签、页面模型），本 crate 只负责：
//! 解析文本规格、构造内容流、遍历文件、写回结果。

pub mod action;
pub mod batch;
pub mod blend;
pub mod bookmarks;
pub mod color;
pub mod content;
pub mod error;
pub mod fill;
pub mod pages;
pub mod pagesize;
pub mod pdf;
// 执行层与记录层：只在 crate 内部使用，对外仍经 `action` 重新导出。
mod pipeline;
mod record;
pub mod report;
pub mod scan;
pub mod spec;
pub mod transform;
pub mod units;

pub use action::{
    Action, BookmarkSpec, FillOp, Outcome, OutcomeStatus, PageRecord, PageSizeMethod, RecordResult,
    bookmark_spec, run_jobs,
};
pub use batch::{Input, Job, Naming, OutputMode};
pub use blend::Blend;
pub use error::{BatchError, Result};
pub use fill::{FillChange, FillInfo, FillStyle, Layer, Matcher, RectMatch};