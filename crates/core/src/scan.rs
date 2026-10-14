//! 扫描件页面适配：由图像像素与 PPI 决定**整个文件统一的页面宽度**。
//!
//! 2026-10-15 的重设计要点：
//!
//! - 彩色 / 灰度 / 位图三种图像**各给一个 PPI**（位图默认更高，避免把线稿放大糊掉）。
//! - 改成两阶段：先扫描全文件，算出"让每一页的每张图像都不低于各自 PPI 所允许的最大
//!   页面宽度"，再取**全局最小值**；最后对每一页应用「适应宽度」。
//! - 由于"允许宽度"的定义就是"此时实际 PPI 恰好等于阈值"，取全局最小值后每一页的
//!   实际 PPI **只可能不小于**设定值。
//! - 图像旋转、检测严格度、多图合集外框都已删除（旋转参数整体删除；多图由本算法自然覆盖）。

use pdfcraft_cos::{Dict, Document, Object, Stream};
use pdfcraft_model::Page;

use crate::content::page_contents;
use crate::error::{BatchError, Result};
use crate::transform::Matrix;

/// 默认 PPI：彩色、灰度、位图。
pub const DEFAULT_COLOR_PPI: f64 = 300.0;
pub const DEFAULT_GRAY_PPI: f64 = 300.0;
pub const DEFAULT_MONO_PPI: f64 = 600.0;

/// 图像类型：决定用哪个 PPI 阈值。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImageKind {
    /// 彩色（`DeviceRGB` / `DeviceCMYK` / ICC 等）。
    Color,
    /// 灰度（`DeviceGray`）。
    Gray,
    /// 位图 / 图像蒙版（`/ImageMask true`，通常 1 位黑白）。
    Mono,
}

impl ImageKind {
    pub fn label(self) -> &'static str {
        match self {
            ImageKind::Color => "彩色图像",
            ImageKind::Gray => "灰度图像",
            ImageKind::Mono => "位图图像",
        }
    }
}

/// 扫描件适配的参数：三种图像各自的 PPI。
#[derive(Clone, Copy, Debug)]
pub struct ScanSpec {
    pub color_ppi: f64,
    pub gray_ppi: f64,
    pub mono_ppi: f64,
}

impl Default for ScanSpec {
    fn default() -> Self {
        Self {
            color_ppi: DEFAULT_COLOR_PPI,
            gray_ppi: DEFAULT_GRAY_PPI,
            mono_ppi: DEFAULT_MONO_PPI,
        }
    }
}

impl ScanSpec {
    pub fn threshold(&self, kind: ImageKind) -> f64 {
        match kind {
            ImageKind::Color => self.color_ppi,
            ImageKind::Gray => self.gray_ppi,
            ImageKind::Mono => self.mono_ppi,
        }
    }

    /// 三个 PPI 都必须为正（用户要求：为 0 时报错）。
    pub fn validate(&self) -> Result<()> {
        for (name, value) in [
            ("彩色图像 PPI", self.color_ppi),
            ("灰度图像 PPI", self.gray_ppi),
            ("位图图像 PPI", self.mono_ppi),
        ] {
            if !(value.is_finite() && value > 0.0) {
                return Err(BatchError::Arg(format!("{name} 必须是正数，当前是 {value}")));
            }
        }
        Ok(())
    }
}

/// 全文件的探测结果。
#[derive(Clone, Debug, Default)]
pub struct Probe {
    /// 全文件统一的页面宽度（点）。`None` 表示这个文件里没有可用的图像。
    pub width: Option<f64>,
    /// 有可用图像的页号（从 0 开始，文档顺序）。
    pub pages_with_images: Vec<usize>,
    /// 含内联图像的页号：内联图像无法可靠折算，这些页面会跳过。
    pub pages_with_inline: Vec<usize>,
}

impl Probe {
    /// 该页是否参与本次适配（有可用图像的页面才参与）。
    pub fn covers(&self, index: usize) -> bool {
        self.pages_with_images.contains(&index)
    }
}

/// 扫描整个文件，得出统一的页面宽度。
pub fn probe(doc: &mut Document, spec: &ScanSpec) -> Result<Probe> {
    spec.validate()?;
    let all = pdfcraft_model::pages(doc);
    let mut out = Probe::default();
    for (index, page) in all.iter().enumerate() {
        let found = find_images(doc, page)?;
        if found.inline {
            out.pages_with_inline.push(index);
            continue;
        }
        if found.images.is_empty() {
            continue;
        }
        let (display_width, _) = page.display_size(doc);
        if !(display_width > 0.0) {
            continue;
        }
        let mut page_best: Option<f64> = None;
        for image in &found.images {
            let Some(allowed) = allowed_width(image, spec, display_width) else {
                continue;
            };
            page_best = Some(page_best.map_or(allowed, |best| best.min(allowed)));
        }
        if let Some(page_best) = page_best {
            out.pages_with_images.push(index);
            out.width = Some(out.width.map_or(page_best, |best| best.min(page_best)));
        }
    }
    Ok(out)
}

/// 一张图像允许的**最大页面宽度**（点）。
///
/// 记阈值为 `P`、像素为 `px`、它在页面用户空间的绘制尺寸为 `r`、等比缩放系数为 `s`，
/// 则实际 PPI = `px / (r · s) · 72`。要求它不低于 `P`，就得到
/// `s ≤ px · 72 / (P · r)`；宽度方向与高度方向各算一次取小者，
/// 再乘上页面的显示宽度，就是这一页允许的最大宽度。
fn allowed_width(image: &FoundImage, spec: &ScanSpec, display_width: f64) -> Option<f64> {
    let threshold = spec.threshold(image.kind);
    if !(threshold.is_finite() && threshold > 0.0) {
        return None;
    }
    let rect_width = image.rect[2] - image.rect[0];
    let rect_height = image.rect[3] - image.rect[1];
    if !(rect_width > 0.0 && rect_height > 0.0) {
        return None;
    }
    let pixels_width = f64::from(image.pixels.0);
    let pixels_height = f64::from(image.pixels.1);
    let by_width = pixels_width * 72.0 / (threshold * rect_width);
    let by_height = pixels_height * 72.0 / (threshold * rect_height);
    let width = display_width * by_width.min(by_height);
    (width.is_finite() && width > 0.0).then_some(width)
}

/// 页面上找到的一张图像。
#[derive(Clone, Copy, Debug)]
struct FoundImage {
    /// 像素尺寸 `(宽, 高)`。
    pixels: (u32, u32),
    /// 在页面用户空间里的绘制区域 `[x0, y0, x1, y1]`。
    rect: [f64; 4],
    /// 图像类型（决定用哪个 PPI）。
    kind: ImageKind,
}

/// 页面上的图像检测结果。
#[derive(Default)]
struct Found {
    images: Vec<FoundImage>,
    /// 是否出现过内联图像（`BI`…`ID`…`EI`）。
    inline: bool,
}

/// 遍历一页的内容流，收集所有图像（含 Form XObject 里的）。
fn find_images(doc: &mut Document, page: &Page) -> Result<Found> {
    let bytes = page_content_bytes(doc, page)?;
    let resources = page
        .dict
        .get(b"Resources")
        .map(|r| doc.resolve(r))
        .and_then(|r| r.as_dict().cloned())
        .unwrap_or_default();
    let mut found = Found::default();
    walk_content(doc, &bytes, &resources, Matrix::IDENTITY, 0, &mut found);
    Ok(found)
}

/// 递归遍历内容流：维护图形状态栈与 CTM，遇到 `Do` 就查 XObject。
fn walk_content(
    doc: &Document,
    data: &[u8],
    resources: &Dict,
    ctm: Matrix,
    depth: usize,
    out: &mut Found,
) {
    // 深度上限防止 Form 互相引用时无限递归。
    if depth > 8 {
        return;
    }
    let parsed = pdfcraft_content::parse(data);
    let mut saved: Vec<Matrix> = Vec::new();
    let mut current = ctm;
    for op in &parsed.ops {
        if op.is("q") {
            saved.push(current);
        } else if op.is("Q") {
            if let Some(restored) = saved.pop() {
                current = restored;
            }
        } else if op.is("cm") {
            if let Some(values) = op.nums::<6>() {
                current = current.then(Matrix(values));
            }
        } else if op.inline.is_some() {
            out.inline = true;
        } else if op.is("Do") {
            let Some(name) = op.name(0) else { continue };
            let Some(object) = lookup_xobject(doc, resources, name) else {
                continue;
            };
            let resolved = doc.resolve(&object);
            let Object::Stream(stream) = resolved.as_ref() else {
                continue;
            };
            match stream.dict.name(b"Subtype") {
                Some(b"Image") => record_image(doc, stream, current, out),
                Some(b"Form") => {
                    let form_ctm = current.then(form_matrix(stream));
                    let sub = form_resources(doc, stream, resources);
                    if let Ok(bytes) = stream.decoded() {
                        walk_content(doc, &bytes, &sub, form_ctm, depth + 1, out);
                    }
                }
                _ => {}
            }
        }
    }
}

/// 记录一张图像：像素尺寸、类型与它在当前 CTM 下的绘制区域。
fn record_image(doc: &Document, stream: &Stream, ctm: Matrix, out: &mut Found) {
    let width = stream.dict.get(b"Width").and_then(Object::as_f64).unwrap_or(0.0);
    let height = stream.dict.get(b"Height").and_then(Object::as_f64).unwrap_or(0.0);
    if !(width >= 1.0 && height >= 1.0) {
        return;
    }
    // 图像绘制时占单位正方形 [0,1]×[0,1]，四个角经 CTM 得到包围盒。
    let corners = [(0.0, 0.0), (1.0, 0.0), (0.0, 1.0), (1.0, 1.0)];
    let points: Vec<(f64, f64)> = corners.iter().map(|(x, y)| ctm.apply(*x, *y)).collect();
    let x0 = points.iter().map(|p| p.0).fold(f64::INFINITY, f64::min);
    let x1 = points.iter().map(|p| p.0).fold(f64::NEG_INFINITY, f64::max);
    let y0 = points.iter().map(|p| p.1).fold(f64::INFINITY, f64::min);
    let y1 = points.iter().map(|p| p.1).fold(f64::NEG_INFINITY, f64::max);
    if !(x1 > x0 && y1 > y0) {
        return;
    }
    out.images.push(FoundImage {
        pixels: (width.round() as u32, height.round() as u32),
        rect: [x0, y0, x1, y1],
        kind: kind_of(doc, stream),
    });
}

/// 判定图像类型：位图（图像蒙版）→ 灰度 → 彩色。
fn kind_of(doc: &Document, stream: &Stream) -> ImageKind {
    let is_mask = matches!(stream.dict.get(b"ImageMask"), Some(Object::Bool(true)));
    if is_mask {
        return ImageKind::Mono;
    }
    let color_space = stream
        .dict
        .get(b"ColorSpace")
        .map(|object| doc.resolve(object))
        .and_then(|object| object.as_name().map(<[u8]>::to_vec));
    match color_space.as_deref() {
        Some(b"DeviceGray") => ImageKind::Gray,
        _ => ImageKind::Color,
    }
}

/// 查页面 Resources 里的 XObject。
fn lookup_xobject(doc: &Document, resources: &Dict, name: &[u8]) -> Option<Object> {
    let xobjects = resources
        .get(b"XObject")
        .map(|o| doc.resolve(o))
        .and_then(|o| o.as_dict().cloned())?;
    xobjects.get(name).cloned()
}

/// Form XObject 的 `/Matrix`（默认单位矩阵）。
fn form_matrix(stream: &Stream) -> Matrix {
    let Some(array) = stream.dict.get(b"Matrix").and_then(Object::as_array) else {
        return Matrix::IDENTITY;
    };
    if array.len() != 6 {
        return Matrix::IDENTITY;
    }
    let mut values = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];
    for (slot, item) in values.iter_mut().zip(array) {
        match item.as_f64() {
            Some(v) => *slot = v,
            None => return Matrix::IDENTITY,
        }
    }
    Matrix(values)
}

/// Form XObject 自己的 Resources；没有就沿用外层。
fn form_resources(doc: &Document, stream: &Stream, outer: &Dict) -> Dict {
    stream
        .dict
        .get(b"Resources")
        .map(|r| doc.resolve(r))
        .and_then(|r| r.as_dict().cloned())
        .unwrap_or_else(|| outer.clone())
}

/// 页面内容流的字节（多段用换行连接，避免两端操作符粘连）。
fn page_content_bytes(doc: &mut Document, page: &Page) -> Result<Vec<u8>> {
    let list = page_contents(doc, page)?;
    let mut out = Vec::new();
    for object in &list {
        let resolved = doc.resolve(object);
        if let Object::Stream(stream) = resolved.as_ref() {
            if let Ok(bytes) = stream.decoded() {
                if !out.is_empty() {
                    out.push(b'\n');
                }
                out.extend_from_slice(&bytes);
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 造一个页面：`draw` 是图像在页面上的绘制尺寸（点），`pixels` 是像素尺寸。
    fn document(pages: &[(Vec<(u32, u32, ImageKindSpec)>, (f64, f64))]) -> Document {
        let mut doc = Document::new_empty();
        let pages_ref = doc.add(Dict::new());
        let root = doc.root().expect("new_empty 有根节点");
        doc.update_dict(root, |d| {
            d.set(b"Type".to_vec(), Object::name("Catalog"));
            d.set(b"Pages".to_vec(), Object::Ref(pages_ref));
        })
        .unwrap();

        let mut kids = Vec::new();
        for (images, (page_w, page_h)) in pages {
            let page = doc.add(Dict::new());
            let mut xobjects = Dict::new();
            let mut content = String::new();
            for (index, (pixels_w, pixels_h, spec)) in images.iter().enumerate() {
                let mut dict = Dict::new();
                dict.set(b"Type".to_vec(), Object::name("XObject"));
                dict.set(b"Subtype".to_vec(), Object::name("Image"));
                dict.set(b"Width".to_vec(), Object::Int(i64::from(*pixels_w)));
                dict.set(b"Height".to_vec(), Object::Int(i64::from(*pixels_h)));
                dict.set(b"BitsPerComponent".to_vec(), Object::Int(8));
                match spec {
                    ImageKindSpec::Color => {
                        dict.set(b"ColorSpace".to_vec(), Object::name("DeviceRGB"));
                    }
                    ImageKindSpec::Gray => {
                        dict.set(b"ColorSpace".to_vec(), Object::name("DeviceGray"));
                    }
                    ImageKindSpec::Mono => {
                        dict.set(b"ImageMask".to_vec(), Object::Bool(true));
                    }
                }
                let reference = doc.add(Object::Stream(Stream::from_raw(dict, Vec::new())));
                xobjects.set(format!("Im{index}").into_bytes(), Object::Ref(reference));
                // 图像绘制时占单位正方形，这里直接缩放成整页大小（绘制区域 = 页面）。
                let sx = *page_w;
                let sy = *page_h;
                content.push_str(&format!("q {sx} 0 0 {sy} 0 0 cm /Im{index} Do Q\n"));
            }
            let mut resources = Dict::new();
            resources.set(b"XObject".to_vec(), Object::Dict(xobjects));
            let content_stream = doc.add(Object::Stream(Stream::from_raw(
                Dict::new(),
                content.into_bytes(),
            )));
            doc.update_dict(page, |d| {
                d.set(b"Type".to_vec(), Object::name("Page"));
                d.set(b"Parent".to_vec(), Object::Ref(pages_ref));
                d.set(
                    b"MediaBox".to_vec(),
                    Object::Array(vec![
                        Object::Int(0),
                        Object::Int(0),
                        Object::Real(*page_w),
                        Object::Real(*page_h),
                    ]),
                );
                d.set(b"Resources".to_vec(), Object::Dict(resources));
                d.set(b"Contents".to_vec(), Object::Ref(content_stream));
            })
            .unwrap();
            kids.push(Object::Ref(page));
        }

        doc.update_dict(pages_ref, |d| {
            d.set(b"Type".to_vec(), Object::name("Pages"));
            d.set(b"Kids".to_vec(), Object::Array(kids));
            d.set(b"Count".to_vec(), Object::Int(pages.len() as i64));
        })
        .unwrap();
        doc
    }

    /// 测试里描述图像类型的简写。
    #[derive(Clone, Copy)]
    enum ImageKindSpec {
        Color,
        Gray,
        Mono,
    }

    /// 用户给的例子：三页分别 1576×2288 彩色、1464×2184 灰度、1559×2368 位图，
    /// 结论应当是宽度 187.08 pt（66.0 mm）。
    #[test]
    fn reproduces_the_reported_example() {
        let doc_pages = [
            (vec![(1576u32, 2288u32, ImageKindSpec::Color)], (378.24, 549.12)),
            (vec![(1464, 2184, ImageKindSpec::Gray)], (351.36, 524.16)),
            (vec![(1559, 2368, ImageKindSpec::Mono)], (187.08, 284.16)),
        ];
        let mut doc = document(&doc_pages);
        let width = probe(&mut doc, &ScanSpec::default()).unwrap().width.unwrap();
        assert!((width - 187.08).abs() < 0.01, "全局宽度应当是位图那一页的允许宽度：{width}");
    }

    /// 位图的默认阈值（600）比彩色/灰度（300）更严：同样的图，允许宽度只有一半。
    #[test]
    fn mono_threshold_is_stricter_than_color() {
        let pages = [(vec![(1200u32, 1200u32, ImageKindSpec::Mono)], (144.0, 144.0))];
        let mut doc = document(&pages);
        // 1200 像素铺在 144pt 上正好是 600 PPI，不能再放大。
        let strict = probe(&mut doc, &ScanSpec::default()).unwrap().width.unwrap();
        assert!((strict - 144.0).abs() < 0.01, "{strict}");
        // 放宽到 300 后允许放大一倍宽度。
        let spec = ScanSpec {
            mono_ppi: 300.0,
            ..ScanSpec::default()
        };
        let relaxed = probe(&mut doc, &spec).unwrap().width.unwrap();
        assert!((relaxed - 288.0).abs() < 0.01, "{relaxed}");
    }

    #[test]
    fn pixel_count_raises_the_allowed_width() {
        // 同样的阈值下，像素翻倍 → 允许宽度也翻倍。
        let pages = [(vec![(2400u32, 2400u32, ImageKindSpec::Mono)], (144.0, 144.0))];
        let mut doc = document(&pages);
        let width = probe(&mut doc, &ScanSpec::default()).unwrap().width.unwrap();
        assert!((width - 288.0).abs() < 0.01, "{width}");
    }

    #[test]
    fn gray_uses_its_own_threshold() {
        let pages = [(vec![(1200u32, 1200u32, ImageKindSpec::Gray)], (144.0, 144.0))];
        let mut doc = document(&pages);
        let mut spec = ScanSpec::default();
        let default_width = probe(&mut doc, &spec).unwrap().width.unwrap();
        assert!((default_width - 288.0).abs() < 0.01, "灰度默认 300：{default_width}");
        spec.gray_ppi = 600.0;
        let strict = probe(&mut doc, &spec).unwrap().width.unwrap();
        assert!((strict - 144.0).abs() < 0.01, "灰度提到 600：{strict}");
    }

    #[test]
    fn pages_without_images_are_reported() {
        let pages = [
            (vec![(600u32, 600u32, ImageKindSpec::Color)], (144.0, 144.0)),
            (vec![], (200.0, 300.0)),
        ];
        let mut doc = document(&pages);
        let found = probe(&mut doc, &ScanSpec::default()).unwrap();
        assert_eq!(found.pages_with_images, vec![0]);
        assert!(found.width.is_some());
    }

    #[test]
    fn a_file_without_images_has_no_width() {
        let pages = [(vec![], (200.0, 300.0))];
        let mut doc = document(&pages);
        let found = probe(&mut doc, &ScanSpec::default()).unwrap();
        assert!(found.width.is_none());
        assert!(found.pages_with_images.is_empty());
    }

    #[test]
    fn zero_ppi_is_rejected() {
        let mut doc = document(&[(vec![], (200.0, 300.0))]);
        let spec = ScanSpec {
            color_ppi: 0.0,
            ..ScanSpec::default()
        };
        assert!(probe(&mut doc, &spec).is_err());
    }

    /// 一张图像的两个方向都会约束：像素宽高比与绘制宽高比不一致时取更严的那个。
    #[test]
    fn the_stricter_axis_wins() {
        // 1000×1000 像素画到 100×200 的区域：横向 720 PPI、纵向 360 PPI，
        // 阈值 300 时纵向更严，允许的缩放系数 = 1000×72/(300×200) = 1.2。
        let pages = [(vec![(1000u32, 1000u32, ImageKindSpec::Color)], (100.0, 200.0))];
        let mut doc = document(&pages);
        let spec = ScanSpec {
            color_ppi: 300.0,
            ..ScanSpec::default()
        };
        let width = probe(&mut doc, &spec).unwrap().width.unwrap();
        assert!((width - 120.0).abs() < 0.01, "{width}");
    }
}
