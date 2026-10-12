//! 页面尺寸：按目标尺寸缩放内容。
//!
//! 参数分成三块：
//!
//! - **目标尺寸**：一张纸的宽高（点），方向决定宽高谁大；
//! - **缩放模式**：内容怎么落到目标页面上（适应 / 填充 / 拉伸 / 百分比 / 只改尺寸…）；
//! - **旋转**：保持原 `/Rotate`，或指定角度。
//!
//! 实现上统一走 [`crate::transform`]：把「原内容 → 新页面」的映射写成一个矩阵，
//! 插到内容流最前；页面尺寸同时改写。原有的 `/Rotate` 会被「摆正」进这个矩阵
//! （`view_matrix` 的逆），所以指定 `0°` 就等于「把 `/Rotate` 归零并并入内容变换」。

use pdfcraft_cos::{Document, Object};
use pdfcraft_model::{Page, view_matrix_for};

use crate::error::{BatchError, Result};
use crate::transform::{self, Matrix};

/// 目标页面的方向：决定宽高谁大。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Orientation {
    /// 纵向（高 ≥ 宽）。
    Portrait,
    /// 横向（宽 ≥ 高）。
    Landscape,
    /// 跟随原页面的显示朝向（即「保持原方向」）。
    Auto,
}

impl Orientation {
    pub fn parse(text: &str) -> std::result::Result<Self, String> {
        match text.trim().to_ascii_lowercase().as_str() {
            "portrait" | "p" | "纵" | "纵向" => Ok(Orientation::Portrait),
            "landscape" | "l" | "横" | "横向" => Ok(Orientation::Landscape),
            "auto" | "keep" | "自动" | "保持" | "保持原方向" => Ok(Orientation::Auto),
            _ => Err(format!("未知方向 `{text}`；可用：portrait、landscape、auto")),
        }
    }

    pub fn canonical(self) -> &'static str {
        match self {
            Orientation::Portrait => "portrait",
            Orientation::Landscape => "landscape",
            Orientation::Auto => "auto",
        }
    }
}

/// 内容如何落到目标页面上。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScaleMode {
    /// 只改页面尺寸，内容不缩放不平移（可能被裁或留白）。
    ResizeOnly,
    /// 等比缩放，完整显示在页面内（四周留白）。
    Fit,
    /// 等比缩放填满页面（超出部分被裁）。
    Fill,
    /// 非等比拉伸填满页面（可能变形）。
    Stretch,
    /// 按用户给的百分比缩放。
    Percent,
    /// 等比缩放使宽度匹配页面宽度。
    FitWidth,
    /// 等比缩放使高度匹配页面高度。
    FitHeight,
}

impl ScaleMode {
    /// 除 `Percent` 外的全部模式。
    pub const ALL: [ScaleMode; 7] = [
        ScaleMode::ResizeOnly,
        ScaleMode::Fit,
        ScaleMode::Fill,
        ScaleMode::Stretch,
        ScaleMode::Percent,
        ScaleMode::FitWidth,
        ScaleMode::FitHeight,
    ];

    pub fn label(self) -> &'static str {
        match self {
            ScaleMode::ResizeOnly => "仅调整页面尺寸",
            ScaleMode::Fit => "适应页面",
            ScaleMode::Fill => "填充页面",
            ScaleMode::Stretch => "内容拉伸",
            ScaleMode::Percent => "自定义比例",
            ScaleMode::FitWidth => "适应宽度",
            ScaleMode::FitHeight => "适应高度",
        }
    }

    pub fn parse(text: &str) -> std::result::Result<Self, String> {
        match text.trim().to_ascii_lowercase().as_str() {
            "resize" | "resize-only" | "only" => Ok(ScaleMode::ResizeOnly),
            "fit" | "contain" => Ok(ScaleMode::Fit),
            "fill" | "cover" => Ok(ScaleMode::Fill),
            "stretch" => Ok(ScaleMode::Stretch),
            "percent" | "scale" => Ok(ScaleMode::Percent),
            "fit-width" | "width" => Ok(ScaleMode::FitWidth),
            "fit-height" | "height" => Ok(ScaleMode::FitHeight),
            _ => Err(format!(
                "未知缩放模式 `{text}`；可用：resize、fit、fill、stretch、percent、fit-width、fit-height"
            )),
        }
    }
}

/// 输出的 `/Rotate` 怎么处理。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rotation {
    /// 保持原值不动。
    Keep,
    /// 指定角度（0/90/180/270）。`Set(0)` 即「归零并并入内容变换」。
    Set(i64),
}

impl Rotation {
    /// 解析角度写法；`keep` 表示保持。
    pub fn parse(text: &str) -> std::result::Result<Self, String> {
        let text = text.trim();
        if text.eq_ignore_ascii_case("keep") || text == "保持" {
            return Ok(Rotation::Keep);
        }
        let degrees: i64 = text
            .parse()
            .map_err(|_| format!("未知旋转 `{text}`；可用：keep、0、90、180、270"))?;
        Rotation::from_degrees(degrees)
    }

    /// 校验角度必须是 90 的倍数。
    pub fn from_degrees(degrees: i64) -> std::result::Result<Self, String> {
        let normalized = degrees.rem_euclid(360);
        if normalized % 90 != 0 {
            return Err(format!("旋转角度 `{degrees}` 必须是 90 的倍数"));
        }
        Ok(Rotation::Set(normalized))
    }

    pub fn canonical(self) -> String {
        match self {
            Rotation::Keep => "keep".to_string(),
            Rotation::Set(deg) => deg.to_string(),
        }
    }
}

/// 一次页面缩放的完整参数。
#[derive(Clone, Copy, Debug)]
pub struct ScaleSpec {
    /// 目标纸张尺寸（点），纵向基准；实际宽高由 `orientation` 决定。
    pub size: (f64, f64),
    pub orientation: Orientation,
    pub mode: ScaleMode,
    /// `ScaleMode::Percent` 用的百分比（100 表示原尺寸）。
    pub percent: f64,
    pub rotation: Rotation,
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
    let display = display_size(rotation, crop);
    let target = resolve_size(spec, display);

    if !(target.0.is_finite() && target.1.is_finite()) || target.0 <= 0.0 || target.1 <= 0.0 {
        return Err(BatchError::Arg(format!(
            "目标页面尺寸必须为正，当前是 {}×{}",
            target.0, target.1
        )));
    }
    if spec.mode == ScaleMode::Percent && !(spec.percent.is_finite() && spec.percent > 0.0) {
        return Err(BatchError::Arg(format!("缩放百分比必须为正，当前是 {}", spec.percent)));
    }

    let target_rotation = match spec.rotation {
        Rotation::Keep => rotation,
        Rotation::Set(deg) => deg.rem_euclid(360),
    };
    // `/Rotate` 为 90/270 时，页面的 user 空间矩形要把宽高换过来，显示出来才是目标尺寸。
    let user_size = if target_rotation % 180 == 0 {
        target
    } else {
        (target.1, target.0)
    };

    let matrix = content_matrix(spec, crop, rotation, display, target, target_rotation, user_size);
    transform::apply_transform(doc, page, matrix, user_size, strip_old)?;

    if let Rotation::Set(deg) = spec.rotation {
        doc.update_dict(page.obj, |d| d.set(b"Rotate".to_vec(), Object::Int(deg.rem_euclid(360))))?;
    }
    Ok(user_size)
}

/// 由「纸张尺寸 + 方向」得出实际的目标显示尺寸。
pub fn resolve_size(spec: &ScaleSpec, display: (f64, f64)) -> (f64, f64) {
    let (w, h) = spec.size;
    let (long, short) = if w >= h { (w, h) } else { (h, w) };
    let landscape = match spec.orientation {
        Orientation::Portrait => false,
        Orientation::Landscape => true,
        Orientation::Auto => display.0 > display.1,
    };
    if landscape {
        (long, short)
    } else {
        (short, long)
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

/// 组装「原内容 → 新页面」的内容变换矩阵。
///
/// 顺序是：先把原内容摆正到显示朝向（`view_matrix` 的逆），再按显示尺寸缩放，
/// 再平移到目标页面居中，最后转成新页面的 user 空间（新 `/Rotate` 的 `view_matrix`）。
#[allow(clippy::too_many_arguments)]
fn content_matrix(
    spec: &ScaleSpec,
    crop: [f64; 4],
    rotation: i64,
    display: (f64, f64),
    target: (f64, f64),
    target_rotation: i64,
    user_size: (f64, f64),
) -> Matrix {
    // 「只改尺寸」不动内容：矩阵为单位阵，只改画面大小。
    if spec.mode == ScaleMode::ResizeOnly {
        return Matrix::IDENTITY;
    }

    let (dw, dh) = display;
    let (tw, th) = target;
    let (sx, sy) = match spec.mode {
        ScaleMode::Fit => {
            let s = (tw / dw).min(th / dh);
            (s, s)
        }
        ScaleMode::Fill => {
            let s = (tw / dw).max(th / dh);
            (s, s)
        }
        ScaleMode::Stretch => (tw / dw, th / dh),
        ScaleMode::Percent => {
            let s = spec.percent / 100.0;
            (s, s)
        }
        ScaleMode::FitWidth => {
            let s = tw / dw;
            (s, s)
        }
        ScaleMode::FitHeight => {
            let s = th / dh;
            (s, s)
        }
        ScaleMode::ResizeOnly => (1.0, 1.0),
    };

    // 缩放后居中。
    let tx = (tw - dw * sx) / 2.0;
    let ty = (th - dh * sy) / 2.0;

    let to_display = Matrix(view_matrix_for(rotation, crop)).inverse();
    let to_new_user = Matrix(view_matrix_for(target_rotation, [0.0, 0.0, user_size.0, user_size.1]));
    to_display
        .then(Matrix::scale(sx, sy))
        .then(Matrix::translate(tx, ty))
        .then(to_new_user)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pdfcraft_cos::{Dict, Object, Stream};
    use pdfcraft_model::pages;

    fn spec(size: (f64, f64), orientation: Orientation, mode: ScaleMode) -> ScaleSpec {
        ScaleSpec {
            size,
            orientation,
            mode,
            percent: 100.0,
            rotation: Rotation::Keep,
        }
    }

    /// 一页 200×300（左下角在原点），带一段内容。
    fn document() -> Document {
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
        })
        .unwrap();
        doc
    }

    fn the_page(doc: &Document) -> Page {
        pages(doc)[0].clone()
    }

    #[test]
    fn orientation_decides_which_side_is_longer() {
        let display = (200.0, 300.0);
        let a4 = (595.0, 842.0);
        assert_eq!(
            resolve_size(&spec(a4, Orientation::Portrait, ScaleMode::Fit), display),
            (595.0, 842.0)
        );
        assert_eq!(
            resolve_size(&spec(a4, Orientation::Landscape, ScaleMode::Fit), display),
            (842.0, 595.0)
        );
        // 自动：跟随原页面的显示朝向（这里是纵向）。
        assert_eq!(resolve_size(&spec(a4, Orientation::Auto, ScaleMode::Fit), display), (595.0, 842.0));
        assert_eq!(
            resolve_size(&spec(a4, Orientation::Auto, ScaleMode::Fit), (300.0, 200.0)),
            (842.0, 595.0)
        );
    }

    #[test]
    fn fit_scales_inside_and_centres() {
        let mut doc = document();
        let page = the_page(&doc);
        // 目标 400×400：原 200×300 按 min(2, 1.333) = 1.333 缩放。
        apply_page(&mut doc, &page, &spec((400.0, 400.0), Orientation::Portrait, ScaleMode::Fit), false).unwrap();
        let page = the_page(&doc);
        assert_eq!(page.crop(&doc), [0.0, 0.0, 400.0, 400.0]);
        // 内容高度正好铺满，宽度居中：(400 - 200·4/3)/2 = 66.67。
        let list = crate::content::page_contents(&mut doc, &page).unwrap();
        let text = read_stream(&doc, &list[0]);
        assert!(text.contains("1.3333 0 0 1.3333 66.6667 0 cm"), "{text}");
    }

    #[test]
    fn fill_scales_to_cover_and_crops() {
        let mut doc = document();
        let page = the_page(&doc);
        apply_page(&mut doc, &page, &spec((400.0, 400.0), Orientation::Portrait, ScaleMode::Fill), false).unwrap();
        let page = the_page(&doc);
        let list = crate::content::page_contents(&mut doc, &page).unwrap();
        let text = read_stream(&doc, &list[0]);
        // max(2, 1.333) = 2：高度溢出（600 > 400），上下各裁 100。
        assert!(text.contains("2 0 0 2 0 -100 cm"), "{text}");
    }

    #[test]
    fn stretch_scales_axes_independently() {
        let mut doc = document();
        let page = the_page(&doc);
        apply_page(&mut doc, &page, &spec((400.0, 600.0), Orientation::Portrait, ScaleMode::Stretch), false).unwrap();
        let page = the_page(&doc);
        let list = crate::content::page_contents(&mut doc, &page).unwrap();
        let text = read_stream(&doc, &list[0]);
        assert!(text.contains("2 0 0 2 0 0 cm"), "{text}");
    }

    #[test]
    fn percent_uses_the_original_size() {
        let mut doc = document();
        let page = the_page(&doc);
        let mut s = spec((400.0, 600.0), Orientation::Portrait, ScaleMode::Percent);
        s.percent = 50.0;
        apply_page(&mut doc, &page, &s, false).unwrap();
        let page = the_page(&doc);
        let list = crate::content::page_contents(&mut doc, &page).unwrap();
        let text = read_stream(&doc, &list[0]);
        // 50% 内容居中在 400×600 里：(400-100)/2=150、(600-150)/2=225。
        assert!(text.contains("0.5 0 0 0.5 150 225 cm"), "{text}");
    }

    #[test]
    fn resize_only_leaves_content_untouched() {
        let mut doc = document();
        let page = the_page(&doc);
        apply_page(&mut doc, &page, &spec((400.0, 600.0), Orientation::Portrait, ScaleMode::ResizeOnly), false)
            .unwrap();
        let page = the_page(&doc);
        assert_eq!(page.crop(&doc), [0.0, 0.0, 400.0, 600.0]);
        assert!(!transform::has_transform(&mut doc, &page).unwrap(), "不该插变换层");
    }

    #[test]
    fn rotation_swaps_the_user_rectangle() {
        let mut doc = document();
        let page = the_page(&doc);
        let mut s = spec((400.0, 600.0), Orientation::Portrait, ScaleMode::Fit);
        s.rotation = Rotation::Set(90);
        apply_page(&mut doc, &page, &s, false).unwrap();
        let page = the_page(&doc);
        // /Rotate = 90，user 空间矩形宽高互换，显示出来仍是 400×600。
        assert_eq!(page.rotation(&doc), 90);
        assert_eq!(page.display_size(&doc), (400.0, 600.0));
        assert_eq!(page.crop(&doc), [0.0, 0.0, 600.0, 400.0]);
    }

    #[test]
    fn rotation_and_percent_validation() {
        assert!(Rotation::parse("keep").is_ok());
        assert!(Rotation::parse("90").is_ok());
        assert_eq!(Rotation::from_degrees(-90).unwrap(), Rotation::Set(270));
        assert!(Rotation::parse("45").is_err());
        assert!(Rotation::from_degrees(45).is_err());
        assert_eq!(Orientation::parse("横向").unwrap(), Orientation::Landscape);
        assert!(Orientation::parse("diagonal").is_err());
        assert_eq!(ScaleMode::parse("cover").unwrap(), ScaleMode::Fill);
        assert!(ScaleMode::parse("squish").is_err());
    }

    #[test]
    fn a_bad_percent_is_rejected() {
        let mut doc = document();
        let page = the_page(&doc);
        let mut s = spec((400.0, 600.0), Orientation::Portrait, ScaleMode::Percent);
        s.percent = 0.0;
        assert!(apply_page(&mut doc, &page, &s, false).is_err());
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
