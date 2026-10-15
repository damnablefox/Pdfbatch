//! 页面尺寸：把内容重新映射到目标页面上。
//!
//! 2026-10-15 的重设计要点：
//!
//! - **适应宽度 / 适应高度会改写输出页面的另一维**：只固定被指定的那一维，
//!   另一维由内容等比缩放后的结果决定，因此"不裁切、不留白"。
//!   它们只在自定义尺寸下可用（由界面与 CLI 约束，本层再校验一次）。
//! - **内容居中**不缩放内容，但会**摆正并居中**（旧版是留在原坐标、贴左下角）。
//! - **旋转参数已删除**：输出一律把 `/Rotate` 归零，原页已有的旋转被烘焙进内容变换，
//!   于是"显示尺寸 = 页面矩形 = 目标尺寸"三者统一。
//!
//! 实现上统一走 [`crate::transform`]：把「原内容 → 新页面」的映射写成一个矩阵，
//! 插到内容流最前；页面尺寸同时改写。

use pdfcraft_cos::{Document, Object};
use pdfcraft_model::{Page, view_matrix_for};

use crate::error::{BatchError, Result};
use crate::transform::{self, Matrix};

/// 纸张方向：决定预设纸张的长边给宽还是给高。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Orientation {
    /// 纵向（长边做高）。
    Portrait,
    /// 横向（长边做宽）。
    Landscape,
    /// 跟随原页面：按**该页的显示朝向**（已含原 `/Rotate`）逐页决定。
    #[default]
    Auto,
}

impl Orientation {
    pub fn parse(text: &str) -> std::result::Result<Self, String> {
        match text.trim().to_ascii_lowercase().as_str() {
            "portrait" | "p" | "纵" | "纵向" => Ok(Orientation::Portrait),
            "landscape" | "l" | "横" | "横向" => Ok(Orientation::Landscape),
            "auto" | "keep" | "自动" | "保持" | "跟随原页面" => Ok(Orientation::Auto),
            _ => Err(format!("未知纸张方向 `{text}`；可用：portrait、landscape、auto")),
        }
    }

    pub fn canonical(self) -> &'static str {
        match self {
            Orientation::Portrait => "portrait",
            Orientation::Landscape => "landscape",
            Orientation::Auto => "auto",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Orientation::Portrait => "纵向",
            Orientation::Landscape => "横向",
            Orientation::Auto => "跟随原页面",
        }
    }
}

/// 目标尺寸的来源。
#[derive(Clone, Copy, Debug)]
pub enum Target {
    /// 预设纸张：给长短边，实际宽高由方向与页面朝向决定。
    Paper {
        long_side: f64,
        short_side: f64,
        orientation: Orientation,
    },
    /// 自定义尺寸：宽度与高度分别给出，任一为 0 表示"这一维不约束"
    /// （此时只有适应宽度 / 适应高度可用）。
    Custom { width: f64, height: f64 },
}

impl Target {
    /// 预设纸张（纵向基准的宽高，点）。
    pub fn paper(width: f64, height: f64, orientation: Orientation) -> Self {
        let (long_side, short_side) = if width >= height { (width, height) } else { (height, width) };
        Target::Paper {
            long_side,
            short_side,
            orientation,
        }
    }

    pub fn custom(width: f64, height: f64) -> Self {
        Target::Custom { width, height }
    }
}

/// 内容如何落到目标页面上。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ScaleMode {
    /// 只固定宽度：等比缩放使宽度贴合，**输出高度由内容决定**。
    FitWidth,
    /// 只固定高度：等比缩放使高度贴合，**输出宽度由内容决定**。
    FitHeight,
    /// 等比缩放，完整显示在页面内（余量方向留白）。
    #[default]
    Fit,
    /// 等比缩放铺满页面（超出方向裁切）。
    Fill,
    /// 非等比拉伸铺满页面（可能变形）。
    Stretch,
    /// 不缩放，内容摆正后居中（可能留白或裁切）。
    Center,
    /// 按给定比例缩放后居中。
    CenterScaled,
}

impl ScaleMode {
    /// 下拉列表顺序（用户指定）。
    pub const ALL: [ScaleMode; 7] = [
        ScaleMode::FitWidth,
        ScaleMode::FitHeight,
        ScaleMode::Fit,
        ScaleMode::Fill,
        ScaleMode::Stretch,
        ScaleMode::Center,
        ScaleMode::CenterScaled,
    ];

    pub fn label(self) -> &'static str {
        match self {
            ScaleMode::FitWidth => "适应宽度",
            ScaleMode::FitHeight => "适应高度",
            ScaleMode::Fit => "适应页面",
            ScaleMode::Fill => "填充页面",
            ScaleMode::Stretch => "内容拉伸",
            ScaleMode::Center => "内容居中",
            ScaleMode::CenterScaled => "内容居中自定义缩放",
        }
    }

    /// 下拉用的英文标识（CLI 用）。
    pub fn canonical(self) -> &'static str {
        match self {
            ScaleMode::FitWidth => "fit-width",
            ScaleMode::FitHeight => "fit-height",
            ScaleMode::Fit => "fit",
            ScaleMode::Fill => "fill",
            ScaleMode::Stretch => "stretch",
            ScaleMode::Center => "center",
            ScaleMode::CenterScaled => "center-scaled",
        }
    }

    /// 解析；同时接受旧版名称，避免既有脚本失效。
    pub fn parse(text: &str) -> std::result::Result<Self, String> {
        match text.trim().to_ascii_lowercase().as_str() {
            "fit-width" | "width" | "适应宽度" => Ok(ScaleMode::FitWidth),
            "fit-height" | "height" | "适应高度" => Ok(ScaleMode::FitHeight),
            "fit" | "contain" | "适应页面" => Ok(ScaleMode::Fit),
            "fill" | "cover" | "填充页面" => Ok(ScaleMode::Fill),
            "stretch" | "内容拉伸" => Ok(ScaleMode::Stretch),
            // 旧名 resize / resize-only / only 对应旧版「仅调整页面尺寸」，现在是「内容居中」。
            "center" | "resize" | "resize-only" | "only" | "内容居中" => Ok(ScaleMode::Center),
            "center-scaled" | "percent" | "scale" | "内容居中自定义缩放" | "自定义比例" => {
                Ok(ScaleMode::CenterScaled)
            }
            _ => Err(format!(
                "未知缩放模式 `{text}`；可用：fit-width、fit-height、fit、fill、stretch、center、center-scaled"
            )),
        }
    }

    /// 是否依赖宽度参数。
    pub fn needs_width(self) -> bool {
        self != ScaleMode::FitHeight
    }

    /// 是否依赖高度参数。
    pub fn needs_height(self) -> bool {
        self != ScaleMode::FitWidth
    }

    /// 是否只在自定义尺寸下可用（这两种会改写输出页面的另一维）。
    pub fn custom_only(self) -> bool {
        matches!(self, ScaleMode::FitWidth | ScaleMode::FitHeight)
    }
}

/// 一次页面缩放的完整参数。
#[derive(Clone, Copy, Debug)]
pub struct ScaleSpec {
    pub target: Target,
    pub mode: ScaleMode,
    /// `ScaleMode::CenterScaled` 用的百分比（100 表示原尺寸）。
    pub percent: f64,
}

impl ScaleSpec {
    pub fn new(target: Target, mode: ScaleMode) -> Self {
        Self {
            target,
            mode,
            percent: 100.0,
        }
    }

    pub fn with_percent(mut self, percent: f64) -> Self {
        self.percent = percent;
        self
    }
}

/// 对一页执行缩放，返回处理后的页面尺寸（点）。
pub fn apply_page(
    doc: &mut Document,
    page: &Page,
    spec: &ScaleSpec,
    strip_old: bool,
) -> Result<(f64, f64)> {
    let crop = page.crop(doc);
    let rotation = page.rotation(doc);
    let (dw, dh) = display_size(rotation, crop);
    if !(dw > 0.0 && dh > 0.0) {
        return Err(BatchError::Arg(format!("原页面尺寸异常：{dw}×{dh}")));
    }

    let (target_w, target_h) = target_size(spec, dw, dh)?;
    let (sx, sy, out_w, out_h) = plan(spec, target_w, target_h, dw, dh)?;

    // 缩放之后一律居中。
    let tx = (out_w - dw * sx) / 2.0;
    let ty = (out_h - dh * sy) / 2.0;

    // 先把原内容摆正到显示朝向（抵消原 `/Rotate`），再缩放、居中。
    // 输出的 `/Rotate` 归零，所以不需要再转成"新页面的 user 空间"那一步。
    let matrix = Matrix(view_matrix_for(rotation, crop))
        .inverse()
        .then(Matrix::scale(sx, sy))
        .then(Matrix::translate(tx, ty));
    transform::apply_transform(doc, page, matrix, (out_w, out_h), strip_old)?;

    // 归零 `/Rotate`：显式写 0 而不是删键，避免暴露从父节点继承来的旧值。
    doc.update_dict(page.obj, |d| d.set(b"Rotate".to_vec(), Object::Int(0)))?;
    Ok((out_w, out_h))
}

/// 由目标来源与页面显示尺寸得出实际的目标宽高。
fn target_size(spec: &ScaleSpec, dw: f64, dh: f64) -> Result<(f64, f64)> {
    match spec.target {
        Target::Paper {
            long_side,
            short_side,
            orientation,
        } => {
            if spec.mode.custom_only() {
                return Err(BatchError::Arg(format!(
                    "「{}」会改写输出页面的另一维，只在自定义尺寸下可用",
                    spec.mode.label()
                )));
            }
            let landscape = match orientation {
                Orientation::Portrait => false,
                Orientation::Landscape => true,
                // 跟随原页面：这一页的显示朝向是横向就按横向。
                Orientation::Auto => dw > dh,
            };
            Ok(if landscape {
                (long_side, short_side)
            } else {
                (short_side, long_side)
            })
        }
        Target::Custom { width, height } => Ok((width, height)),
    }
}

/// 由目标宽高与页面显示尺寸算出缩放系数与**输出页面尺寸**。
///
/// 适应宽度 / 适应高度会改写输出页面的另一维，这是它们"不裁不留白"的关键。
fn plan(
    spec: &ScaleSpec,
    target_w: f64,
    target_h: f64,
    dw: f64,
    dh: f64,
) -> Result<(f64, f64, f64, f64)> {
    let (w, h) = (target_w, target_h);
    let positive = |value: f64| value.is_finite() && value > 0.0;

    match spec.mode {
        ScaleMode::FitWidth => {
            if !positive(w) {
                return Err(BatchError::Arg("「适应宽度」需要宽度参数大于 0".to_string()));
            }
            let s = w / dw;
            Ok((s, s, w, dh * s))
        }
        ScaleMode::FitHeight => {
            if !positive(h) {
                return Err(BatchError::Arg("「适应高度」需要高度参数大于 0".to_string()));
            }
            let s = h / dh;
            Ok((s, s, dw * s, h))
        }
        _ => {
            if !positive(w) || !positive(h) {
                return Err(BatchError::Arg(format!(
                    "「{}」需要宽度与高度都大于 0（当前 {}×{}）",
                    spec.mode.label(),
                    w,
                    h
                )));
            }
            let (sx, sy) = match spec.mode {
                ScaleMode::Fit => {
                    let s = (w / dw).min(h / dh);
                    (s, s)
                }
                ScaleMode::Fill => {
                    let s = (w / dw).max(h / dh);
                    (s, s)
                }
                ScaleMode::Stretch => (w / dw, h / dh),
                ScaleMode::Center => (1.0, 1.0),
                ScaleMode::CenterScaled => {
                    if !positive(spec.percent) {
                        return Err(BatchError::Arg(format!(
                            "缩放百分比必须为正，当前是 {}",
                            spec.percent
                        )));
                    }
                    let s = spec.percent / 100.0;
                    (s, s)
                }
                ScaleMode::FitWidth | ScaleMode::FitHeight => unreachable!("已在上面处理"),
            };
            Ok((sx, sy, w, h))
        }
    }
}

/// 页面的显示尺寸（`/Rotate` 为 90/270 时宽高互换）。
fn display_size(rotation: i64, crop: [f64; 4]) -> (f64, f64) {
    let (w, h) = (crop[2] - crop[0], crop[3] - crop[1]);
    if rotation % 180 == 0 {
        (w, h)
    } else {
        (h, w)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pdfcraft_cos::{Dict, Stream};
    use pdfcraft_model::pages;

    /// 一页 200×300（左下角在原点），带一段内容。
    fn document() -> Document {
        document_with_rotate(0)
    }

    /// 同上，但带 `/Rotate`。
    fn document_with_rotate(degrees: i64) -> Document {
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
        let content = doc.add(Object::Stream(Stream::from_raw(Dict::new(), b"0 0 m\n".to_vec())));
        doc.update_dict(page, |d| {
            d.set(b"Type".to_vec(), Object::name("Page"));
            d.set(b"Parent".to_vec(), Object::Ref(pages_ref));
            d.set(
                b"MediaBox".to_vec(),
                Object::Array(vec![Object::Int(0), Object::Int(0), Object::Int(200), Object::Int(300)]),
            );
            d.set(b"Contents".to_vec(), Object::Ref(content));
            if degrees != 0 {
                d.set(b"Rotate".to_vec(), Object::Int(degrees));
            }
        })
        .unwrap();
        doc
    }

    fn the_page(doc: &Document) -> Page {
        pages(doc)[0].clone()
    }

    fn spec(target: Target, mode: ScaleMode) -> ScaleSpec {
        ScaleSpec::new(target, mode)
    }

    /// 内容流里插入的变换矩阵（测试断言用）。
    fn matrix_text(doc: &mut Document) -> String {
        let page = the_page(doc);
        let list = crate::content::page_contents(doc, &page).unwrap();
        read_stream(doc, &list[0])
    }

    #[test]
    fn paper_orientation_decides_which_side_is_longer() {
        let paper = Target::paper(595.0, 842.0, Orientation::Portrait);
        assert_eq!(target_size(&spec(paper, ScaleMode::Fit), 200.0, 300.0).unwrap(), (595.0, 842.0));
        let landscape = Target::paper(595.0, 842.0, Orientation::Landscape);
        assert_eq!(
            target_size(&spec(landscape, ScaleMode::Fit), 200.0, 300.0).unwrap(),
            (842.0, 595.0)
        );
        // 跟随原页面：逐页看该页的显示朝向。
        let auto = Target::paper(595.0, 842.0, Orientation::Auto);
        assert_eq!(target_size(&spec(auto, ScaleMode::Fit), 200.0, 300.0).unwrap(), (595.0, 842.0));
        assert_eq!(target_size(&spec(auto, ScaleMode::Fit), 300.0, 200.0).unwrap(), (842.0, 595.0));
    }

    #[test]
    fn fit_scales_inside_and_centres() {
        let mut doc = document();
        let page = the_page(&doc);
        let s = spec(Target::custom(400.0, 400.0), ScaleMode::Fit);
        apply_page(&mut doc, &page, &s, false).unwrap();
        let page = the_page(&doc);
        assert_eq!(page.crop(&doc), [0.0, 0.0, 400.0, 400.0]);
        // 原 200×300 按 min(2, 1.333) = 1.333 缩放：高度铺满，宽度居中留白。
        assert!(matrix_text(&mut doc).contains("1.333333 0 0 1.333333 66.666667 0 cm"));
    }

    #[test]
    fn fill_scales_to_cover_and_crops() {
        let mut doc = document();
        let page = the_page(&doc);
        let s = spec(Target::custom(400.0, 400.0), ScaleMode::Fill);
        apply_page(&mut doc, &page, &s, false).unwrap();
        // max(2, 1.333) = 2：高度溢出（600 > 400），上下各裁 100。
        assert!(matrix_text(&mut doc).contains("2 0 0 2 0 -100 cm"));
    }

    #[test]
    fn stretch_scales_axes_independently() {
        let mut doc = document();
        let page = the_page(&doc);
        let s = spec(Target::custom(400.0, 600.0), ScaleMode::Stretch);
        apply_page(&mut doc, &page, &s, false).unwrap();
        assert!(matrix_text(&mut doc).contains("2 0 0 2 0 0 cm"));
    }

    /// 新行为：适应宽度固定宽度，**输出高度由内容决定**（不裁不留白）。
    #[test]
    fn fit_width_rewrites_the_output_height() {
        let mut doc = document();
        let page = the_page(&doc);
        let s = spec(Target::custom(400.0, 0.0), ScaleMode::FitWidth);
        let (width, height) = apply_page(&mut doc, &page, &s, false).unwrap();
        assert_eq!((width, height), (400.0, 600.0), "高度应当是 300 × (400/200)");
        let page = the_page(&doc);
        assert_eq!(page.crop(&doc), [0.0, 0.0, 400.0, 600.0]);
        // 缩放 2 倍、正好铺满，无裁切无留白。
        assert!(matrix_text(&mut doc).contains("2 0 0 2 0 0 cm"));
    }

    /// 对称：适应高度固定高度，输出宽度由内容决定。
    #[test]
    fn fit_height_rewrites_the_output_width() {
        let mut doc = document();
        let page = the_page(&doc);
        let s = spec(Target::custom(0.0, 900.0), ScaleMode::FitHeight);
        let (width, height) = apply_page(&mut doc, &page, &s, false).unwrap();
        assert_eq!((width, height), (600.0, 900.0), "宽度应当是 200 × 3");
        assert!(matrix_text(&mut doc).contains("3 0 0 3 0 0 cm"));
    }

    /// 新行为：内容居中不再贴左下角，而是摆正后居中。
    #[test]
    fn center_keeps_the_size_but_centres() {
        let mut doc = document();
        let page = the_page(&doc);
        let s = spec(Target::custom(400.0, 600.0), ScaleMode::Center);
        apply_page(&mut doc, &page, &s, false).unwrap();
        let page = the_page(&doc);
        assert_eq!(page.crop(&doc), [0.0, 0.0, 400.0, 600.0]);
        // 原尺寸 200×300 居中于 400×600：(400-200)/2 = 100、(600-300)/2 = 150。
        assert!(matrix_text(&mut doc).contains("1 0 0 1 100 150 cm"));
    }

    #[test]
    fn center_scaled_uses_the_percent() {
        let mut doc = document();
        let page = the_page(&doc);
        let s = spec(Target::custom(400.0, 600.0), ScaleMode::CenterScaled).with_percent(50.0);
        apply_page(&mut doc, &page, &s, false).unwrap();
        // 50% 内容居中：(400-100)/2 = 150、(600-150)/2 = 225。
        assert!(matrix_text(&mut doc).contains("0.5 0 0 0.5 150 225 cm"));
    }

    /// 旋转参数已删除：输出 `/Rotate` 一律归零，原旋转被烘焙进内容。
    #[test]
    fn rotation_is_zeroed_and_content_is_rectified() {
        let mut doc = document_with_rotate(90);
        let page = the_page(&doc);
        assert_eq!(page.display_size(&doc), (300.0, 200.0), "原页显示朝向是横向");
        let s = spec(Target::custom(300.0, 200.0), ScaleMode::Center);
        let (width, height) = apply_page(&mut doc, &page, &s, false).unwrap();
        let page = the_page(&doc);
        assert_eq!(page.rotation(&doc), 0, "输出的 /Rotate 应当归零");
        assert_eq!((width, height), (300.0, 200.0));
        assert_eq!(page.display_size(&doc), (300.0, 200.0), "显示尺寸仍是目标尺寸");
        // 摆正矩阵：把原 90° 的显示空间换回用户空间。
        assert!(matrix_text(&mut doc).contains("0 -1 1 0 0 200 cm"));
    }

    #[test]
    fn single_dimension_modes_are_rejected_on_paper() {
        let mut doc = document();
        let page = the_page(&doc);
        let paper = Target::paper(595.0, 842.0, Orientation::Portrait);
        assert!(apply_page(&mut doc, &page, &spec(paper, ScaleMode::FitWidth), false).is_err());
        assert!(apply_page(&mut doc, &page, &spec(paper, ScaleMode::FitHeight), false).is_err());
    }

    #[test]
    fn missing_dimensions_are_rejected() {
        let mut doc = document();
        let page = the_page(&doc);
        // 宽高都是 0：只有单维模式合法，其余必须报错。
        for mode in [ScaleMode::Fit, ScaleMode::Fill, ScaleMode::Stretch, ScaleMode::Center] {
            let s = spec(Target::custom(0.0, 0.0), mode);
            assert!(apply_page(&mut doc, &page, &s, false).is_err(), "{mode:?}");
        }
        // 适应宽度缺宽度、适应高度缺高度。
        assert!(apply_page(&mut doc, &page, &spec(Target::custom(0.0, 500.0), ScaleMode::FitWidth), false).is_err());
        assert!(apply_page(&mut doc, &page, &spec(Target::custom(500.0, 0.0), ScaleMode::FitHeight), false).is_err());
        // 比例非正。
        let bad = spec(Target::custom(400.0, 400.0), ScaleMode::CenterScaled).with_percent(0.0);
        assert!(apply_page(&mut doc, &page, &bad, false).is_err());
    }

    #[test]
    fn mode_dependencies_are_reported() {
        assert!(ScaleMode::FitWidth.needs_width() && !ScaleMode::FitWidth.needs_height());
        assert!(!ScaleMode::FitHeight.needs_width() && ScaleMode::FitHeight.needs_height());
        for mode in [ScaleMode::Fit, ScaleMode::Fill, ScaleMode::Stretch, ScaleMode::Center] {
            assert!(mode.needs_width() && mode.needs_height());
        }
        assert!(ScaleMode::FitWidth.custom_only() && ScaleMode::FitHeight.custom_only());
        assert!(!ScaleMode::Fit.custom_only());
    }

    #[test]
    fn parsing_accepts_new_and_legacy_names() {
        assert_eq!(ScaleMode::parse("fit-width").unwrap(), ScaleMode::FitWidth);
        assert_eq!(ScaleMode::parse("适应宽度").unwrap(), ScaleMode::FitWidth);
        assert_eq!(ScaleMode::parse("cover").unwrap(), ScaleMode::Fill);
        // 旧名兼容：resize 现在是「内容居中」。
        assert_eq!(ScaleMode::parse("resize").unwrap(), ScaleMode::Center);
        assert_eq!(ScaleMode::parse("percent").unwrap(), ScaleMode::CenterScaled);
        assert_eq!(ScaleMode::ALL.len(), 7);
        assert_eq!(ScaleMode::ALL[0], ScaleMode::FitWidth, "下拉第一项应当是适应宽度");
        assert_eq!(ScaleMode::ALL[6], ScaleMode::CenterScaled);
        assert!(ScaleMode::parse("squish").is_err());
        assert_eq!(Orientation::parse("跟随原页面").unwrap(), Orientation::Auto);
        assert!(Orientation::parse("diagonal").is_err());
    }

    /// 读出内容流文本（测试断言用）。
    fn read_stream(doc: &Document, object: &Object) -> String {
        let resolved = doc.resolve(object);
        let Object::Stream(stream) = resolved.as_ref() else {
            return String::new();
        };
        String::from_utf8_lossy(&stream.decoded().unwrap_or_default()).into_owned()
    }
}
