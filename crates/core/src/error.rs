//! 统一错误类型。

use std::path::PathBuf;

use crate::spec::SpecError;

/// 本工具所有可能失败的汇总。`File` 变体用于在批量处理时把出错的文件路径带上。
#[derive(Debug, thiserror::Error)]
pub enum BatchError {
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Cos(#[from] pdfcraft_cos::CosError),
    #[error("{0}")]
    Outline(#[from] pdfcraft_organize::OutlineError),
    #[error("{0}")]
    Organize(#[from] pdfcraft_organize::OrganizeError),
    #[error("{0}")]
    Spec(#[from] SpecError),
    /// 命令行参数本身有问题。
    #[error("{0}")]
    Arg(String),
    /// 单个文件处理失败（批量模式下逐文件收集）。
    #[error("{path}：{source}")]
    File {
        path: PathBuf,
        source: Box<BatchError>,
    },
}

impl BatchError {
    /// 把一个错误标注上它所发生的文件。
    pub fn in_file(self, path: impl Into<PathBuf>) -> Self {
        BatchError::File {
            path: path.into(),
            source: Box::new(self),
        }
    }
}

pub type Result<T> = std::result::Result<T, BatchError>;