//! 页面内容流与流标记的公共操作。
//!
//! 填充层（`fill.rs`）与页面变换（`transform.rs`）都要读写页面的 `/Contents`，
//! 也都靠流字典里的 `/PBMark` 标记来识别「哪些流是自己写进去的」，这里放共用的部分。

use pdfcraft_cos::{Dict, Document, Object};
use pdfcraft_model::Page;

use crate::error::Result;

/// 我们写在流字典里的标记键；值由各模块自己定（`PBFill`、`PBScale`…）。
pub const MARK_KEY: &[u8] = b"PBMark";

/// 页面 `/Contents` 的流引用列表。
///
/// 四种形态都要处理：缺失、间接引用指向数组、数组、单个流（提升为间接对象）。
pub fn page_contents(doc: &mut Document, page: &Page) -> Result<Vec<Object>> {
    let dict = doc.get(page.obj).as_dict().cloned().unwrap_or_default();
    Ok(match dict.get(b"Contents").cloned() {
        None => Vec::new(),
        Some(Object::Ref(r)) => match doc.get(r).as_array() {
            Some(array) => array.clone(),
            None => vec![Object::Ref(r)],
        },
        Some(Object::Array(array)) => array,
        Some(other) => vec![Object::Ref(doc.add(other))],
    })
}

/// 把内容流列表写回页面的 `/Contents`。
pub fn set_page_contents(doc: &mut Document, page: &Page, list: Vec<Object>) -> Result<()> {
    doc.update_dict(page.obj, |d| d.set(b"Contents".to_vec(), Object::Array(list)))?;
    Ok(())
}

/// 读一个对象（流）字典里的标记，不解压内容。
pub fn stream_tag(doc: &Document, object: &Object) -> Option<String> {
    doc.resolve(object)
        .as_dict()?
        .name(MARK_KEY)
        .map(|name| String::from_utf8_lossy(name).into_owned())
}

/// 带标记的流字典。
pub fn tagged(tag: &str) -> Dict {
    let mut dict = Dict::new();
    dict.set(MARK_KEY.to_vec(), Object::name(tag));
    dict
}
