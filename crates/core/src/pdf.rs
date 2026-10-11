//! 打开与保存 PDF 的薄封装。

use std::path::Path;
use std::sync::Arc;

use pdfcraft_cos::{CosError, Document, SaveOptions};

use crate::error::{BatchError, Result};

/// 读取并打开一个 PDF。加密文档需要 `password`。
pub fn open(path: &Path, password: Option<&str>) -> Result<Document> {
    let bytes = std::fs::read(path)?;
    let opened = match password {
        Some(pw) => Document::open_with_password(Arc::new(bytes), Some(pw)),
        None => Document::open(Arc::new(bytes)),
    };
    opened.map_err(|e| match e {
        CosError::NeedsPassword => BatchError::Arg("文档已加密，请用 --password-file 提供密码".to_string()),
        CosError::WrongPassword => BatchError::Arg("密码不正确".to_string()),
        other => BatchError::Cos(other),
    })
}

/// 序列化文档。
///
/// 默认增量保存：原始字节原样保留，已有数字签名不会失效。`full` 为真时整体重写，
/// 体积更小、旧对象被回收，但**会让已有签名失效**。
pub fn save(doc: &mut Document, full: bool) -> Result<Vec<u8>> {
    let options = SaveOptions::default();
    if full {
        doc.require_full_save();
        Ok(pdfcraft_cos::write_full(doc, &options)?)
    } else {
        Ok(pdfcraft_cos::write_incremental(doc, &options)?)
    }
}