//! 扫描件页面适配：让页面尺寸贴合页面上的图像。
//!
//! 思路：图像有像素尺寸（`/Width`、`/Height`），配合用户给的 PPI 就能算出它的
//! **本征物理尺寸**（`像素 ÷ PPI × 72` 点）。页面尺寸改成这个尺寸，整页就"贴合"图像、
//! 四周不留白；内容按同一个比例平移缩放过去。
//!
//! 多张图时（已确认按**方案 A**）：取各图像**绘制区域的并集外框**作为整体边界，
//! 按同一比例折算页面尺寸。**不做像素级合成**——不新建图像对象，图与图之间的空隙
//! 会成为页面内部留白（但页面四周无留白）。

use pdfcraft_cos::{Dict, Document, Object, Stream};
use pdfcraft_model::Page;

use crate::content::page_contents;
use crate::error::{BatchError, Result};
use crate::transform::{self, Matrix};

/// 默认 PPI。
pub const DEFAULT_PPI: f64 = 300.0;

/// 检测严格度。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Strictness {
    /// 严格：页面上必须恰好有一个图像对象。
    Strict,
    /// 宽松：忽略小图 / 水印 / 背景后，恰好有一个图像对象。
    Lenient,
}

impl Strictness {
    pub fn parse(text: &str) -> std::result::Result<Self, String> {
        match text.trim().to_ascii_lowercase().as_str() {
            "strict" | "严格" => Ok(Strictness::Strict),
            "lenient" | "loose" | "宽松" => Ok(Strictness::Lenient),
            _ => Err(format!("未知检测严格度 `{text}`；可用：strict、lenient")),
        }
    }

    pub fn canonical(self) -> &'static str {
        match self {
            Strictness::Strict => "strict",
            Strictness::Lenient => "lenient",
        }
    }
}

/// 图像旋转的处理方式。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImageRotation {
    /// 保持原样，不额外旋转。
    Keep,
    /// 自动：图像像素朝向与页面绘制朝向明显相反时转 90°。
    Auto,
    /// 指定角度（0/90/180/270）。
    Set(i64),
}

impl ImageRotation {
    pub fn parse(text: &str) -> std::result::Result<Self, String> {
        let text = text.trim();
        match text.to_ascii_lowercase().as_str() {
            "keep" | "保持" => Ok(ImageRotation::Keep),
            "auto" | "自动" => Ok(ImageRotation::Auto),
            _ => {
                let degrees: i64 = text.parse().map_err(|_| {
                    format!("未知图像旋转 `{text}`；可用：auto、keep、0、90、180、270")
                })?;
                ImageRotation::from_degrees(degrees)
            }
        }
    }

    pub fn from_degrees(degrees: i64) -> std::result::Result<Self, String> {
        let normalized = degrees.rem_euclid(360);
        if normalized % 90 != 0 {
            return Err(format!("旋转角度 `{degrees}` 必须是 90 的倍数"));
        }
        Ok(ImageRotation::Set(normalized))
    }

    pub fn canonical(self) -> String {
        match self {
            ImageRotation::Keep => "keep".to_string(),
            ImageRotation::Auto => "auto".to_string(),
            ImageRotation::Set(deg) => deg.to_string(),
        }
    }
}

/// 一次扫描件适配的参数。
#[derive(Clone, Copy, Debug)]
pub struct ScanSpec {
    pub ppi: f64,
    pub strictness: Strictness,
    /// 多张图时是否按「图像合集外框」适配（关闭则跳过多图页面）。
    pub merge: bool,
    pub rotation: ImageRotation,
    /// 宽松模式的过滤阈值：图像宽、高都不得小于这个像素数。
    pub min_pixels: u32,
    /// 宽松模式的过滤阈值：绘制面积占页面面积的比例下限。
    pub min_area_ratio: f64,
}

impl Default for ScanSpec {
    fn default() -> Self {
        Self {
            ppi: DEFAULT_PPI,
            strictness: Strictness::Strict,
            merge: false,
            rotation: ImageRotation::Auto,
            min_pixels: 64,
            min_area_ratio: 0.1,
        }
    }
}

/// 一页的适配结果。
#[derive(Clone, Debug)]
pub enum ScanOutcome {
    /// 适配完成：新的页面尺寸（点）与用到的图像数量。
    Done {
        width: f64,
        height: f64,
        images: usize,
    },
    /// 跳过，附原因。
    Skipped(String),
}

/// 对一页执行扫描件适配。
pub fn apply_page(doc: &mut Document, page: &Page, spec: &ScanSpec) -> Result<ScanOutcome> {
    if !(spec.ppi.is_finite() && spec.ppi > 0.0) {
        return Err(BatchError::Arg(format!("PPI 必须是正数，当前是 {}", spec.ppi)));
    }

    let found = find_images(doc, page)?;
    if found.inline {
        return Ok(ScanOutcome::Skipped("页面存在内联图像".to_string()));
    }

    let crop = page.crop(doc);
    let page_area = (crop[2] - crop[0]) * (crop[3] - crop[1]);
    let kept: Vec<FoundImage> = found
        .images
        .into_iter()
        .filter(|image| match spec.strictness {
            Strictness::Strict => true,
            Strictness::Lenient => !image.is_ignorable(spec, page_area),
        })
        .collect();

    if kept.is_empty() {
        return Ok(ScanOutcome::Skipped("页面无图像".to_string()));
    }
    if kept.len() > 1 && !spec.merge {
        return Ok(ScanOutcome::Skipped("非单图像页面".to_string()));
    }

    // 合集外框（单图时就是它自己的绘制区域）。
    let bbox = union_rects(&kept).ok_or_else(|| BatchError::Arg("图像绘制区域异常".to_string()))?;
    let (bbox_w, bbox_h) = (bbox[2] - bbox[0], bbox[3] - bbox[1]);
    if !(bbox_w > 0.0 && bbox_h > 0.0) {
        return Ok(ScanOutcome::Skipped("图像绘制区域为空".to_string()));
    }

    // 比例：图像本征物理尺寸 ÷ 它在页面上的绘制尺寸（各图应一致，取第一张）。
    let first = &kept[0];
    let kx = (f64::from(first.pixels.0) / spec.ppi * 72.0) / (first.rect[2] - first.rect[0]);
    let ky = (f64::from(first.pixels.1) / spec.ppi * 72.0) / (first.rect[3] - first.rect[1]);
    if !(kx.is_finite() && ky.is_finite()) || kx <= 0.0 || ky <= 0.0 {
        return Ok(ScanOutcome::Skipped("图像比例异常".to_string()));
    }

    let width = bbox_w * kx;
    let height = bbox_h * ky;

    // 内容变换：把外框左下角移到原点、按比例缩放，再按需要旋转回正。
    let degrees = resolve_rotation(spec.rotation, &kept);
    let matrix = Matrix::translate(-bbox[0], -bbox[1])
        .then(Matrix::scale(kx, ky))
        .then(rotate_fit(degrees, width, height));
    let user_size = if degrees % 180 == 0 {
        (width, height)
    } else {
        (height, width)
    };
    transform::apply_transform(doc, page, matrix, user_size, true)?;

    Ok(ScanOutcome::Done {
        width: user_size.0,
        height: user_size.1,
        images: kept.len(),
    })
}

/// 页面上找到的一张图像。
#[derive(Clone, Copy, Debug)]
struct FoundImage {
    /// 像素尺寸 `(宽, 高)`。
    pixels: (u32, u32),
    /// 在页面用户空间里的绘制区域 `[x0, y0, x1, y1]`。
    rect: [f64; 4],
    /// 是否带 `/SMask`（只记录，不影响尺寸）。
    #[allow(dead_code)]
    smask: bool,
}

impl FoundImage {
    /// 宽松模式下是否应当忽略（小图 / 水印 / 背景）。
    fn is_ignorable(&self, spec: &ScanSpec, page_area: f64) -> bool {
        if self.pixels.0 < spec.min_pixels || self.pixels.1 < spec.min_pixels {
            return true;
        }
        let area = (self.rect[2] - self.rect[0]) * (self.rect[3] - self.rect[1]);
        page_area > 0.0 && area / page_area < spec.min_area_ratio
    }
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
                Some(b"Image") => record_image(stream, current, out),
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

/// 记录一张图像：像素尺寸与它在当前 CTM 下的绘制区域。
fn record_image(stream: &Stream, ctm: Matrix, out: &mut Found) {
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
        smask: stream.dict.contains(b"SMask"),
    });
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

/// 若干绘制区域的并集外框。
fn union_rects(images: &[FoundImage]) -> Option<[f64; 4]> {
    let first = images.first()?;
    let mut out = first.rect;
    for image in &images[1..] {
        out[0] = out[0].min(image.rect[0]);
        out[1] = out[1].min(image.rect[1]);
        out[2] = out[2].max(image.rect[2]);
        out[3] = out[3].max(image.rect[3]);
    }
    Some(out)
}

/// 决定实际要旋转的角度。
///
/// 「自动」只看**图像自身**：像素朝向与它的绘制区域朝向相反时才转 90°。
/// 多张图的外框朝向可能和单张图不同（并排的几张图外框是横向的），
/// 拿外框去判会把不该转的页面转掉，所以多图时不自动旋转。
fn resolve_rotation(rotation: ImageRotation, images: &[FoundImage]) -> i64 {
    match rotation {
        ImageRotation::Keep => 0,
        ImageRotation::Set(deg) => deg.rem_euclid(360),
        ImageRotation::Auto => {
            if images.len() != 1 {
                return 0;
            }
            let image = &images[0];
            let image_landscape = image.pixels.0 > image.pixels.1;
            let rect_landscape = (image.rect[2] - image.rect[0]) > (image.rect[3] - image.rect[1]);
            if image_landscape != rect_landscape {
                90
            } else {
                0
            }
        }
    }
}

/// 「旋转 + 平移回正」：把 `w×h` 的内容转 `degrees` 后仍落在第一象限。
fn rotate_fit(degrees: i64, w: f64, h: f64) -> Matrix {
    match degrees.rem_euclid(360) {
        90 => Matrix([0.0, 1.0, -1.0, 0.0, 0.0, 0.0]).then(Matrix::translate(h, 0.0)),
        180 => Matrix([-1.0, 0.0, 0.0, -1.0, 0.0, 0.0]).then(Matrix::translate(w, h)),
        270 => Matrix([0.0, -1.0, 1.0, 0.0, 0.0, 0.0]).then(Matrix::translate(0.0, w)),
        _ => Matrix::IDENTITY,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pdfcraft_cos::Object;
    use pdfcraft_model::pages;

    /// 一页内容：依次用 `w 0 0 h x y cm /ImN Do` 放若干张图。
    ///
    /// 每项为（像素宽, 像素高, 绘制宽, 绘制高, 绘制 x, 绘制 y）。
    fn document_with_images(images: &[(i64, i64, f64, f64, f64, f64)]) -> Document {
        let mut doc = Document::new_empty();
        let pages_ref = doc.add(Dict::new());
        let page = doc.add(Dict::new());
        let root = doc.root().expect("new_empty 有根节点");
        doc.update_dict(root, |d| {
            d.set(b"Type".to_vec(), Object::name("Catalog"));
            d.set(b"Pages".to_vec(), Object::Ref(pages_ref));
        })
        .unwrap();
        doc.update_dict(pages_ref, |d| {
            d.set(b"Type".to_vec(), Object::name("Pages"));
            d.set(b"Kids".to_vec(), Object::Array(vec![Object::Ref(page)]));
            d.set(b"Count".to_vec(), Object::Int(1));
        })
        .unwrap();

        let mut xobjects = Dict::new();
        let mut content = String::new();
        for (index, (px_w, px_h, dw, dh, dx, dy)) in images.iter().enumerate() {
            let mut image = Dict::new();
            image.set(b"Type".to_vec(), Object::name("XObject"));
            image.set(b"Subtype".to_vec(), Object::name("Image"));
            image.set(b"Width".to_vec(), Object::Int(*px_w));
            image.set(b"Height".to_vec(), Object::Int(*px_h));
            let image_ref = doc.add(Object::Stream(Stream::from_raw(image, vec![0u8; 4])));
            let name = format!("Im{index}");
            xobjects.set(name.clone().into_bytes(), Object::Ref(image_ref));
            content.push_str(&format!("q\n{dw} 0 0 {dh} {dx} {dy} cm\n/{name} Do\nQ\n"));
        }

        let content_ref = doc.add(Object::Stream(Stream::from_raw(Dict::new(), content.into_bytes())));
        let mut resources = Dict::new();
        resources.set(b"XObject".to_vec(), Object::Dict(xobjects));

        doc.update_dict(page, |d| {
            d.set(b"Type".to_vec(), Object::name("Page"));
            d.set(b"Parent".to_vec(), Object::Ref(pages_ref));
            d.set(
                b"MediaBox".to_vec(),
                Object::Array(vec![Object::Int(0), Object::Int(0), Object::Int(600), Object::Int(800)]),
            );
            d.set(b"Resources".to_vec(), Object::Dict(resources));
            d.set(b"Contents".to_vec(), Object::Ref(content_ref));
        })
        .unwrap();
        doc
    }

    /// 单图页面的便捷写法。
    fn document_with_image(px_w: i64, px_h: i64, draw: (f64, f64, f64, f64)) -> Document {
        document_with_images(&[(px_w, px_h, draw.0, draw.1, draw.2, draw.3)])
    }

    fn the_page(doc: &Document) -> Page {
        pages(doc)[0].clone()
    }

    #[test]
    fn a_full_page_scan_gets_the_size_its_pixels_imply() {
        // 300 PPI 下 2550×3300 像素 = 8.5×11 英寸 = 612×792 点。
        let mut doc = document_with_image(2550, 3300, (2550.0, 3300.0, 0.0, 0.0));
        let page = the_page(&doc);
        let outcome = apply_page(&mut doc, &page, &ScanSpec::default()).unwrap();
        match outcome {
            ScanOutcome::Done { width, height, images } => {
                assert_eq!(images, 1);
                assert!((width - 612.0).abs() < 0.01, "{width}");
                assert!((height - 792.0).abs() < 0.01, "{height}");
            }
            other => panic!("应当适配成功：{other:?}"),
        }
        let page = the_page(&doc);
        assert_eq!(page.display_size(&doc), (612.0, 792.0));
    }

    #[test]
    fn ppi_changes_the_resulting_size() {
        // 同一张图，PPI 减半 → 物理尺寸翻倍。
        let mut doc = document_with_image(2550, 3300, (2550.0, 3300.0, 0.0, 0.0));
        let page = the_page(&doc);
        let spec = ScanSpec { ppi: 150.0, ..ScanSpec::default() };
        match apply_page(&mut doc, &page, &spec).unwrap() {
            ScanOutcome::Done { width, height, .. } => {
                assert!((width - 1224.0).abs() < 0.01, "{width}");
                assert!((height - 1584.0).abs() < 0.01, "{height}");
            }
            other => panic!("应当适配成功：{other:?}"),
        }
    }

    #[test]
    fn an_offset_image_is_shrunk_onto_the_page() {
        // 图像只占页面中间一小块：适配后页面贴合它，不再有留白。
        let mut doc = document_with_image(300, 300, (300.0, 300.0, 100.0, 200.0));
        let page = the_page(&doc);
        match apply_page(&mut doc, &page, &ScanSpec::default()).unwrap() {
            ScanOutcome::Done { width, height, .. } => {
                // 300 像素 @300PPI = 72 点。
                assert!((width - 72.0).abs() < 0.01, "{width}");
                assert!((height - 72.0).abs() < 0.01, "{height}");
            }
            other => panic!("应当适配成功：{other:?}"),
        }
        let page = the_page(&doc);
        assert_eq!(page.crop(&doc), [0.0, 0.0, 72.0, 72.0]);
    }

    #[test]
    fn multiple_images_need_the_merge_switch() {
        // 两张 300×300 的图并排：绘制区域 300×300 与 300×300（第二张在 x=300）。
        let images = [
            (300, 300, 300.0, 300.0, 0.0, 0.0),
            (300, 300, 300.0, 300.0, 300.0, 0.0),
        ];

        // 未开「按图像合集适配」：多图页面跳过。
        let mut doc = document_with_images(&images);
        let page = the_page(&doc);
        match apply_page(&mut doc, &page, &ScanSpec::default()).unwrap() {
            ScanOutcome::Skipped(why) => assert!(why.contains("非单图像"), "{why}"),
            other => panic!("应当跳过：{other:?}"),
        }

        // 开了合集：按外框适配。外框 600×300，300px@300PPI = 72pt、绘制 300pt → k = 0.24。
        let mut doc = document_with_images(&images);
        let page = the_page(&doc);
        let spec = ScanSpec { merge: true, ..ScanSpec::default() };
        match apply_page(&mut doc, &page, &spec).unwrap() {
            ScanOutcome::Done { width, height, images } => {
                assert_eq!(images, 2);
                assert!((width - 144.0).abs() < 0.01, "{width}");
                assert!((height - 72.0).abs() < 0.01, "{height}");
            }
            other => panic!("应当适配成功：{other:?}"),
        }
    }

    #[test]
    fn a_page_without_images_is_skipped() {
        let mut doc = Document::new_empty();
        let pages_ref = doc.add(Dict::new());
        let page = doc.add(Dict::new());
        let root = doc.root().unwrap();
        doc.update_dict(root, |d| {
            d.set(b"Type".to_vec(), Object::name("Catalog"));
            d.set(b"Pages".to_vec(), Object::Ref(pages_ref));
        })
        .unwrap();
        doc.update_dict(pages_ref, |d| {
            d.set(b"Type".to_vec(), Object::name("Pages"));
            d.set(b"Kids".to_vec(), Object::Array(vec![Object::Ref(page)]));
            d.set(b"Count".to_vec(), Object::Int(1));
        })
        .unwrap();
        doc.update_dict(page, |d| {
            d.set(b"Type".to_vec(), Object::name("Page"));
            d.set(b"Parent".to_vec(), Object::Ref(pages_ref));
            d.set(
                b"MediaBox".to_vec(),
                Object::Array(vec![Object::Int(0), Object::Int(0), Object::Int(200), Object::Int(300)]),
            );
        })
        .unwrap();
        let page = the_page(&doc);
        match apply_page(&mut doc, &page, &ScanSpec::default()).unwrap() {
            ScanOutcome::Skipped(why) => assert!(why.contains("无图像"), "{why}"),
            other => panic!("应当跳过：{other:?}"),
        }
    }

    #[test]
    fn an_inline_image_is_reported() {
        let mut doc = document_with_image(300, 300, (300.0, 300.0, 0.0, 0.0));
        let page = the_page(&doc);
        // 往内容流里塞一段内联图像。
        let content = doc.add(Object::Stream(Stream::from_raw(
            Dict::new(),
            b"BI /W 4 /H 4 /BPC 8 /CS /G ID\x00\x01\x02\x03 EI\n".to_vec(),
        )));
        let list = page_contents(&mut doc, &page).unwrap();
        let mut list = list;
        list.push(Object::Ref(content));
        crate::content::set_page_contents(&mut doc, &page, list).unwrap();

        let page = the_page(&doc);
        match apply_page(&mut doc, &page, &ScanSpec::default()).unwrap() {
            ScanOutcome::Skipped(why) => assert!(why.contains("内联"), "{why}"),
            other => panic!("应当跳过：{other:?}"),
        }
    }

    #[test]
    fn lenient_mode_ignores_small_images() {
        // 一张小图（16×16，远低于阈值）在宽松模式下被忽略 → 页面无图像可适配。
        let mut doc = document_with_image(16, 16, (16.0, 16.0, 0.0, 0.0));
        let page = the_page(&doc);
        let spec = ScanSpec { strictness: Strictness::Lenient, ..ScanSpec::default() };
        match apply_page(&mut doc, &page, &spec).unwrap() {
            ScanOutcome::Skipped(why) => assert!(why.contains("无图像"), "{why}"),
            other => panic!("应当跳过：{other:?}"),
        }
        // 严格模式下它仍然算一张图。
        let mut doc = document_with_image(16, 16, (16.0, 16.0, 0.0, 0.0));
        let page = the_page(&doc);
        assert!(matches!(
            apply_page(&mut doc, &page, &ScanSpec::default()).unwrap(),
            ScanOutcome::Done { .. }
        ));
    }

    #[test]
    fn a_bad_ppi_is_rejected() {
        let mut doc = document_with_image(300, 300, (300.0, 300.0, 0.0, 0.0));
        let page = the_page(&doc);
        let spec = ScanSpec { ppi: 0.0, ..ScanSpec::default() };
        assert!(apply_page(&mut doc, &page, &spec).is_err());
    }

    #[test]
    fn rotation_helpers_are_consistent() {
        // 转 90° 后 100×50 的内容落在 [0,50]×[0,100]。
        let m = rotate_fit(90, 100.0, 50.0);
        let (x, y) = m.apply(0.0, 0.0);
        assert!((x - 50.0).abs() < 1e-9 && y.abs() < 1e-9, "{x},{y}");
        assert!(rotate_fit(0, 10.0, 10.0).is_identity());
    }

    #[test]
    fn values_and_parsing() {
        assert_eq!(Strictness::parse("宽松").unwrap(), Strictness::Lenient);
        assert!(Strictness::parse("whatever").is_err());
        assert_eq!(ImageRotation::parse("auto").unwrap(), ImageRotation::Auto);
        assert_eq!(ImageRotation::from_degrees(450).unwrap(), ImageRotation::Set(90));
        assert!(ImageRotation::parse("45").is_err());
        assert!(union_rects(&[]).is_none());
    }

}
