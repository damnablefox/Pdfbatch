//! 给页面的指定位置盖一层纯色填充，并控制混合模式与层级。
//!
//! 每页做三件事：
//!
//! 1. 在该页自己的 `Resources` 里注册一个图形状态（含 `/BM` 混合模式与 `/CA`、`/ca` 不透明度）；
//! 2. 生成一段内容流：`q /<状态> gs R G B rg x y w h re f Q`；
//! 3. 按层级写进 `/Contents`——`Bottom` 插到最前，`Top` 追加到最后（先把原有内容用 q/Q 包住）。
//!
//! 添加的内容流会打上 `/PBMark` 标记（写在流字典里，识别时无需解压页面内容），
//! `--replace` 就是靠它把上一次加的东西先摘掉。

use pdfcraft_cos::{Dict, Document, Object, Stream};
use pdfcraft_model::{Page, pages};

use crate::blend::Blend;
use crate::error::{BatchError, Result};

/// 标记键与两个标记值：填充层、包裹原有内容的 q/Q 对。
const MARK_KEY: &[u8] = b"PBMark";
const MARK_FILL: &str = "PBFill";
const MARK_WRAP: &str = "PBWrap";

/// 填充相对于页面其它内容的层级。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layer {
    /// 覆盖在原有内容之上（追加到内容流末尾）。
    Top,
    /// 垫在原有内容之下（插到内容流开头）。
    Bottom,
}

impl Layer {
    pub fn parse(s: &str) -> std::result::Result<Self, String> {
        match s.trim().to_ascii_lowercase().as_str() {
            "top" | "above" | "上" | "上层" => Ok(Layer::Top),
            "bottom" | "below" | "under" | "下" | "底层" => Ok(Layer::Bottom),
            _ => Err(format!("未知层级 `{s}`；可用值：top、bottom")),
        }
    }

    pub fn canonical(self) -> &'static str {
        match self {
            Layer::Top => "top",
            Layer::Bottom => "bottom",
        }
    }
}

/// 一次填充请求的参数。
#[derive(Clone, Copy, Debug)]
pub struct FillStyle {
    /// 用户空间下的矩形 `[x, y, 宽, 高]`；`None` 表示整页（用页面 CropBox）。
    pub rect: Option<[f64; 4]>,
    pub color: [f64; 3],
    pub opacity: f64,
    pub blend: Blend,
    pub layer: Layer,
}

/// 在给定页面上盖填充层，返回实际处理过的页数。
///
/// `replace` 为真时先摘掉本工具此前加在该页的填充层——这正是"批量改混合模式/层级"的做法。
pub fn apply(doc: &mut Document, targets: &[usize], style: &FillStyle, replace: bool) -> Result<usize> {
    let all = pages(doc);
    if all.is_empty() {
        return Err(BatchError::Arg("文档没有任何页面".to_string()));
    }
    let mut painted = 0;
    for &index in targets {
        let page = all
            .get(index)
            .ok_or_else(|| BatchError::Arg(format!("文档没有第 {} 页", index + 1)))?
            .clone();
        if replace {
            strip(doc, &page)?;
        }
        paint(doc, &page, style)?;
        painted += 1;
    }
    Ok(painted)
}

/// 在单页上完成"注册图形状态 → 生成内容流 → 按层级写入"。
fn paint(doc: &mut Document, page: &Page, style: &FillStyle) -> Result<()> {
    let crop = page.crop(doc);
    let rect = style.rect.unwrap_or([crop[0], crop[1], crop[2] - crop[0], crop[3] - crop[1]]);
    if !(rect[2] > 0.0 && rect[3] > 0.0) {
        return Err(BatchError::Arg(format!(
            "填充矩形宽高必须为正，当前是 {}×{}",
            rect[2], rect[3]
        )));
    }

    let state_name = add_gstate(doc, page, style)?;
    let content = fill_content(&state_name, style.color, rect);
    place(doc, page, content, style.layer)
}

/// 在页面自己的资源字典里注册图形状态，返回它在内容流里的名字。
fn add_gstate(doc: &mut Document, page: &Page, style: &FillStyle) -> Result<String> {
    // 名字带上参数，内容流里一眼能看出用的是什么混合模式与不透明度。
    let name = format!(
        "PBFill{}{}",
        style.blend.canonical(),
        (style.opacity * 100.0).round() as i64
    );

    let mut resources = page_owned_dict(doc, page, b"Resources");
    let mut states = resources
        .get(b"ExtGState")
        .map(|g| doc.resolve(g))
        .and_then(|g| g.as_dict().cloned())
        .unwrap_or_default();

    let mut state = Dict::new();
    state.set(b"Type".to_vec(), Object::name("ExtGState"));
    if let Some(blend) = style.blend.pdf_name() {
        state.set(b"BM".to_vec(), Object::name(blend));
    }
    state.set(b"CA".to_vec(), Object::Real(style.opacity));
    state.set(b"ca".to_vec(), Object::Real(style.opacity));
    states.set(name.clone().into_bytes(), Object::Dict(state));
    resources.set(b"ExtGState".to_vec(), Object::Dict(states));

    // 页面拿到自己的一份资源字典，共享的字典不被改动。
    doc.update_dict(page.obj, |d| d.set(b"Resources".to_vec(), Object::Dict(resources)))?;
    Ok(name)
}

/// 取页面某个键上的字典（没有就新建），并把它复制成页面自己的一份。
fn page_owned_dict(doc: &Document, page: &Page, key: &[u8]) -> Dict {
    page.dict
        .get(key)
        .map(|r| doc.resolve(r))
        .and_then(|r| r.as_dict().cloned())
        .unwrap_or_default()
}

/// 把一段内容写进页面的 `/Contents`。
///
/// - `Bottom`：插到最前；
/// - `Top`：先把页面原有内容用 q/Q 包住（隔离图形状态，避免颜色与状态"渗出"），再追加到末尾。
fn place(doc: &mut Document, page: &Page, content: Vec<u8>, layer: Layer) -> Result<()> {
    let mut list = contents(doc, page)?;
    let mark = Object::Ref(doc.add(Object::Stream(Stream::flate(tagged(MARK_FILL), &content))));

    if layer == Layer::Bottom {
        list.insert(0, mark);
    } else {
        let wrapped = list.iter().any(|o| stream_tag(doc, o).as_deref() == Some(MARK_WRAP));
        let has_original = list.iter().any(|o| stream_tag(doc, o).is_none());
        if !wrapped && has_original {
            let open = Object::Ref(doc.add(Object::Stream(Stream::from_raw(tagged(MARK_WRAP), b"q % pdfbatch\n".to_vec()))));
            let close = Object::Ref(doc.add(Object::Stream(Stream::from_raw(tagged(MARK_WRAP), b"Q % pdfbatch\n".to_vec()))));
            // 包裹只圈住页面原有内容；先前垫在底层的填充留在包裹外。
            let split = list
                .iter()
                .position(|o| stream_tag(doc, o).is_none())
                .unwrap_or(list.len());
            list.insert(split, open);
            list.push(close);
        }
        list.push(mark);
    }

    doc.update_dict(page.obj, |d| d.set(b"Contents".to_vec(), Object::Array(list)))?;
    Ok(())
}

/// 摘掉本工具此前加在该页的填充层，返回移除的层数。
fn strip(doc: &mut Document, page: &Page) -> Result<usize> {
    let list = contents(doc, page)?;
    let mut kept: Vec<Object> = Vec::new();
    let mut removed = 0;
    for item in list {
        if stream_tag(doc, &item).as_deref() == Some(MARK_FILL) {
            removed += 1;
        } else {
            kept.push(item);
        }
    }
    if removed == 0 {
        return Ok(0);
    }
    // 一页上如果已经没有别的填充层，包裹用的 q/Q 也一并去掉。
    if !kept.iter().any(|o| stream_tag(doc, o).as_deref() == Some(MARK_FILL)) {
        kept.retain(|o| stream_tag(doc, o).as_deref() != Some(MARK_WRAP));
    }
    doc.update_dict(page.obj, |d| d.set(b"Contents".to_vec(), Object::Array(kept)))?;
    Ok(removed)
}

/// 页面 `/Contents` 的流引用列表。
///
/// 四种形态都要处理：缺失、间接引用指向数组、数组、单个流（提升为间接对象）。
fn contents(doc: &mut Document, page: &Page) -> Result<Vec<Object>> {
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

/// 带标记的流字典。
fn tagged(tag: &str) -> Dict {
    let mut dict = Dict::new();
    dict.set(MARK_KEY.to_vec(), Object::name(tag));
    dict
}

/// 读一个对象（流）字典里的标记，不解压内容。
fn stream_tag(doc: &Document, object: &Object) -> Option<String> {
    doc.resolve(object)
        .as_dict()?
        .name(MARK_KEY)
        .map(|name| String::from_utf8_lossy(name).into_owned())
}

/// 生成填充用的内容流。首字节是换行，避免与上一段流末尾的记号粘连。
fn fill_content(state_name: &str, color: [f64; 3], rect: [f64; 4]) -> Vec<u8> {
    let mut out = String::new();
    out.push('\n');
    out.push_str("% pdfbatch fill\n");
    out.push_str("q\n");
    out.push_str(&format!("/{state_name} gs\n"));
    out.push_str(&format!("{} {} {} rg\n", number(color[0]), number(color[1]), number(color[2])));
    out.push_str(&format!(
        "{} {} {} {} re f\n",
        number(rect[0]),
        number(rect[1]),
        number(rect[2]),
        number(rect[3])
    ));
    out.push_str("Q\n");
    out.into_bytes()
}

/// PDF 里数字的紧凑写法：最多四位小数，去掉多余的零。
fn number(value: f64) -> String {
    let text = format!("{value:.4}");
    let trimmed = text.trim_end_matches('0').trim_end_matches('.');
    if trimmed.is_empty() || trimmed == "-" {
        "0".to_string()
    } else {
        trimmed.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_are_compact() {
        assert_eq!(number(0.0), "0");
        assert_eq!(number(1.0), "1");
        assert_eq!(number(0.25), "0.25");
        assert_eq!(number(-12.5), "-12.5");
        assert_eq!(number(0.123456), "0.1235");
    }

    #[test]
    fn content_stream_is_well_formed() {
        let content = String::from_utf8(fill_content("PBFillMultiply25", [1.0, 0.95, 0.69], [10.0, 20.0, 100.0, 50.0]))
            .expect("内容流是 UTF-8");
        assert!(content.starts_with('\n'), "首字节必须是换行");
        assert!(content.contains("/PBFillMultiply25 gs"), "{content}");
        assert!(content.contains("1 0.95 0.69 rg"), "{content}");
        assert!(content.contains("10 20 100 50 re f"), "{content}");
        let lines: Vec<&str> = content.lines().collect();
        assert_eq!(lines[1], "% pdfbatch fill");
        assert_eq!(*lines.last().expect("有最后一行"), "Q");
    }

    #[test]
    fn layer_names_round_trip() {
        assert_eq!(Layer::parse("top").unwrap(), Layer::Top);
        assert_eq!(Layer::parse("BOTTOM").unwrap(), Layer::Bottom);
        assert_eq!(Layer::parse("上层").unwrap(), Layer::Top);
        assert!(Layer::parse("middle").is_err());
        for layer in [Layer::Top, Layer::Bottom] {
            assert_eq!(Layer::parse(layer.canonical()).unwrap(), layer);
        }
    }
}