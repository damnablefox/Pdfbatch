//! 执行：把界面上的参数整理成一次可运行的请求，并在后台线程里跑批量任务。
//!
//! 按主题拆成若干子模块，本文件只做声明与重导出，保持 `crate::job::X` 路径不变：
//! `mode` 界面动作枚举、`fill_draft` 填充草稿、`pagesize_draft` 页面尺寸草稿、
//! `bookmark_draft` 书签与公共选项、`request` 请求构造、`validate` 静态校验、`run` 后台执行。

mod bookmark_draft;
mod fill_draft;
mod mode;
mod pagesize_draft;
mod request;
mod run;
mod validate;

pub use bookmark_draft::{BookmarkDraft, CommonOptions};
pub use fill_draft::{matcher_fingerprint, FillDraft};
pub use mode::{ChangeRect, FillAction, FillEditMode, MatchRect, Mode, OutputChoice};
pub use pagesize_draft::{allowed_modes, legalize, PageSizeDraft, SizeMethod, SizeSource};
pub use request::{build_probe_request, build_request, effective_naming};
// `Request` 只在子模块内部引用，这里保留重导出以维持 `crate::job::Request` 路径。
#[allow(unused_imports)]
pub use request::Request;
pub use run::{spawn_job, spawn_probe, JobProgress, JobRun, ProbeReport, ProbeRun};
pub use validate::{preflight, validate_naming};

pub(crate) use validate::validate_page_syntax;
