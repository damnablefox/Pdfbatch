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

/// 允许宽度向下留的安全余量。
///
/// 内容流里的 `cm` 矩阵是按有限小数序列化的，缩放系数会有极小的舍入；
/// 留一点余量才能保证**任何舍入方向下实际 PPI 都不低于**阈值——
/// 设计上承诺的是"只允许大于，绝不允许小于"。
const SAFETY_MARGIN: f64 = 1e-4;

/// 处理完之后重新测量：该页所有图像的**最小实际 PPI**。
///
/// 变换层已经插在内容流里，所以这里枚举到的绘制长度就是缩放后的结果，
/// 直接按 `像素 ÷ 绘制长度 × 72` 取每个方向的值，再取全页最小值。
/// 这个值用于日志自查：它应当不低于对应类型的阈值。
pub fn measure_min_ppi(doc: &mut Document, page: &Page) -> Result<Option<f64>> {
    let found = find_images(doc, page)?;
    let mut best: Option<f64> = None;
    for image in &found.images {
        if !(image.draw.0 > 0.0 && image.draw.1 > 0.0) {
            continue;
        }
        let horizontal = image.pixels.0 * 72.0 / image.draw.0;
        let vertical = image.pixels.1 * 72.0 / image.draw.1;
        let ppi = horizontal.min(vertical);
        best = Some(best.map_or(ppi, |value: f64| value.min(ppi)));
    }
    Ok(best)
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
    let (draw_width, draw_height) = image.draw;
    if !(draw_width > 0.0 && draw_height > 0.0) {
        return None;
    }
    let by_width = image.pixels.0 * 72.0 / (threshold * draw_width);
    let by_height = image.pixels.1 * 72.0 / (threshold * draw_height);
    let width = display_width * by_width.min(by_height) * (1.0 - SAFETY_MARGIN);
    (width.is_finite() && width > 0.0).then_some(width)
}

/// 页面上找到的一张图像。
#[derive(Clone, Copy, Debug)]
struct FoundImage {
    /// 像素尺寸 `(宽, 高)`；保留小数（PDF 允许 `/Width` 是 Real）。
    pixels: (f64, f64),
    /// 图像**自身的两轴**在页面上的实际绘制长度（点）。
    ///
    /// 用 CTM 两个列向量的模长（宽轴 `√(a²+b²)`、高轴 `√(c²+d²)`），
    /// 而不是绘制区域的包围盒：图像被旋转 90° 绘制时，像素宽那一边会落到
    /// 包围盒的**高**上，按包围盒配对会把允许宽度算大、实际 PPI 掉到阈值以下。
    draw: (f64, f64),
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
    // 图像绘制时占单位正方形：两条边经 CTM 后就是两个列向量，模长即实际绘制长度。
    let [a, b, c, d, _, _] = ctm.0;
    let draw = ((a * a + b * b).sqrt(), (c * c + d * d).sqrt());
    if !(draw.0 > 0.0 && draw.1 > 0.0) {
        return;
    }
    out.images.push(FoundImage {
        pixels: (width, height),
        draw,
        kind: kind_of(doc, stream),
    });
}

/// 判定图像类型：位图（1 位黑白）→ 灰度 → 彩色。
///
/// **「位图」不只是 `/ImageMask`**：扫描仪输出的黑白件通常是
/// `BitsPerComponent = 1` 的普通图像（`DeviceGray` 或 `Indexed` 两色），
/// 它们同样只有黑白两值，必须按位图的 PPI 处理——否则那档更高的阈值永远不会生效。
fn kind_of(doc: &Document, stream: &Stream) -> ImageKind {
    if matches!(stream.dict.get(b"ImageMask"), Some(Object::Bool(true))) {
        return ImageKind::Mono;
    }
    // 每分量 1 位：只有黑白两值，按位图算。
    if stream.dict.get(b"BitsPerComponent").and_then(Object::as_f64) == Some(1.0) {
        return ImageKind::Mono;
    }
    if color_space_is_gray(doc, stream) {
        return ImageKind::Gray;
    }
    ImageKind::Color
}

/// 色彩空间是否等价于灰度：`DeviceGray`，或 `/Indexed` 的基色空间是灰度。
fn color_space_is_gray(doc: &Document, stream: &Stream) -> bool {
    let Some(object) = stream.dict.get(b"ColorSpace") else {
        return false;
    };
    let is_gray_name = |name: &[u8]| name == b"DeviceGray" || name == b"G";
    match doc.resolve(object).as_ref() {
        Object::Name(name) => is_gray_name(name),
        Object::Array(items) => {
            // [/Indexed 基色空间 hival lookup ...]：看基色空间是不是灰度。
            let family = items
                .first()
                .map(|item| doc.resolve(item))
                .and_then(|item| item.as_name().map(<[u8]>::to_vec));
            match family.as_deref() {
                Some(b"Indexed") | Some(b"I") => items
                    .get(1)
                    .map(|base| doc.resolve(base))
                    .and_then(|base| base.as_name().map(<[u8]>::to_vec))
                    .map(|name| is_gray_name(&name))
                    .unwrap_or(false),
                _ => false,
            }
        }
        _ => false,
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
                    ImageKindSpec::OneBitGray => {
                        dict.set(b"ColorSpace".to_vec(), Object::name("DeviceGray"));
                        dict.set(b"BitsPerComponent".to_vec(), Object::Int(1));
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
        /// 1 位黑白、但不是 `/ImageMask`（扫描件最常见的形态）。
        OneBitGray,
    }

    /// 一页 200×300，其中一张 1559×2368 的位图被**旋转 90°**绘制：
    /// 图像自身的宽轴（1559 px 那一边）沿页面竖直方向、长 300pt，
    /// 高轴（2368 px）沿水平方向、长 200pt。
    fn rotated_document() -> Document {
        let mut doc = Document::new_empty();
        let pages_ref = doc.add(Dict::new());
        let root = doc.root().expect("new_empty 有根节点");
        doc.update_dict(root, |d| {
            d.set(b"Type".to_vec(), Object::name("Catalog"));
            d.set(b"Pages".to_vec(), Object::Ref(pages_ref));
        })
        .unwrap();

        let page = doc.add(Dict::new());
        let mut dict = Dict::new();
        dict.set(b"Type".to_vec(), Object::name("XObject"));
        dict.set(b"Subtype".to_vec(), Object::name("Image"));
        dict.set(b"Width".to_vec(), Object::Int(1559));
        dict.set(b"Height".to_vec(), Object::Int(2368));
        dict.set(b"BitsPerComponent".to_vec(), Object::Int(1));
        dict.set(b"ColorSpace".to_vec(), Object::name("DeviceGray"));
        let image = doc.add(Object::Stream(Stream::from_raw(dict, Vec::new())));
        let mut xobjects = Dict::new();
        xobjects.set(b"Im0".to_vec(), Object::Ref(image));
        let mut resources = Dict::new();
        resources.set(b"XObject".to_vec(), Object::Dict(xobjects));
        // `0 300 -200 0 200 0 cm`：图像 x 轴 → 页面 +y（长 300），y 轴 → 页面 -x（长 200）。
        let content = doc.add(Object::Stream(Stream::from_raw(
            Dict::new(),
            b"q 0 300 -200 0 200 0 cm /Im0 Do Q\n".to_vec(),
        )));
        doc.update_dict(page, |d| {
            d.set(b"Type".to_vec(), Object::name("Page"));
            d.set(b"Parent".to_vec(), Object::Ref(pages_ref));
            d.set(
                b"MediaBox".to_vec(),
                Object::Array(vec![Object::Int(0), Object::Int(0), Object::Int(200), Object::Int(300)]),
            );
            d.set(b"Resources".to_vec(), Object::Dict(resources));
            d.set(b"Contents".to_vec(), Object::Ref(content));
        })
        .unwrap();
        doc.update_dict(pages_ref, |d| {
            d.set(b"Type".to_vec(), Object::name("Pages"));
            d.set(b"Kids".to_vec(), Object::Array(vec![Object::Ref(page)]));
            d.set(b"Count".to_vec(), Object::Int(1));
        })
        .unwrap();
        doc
    }

    /// 旋转绘制的图像必须按「图像自身的轴」配对像素，不能按包围盒：
    /// 否则允许宽度会算大，实际 PPI 掉到阈值以下。
    #[test]
    fn rotated_images_pair_the_right_axis() {
        let mut doc = rotated_document();
        let width = probe(&mut doc, &ScanSpec::default()).unwrap().width.unwrap();
        // 宽轴 300pt：1559×72/(600×300) = 0.6236；高轴 200pt：2368×72/(600×200) = 1.4208。
        // 取小者 0.6236，页面宽 200 → 允许 124.72，再乘 1e-4 的余量 → 124.7075。
        assert!((width - 124.71).abs() < 0.05, "允许宽度应当是 124.71：{width}");
    }

    /// 端到端：按探测出的统一宽度处理完之后，实测的最小 PPI 必须不低于阈值。
    #[test]
    fn measured_ppi_after_apply_is_not_below_the_threshold() {
        let pages = [
            (vec![(3000u32, 3000u32, ImageKindSpec::Color)], (720.0, 720.0)),
            (vec![(1200u32, 1200u32, ImageKindSpec::Mono)], (144.0, 144.0)),
        ];
        let mut doc = document(&pages);
        let spec = ScanSpec::default();
        let width = probe(&mut doc, &spec).unwrap().width.unwrap();

        let scale = crate::pagesize::ScaleSpec::new(
            crate::pagesize::Target::custom(width, 0.0),
            crate::pagesize::ScaleMode::FitWidth,
        );
        // 彩色页要求 ≥300、位图页要求 ≥600；统一宽度由位图那一页决定。
        for (index, threshold) in [(0usize, 300.0), (1usize, 600.0)] {
            let page = pdfcraft_model::pages(&doc)[index].clone();
            crate::pagesize::apply_page(&mut doc, &page, &scale, true).unwrap();
            let ppi = measure_min_ppi(&mut doc, &page).unwrap().unwrap();
            assert!(ppi >= threshold, "第 {} 页实测 {ppi} 应当不小于 {threshold}", index + 1);
        }
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
        assert!((width - 187.08).abs() < 0.05, "全局宽度应当是位图那一页的允许宽度：{width}");
    }

    /// 位图的默认阈值（600）比彩色/灰度（300）更严：同样的图，允许宽度只有一半。
    #[test]
    fn mono_threshold_is_stricter_than_color() {
        let pages = [(vec![(1200u32, 1200u32, ImageKindSpec::Mono)], (144.0, 144.0))];
        let mut doc = document(&pages);
        // 1200 像素铺在 144pt 上正好是 600 PPI，不能再放大。
        let strict = probe(&mut doc, &ScanSpec::default()).unwrap().width.unwrap();
        assert!((strict - 144.0).abs() < 0.05, "{strict}");
        // 放宽到 300 后允许放大一倍宽度。
        let spec = ScanSpec {
            mono_ppi: 300.0,
            ..ScanSpec::default()
        };
        let relaxed = probe(&mut doc, &spec).unwrap().width.unwrap();
        assert!((relaxed - 288.0).abs() < 0.05, "{relaxed}");
    }

    #[test]
    fn pixel_count_raises_the_allowed_width() {
        // 同样的阈值下，像素翻倍 → 允许宽度也翻倍。
        let pages = [(vec![(2400u32, 2400u32, ImageKindSpec::Mono)], (144.0, 144.0))];
        let mut doc = document(&pages);
        let width = probe(&mut doc, &ScanSpec::default()).unwrap().width.unwrap();
        assert!((width - 288.0).abs() < 0.05, "{width}");
    }

    /// 位图判定：1 位黑白即使不是 `/ImageMask`，也要按位图的高阈值处理。
    #[test]
    fn one_bit_images_use_the_mono_threshold() {
        // 1200×1200 铺在 144pt：按位图阈值 600 只允许 144，按灰度阈值 300 会放到 288。
        let pages = [(vec![(1200u32, 1200u32, ImageKindSpec::OneBitGray)], (144.0, 144.0))];
        let mut doc = document(&pages);
        let width = probe(&mut doc, &ScanSpec::default()).unwrap().width.unwrap();
        assert!(width < 144.0 && width > 143.9, "1 位黑白应当走位图阈值：{width}");
    }

    /// 允许宽度会向下留安全余量：实际 PPI 不会因内容流数字的序列化舍入掉到阈值之下。
    #[test]
    fn allowed_width_keeps_a_safety_margin() {
        let pages = [(vec![(1200u32, 1200u32, ImageKindSpec::Mono)], (144.0, 144.0))];
        let mut doc = document(&pages);
        let width = probe(&mut doc, &ScanSpec::default()).unwrap().width.unwrap();
        let expected = 144.0 * (1.0 - SAFETY_MARGIN);
        assert!((width - expected).abs() < 1e-6, "{width} 与 {expected}");
    }

    #[test]
    fn gray_uses_its_own_threshold() {
        let pages = [(vec![(1200u32, 1200u32, ImageKindSpec::Gray)], (144.0, 144.0))];
        let mut doc = document(&pages);
        let mut spec = ScanSpec::default();
        let default_width = probe(&mut doc, &spec).unwrap().width.unwrap();
        assert!((default_width - 288.0).abs() < 0.05, "灰度默认 300：{default_width}");
        spec.gray_ppi = 600.0;
        let strict = probe(&mut doc, &spec).unwrap().width.unwrap();
        assert!((strict - 144.0).abs() < 0.05, "灰度提到 600：{strict}");
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
        assert!((width - 120.0).abs() < 0.05, "{width}");
    }
}
