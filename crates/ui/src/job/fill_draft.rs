//! 填充区域模式的参数草稿，以及由草稿换算查找条件与改写内容。

use pdfbatch_core::units::Unit;
use pdfbatch_core::{Blend, FillChange, Layer, Matcher, RectMatch};

use super::mode::{ChangeRect, FillAction, FillEditMode, MatchRect};

/// 填充区域模式的参数草稿。
#[derive(Clone, Debug)]
pub struct FillDraft {
    /// 新增，或删改。
    pub action: FillAction,
    /// `FillAction::Edit` 时用：删除还是修改。
    pub edit_mode: FillEditMode,

    // —— 新增用：要盖上去的样式 ——
    pub whole_page: bool,
    /// 自定义矩形 `[x, y, 宽, 高]`，按 `unit` 解释（原点在页面左下角）。
    pub rect: [f64; 4],
    /// 颜色，按 sRGB 存储（取色控件与 PDF 的 `rg` 用的是同一套取值）。
    pub color: egui::Color32,
    pub opacity: f64,
    pub blend: Blend,
    pub layer: Layer,

    // —— 所有尺寸输入共用的单位 ——
    /// 矩形与尺寸容差的输入单位（内部一律换算成点再比较）。
    pub unit: Unit,

    // —— 查找用：匹配条件 ——
    pub match_rect: MatchRect,
    /// `MatchRect::Size` 时用到的宽高（按 `unit` 解释）。
    pub match_size: [f64; 2],
    /// `MatchRect::Rect` 时用到的左下起点与宽高（按 `unit` 解释）。
    pub match_rect_value: [f64; 4],
    pub match_color_on: bool,
    pub match_color: egui::Color32,
    pub match_opacity_on: bool,
    pub match_opacity: f64,
    pub match_blend_on: bool,
    pub match_blend: Blend,
    pub match_layer_on: bool,
    pub match_layer: Layer,
    /// 尺寸容差（按 `unit` 解释）；0 表示精确相等。
    pub rect_tolerance: f64,
    /// 颜色与不透明度容差（0–1，逐通道 ±）；0 表示精确相等。
    pub color_tolerance: f64,

    // —— 修改用：更改要求（每项可单独开关） ——
    pub change_rect: ChangeRect,
    pub change_rect_value: [f64; 4],
    pub change_color_on: bool,
    pub change_color: egui::Color32,
    pub change_opacity_on: bool,
    pub change_opacity: f64,
    pub change_blend_on: bool,
    pub change_blend: Blend,
    pub change_layer_on: bool,
    pub change_layer: Layer,

    /// 页范围原文（`all` 或 `1-5,8,10-`）。
    pub pages: String,
}

impl Default for FillDraft {
    fn default() -> Self {
        // 自定义矩形默认按毫米给整数，界面回显不会出现 25.40 这类零碎数字。
        let default_rect = [20.0, 20.0, 150.0, 50.0];
        let default_color = egui::Color32::from_rgb(255, 235, 140);
        Self {
            action: FillAction::Add,
            edit_mode: FillEditMode::Erase,

            whole_page: true,
            rect: default_rect,
            color: default_color,
            opacity: 1.0,
            blend: Blend::Normal,
            layer: Layer::Top,

            unit: Unit::Mm,

            match_rect: MatchRect::Any,
            match_size: [150.0, 50.0],
            match_rect_value: default_rect,
            match_color_on: false,
            match_color: default_color,
            match_opacity_on: false,
            match_opacity: 1.0,
            match_blend_on: false,
            match_blend: Blend::Normal,
            match_layer_on: false,
            match_layer: Layer::Top,
            rect_tolerance: 0.0,
            color_tolerance: 0.0,

            change_rect: ChangeRect::Keep,
            change_rect_value: default_rect,
            change_color_on: false,
            change_color: default_color,
            change_opacity_on: false,
            change_opacity: 1.0,
            change_blend_on: false,
            change_blend: Blend::Normal,
            change_layer_on: false,
            change_layer: Layer::Top,

            pages: "all".to_string(),
        }
    }
}

/// 把 sRGB 取色控件的颜色转成 PDF 的 0.0–1.0 分量。
pub(crate) fn srgb(color: egui::Color32) -> [f64; 3] {
    [
        f64::from(color.r()) / 255.0,
        f64::from(color.g()) / 255.0,
        f64::from(color.b()) / 255.0,
    ]
}

/// 把草稿里按 `unit` 解释的尺寸换算成点。
pub(crate) fn rect_pt(unit: Unit, rect: [f64; 4]) -> [f64; 4] {
    rect.map(|value| unit.to_pt(value))
}

/// 由草稿里的匹配条件得到查找谓词。
pub(crate) fn fill_matcher(fill: &FillDraft) -> Matcher {
    let unit = fill.unit;
    Matcher {
        rect: match fill.match_rect {
            MatchRect::Any => None,
            MatchRect::FullPage => Some(RectMatch::FullPage),
            MatchRect::Size => {
                Some(RectMatch::Size([unit.to_pt(fill.match_size[0]), unit.to_pt(fill.match_size[1])]))
            }
            MatchRect::Rect => Some(RectMatch::Rect(rect_pt(unit, fill.match_rect_value))),
        },
        color: fill.match_color_on.then(|| srgb(fill.match_color)),
        opacity: fill.match_opacity_on.then_some(fill.match_opacity),
        blend: fill.match_blend_on.then_some(fill.match_blend),
        layer: fill.match_layer_on.then_some(fill.match_layer),
        rect_tolerance: unit.to_pt(fill.rect_tolerance),
        color_tolerance: fill.color_tolerance,
    }
}

/// 由草稿里的更改要求得到改写内容；全为 `None` 时返回空（表示没勾任何项）。
pub(crate) fn fill_change(fill: &FillDraft) -> FillChange {
    FillChange {
        rect: (fill.change_rect == ChangeRect::Rect)
            .then(|| rect_pt(fill.unit, fill.change_rect_value)),
        full_page: (fill.change_rect == ChangeRect::FullPage).then_some(true),
        color: fill.change_color_on.then(|| srgb(fill.change_color)),
        opacity: fill.change_opacity_on.then_some(fill.change_opacity),
        blend: fill.change_blend_on.then_some(fill.change_blend),
        layer: fill.change_layer_on.then_some(fill.change_layer),
    }
}

/// 匹配条件的指纹：条件一变，之前的查找结果就作废，需重新查找。
///
/// 单位也纳入指纹：同一个数值换单位后指向的物理尺寸不同，匹配结果也随之改变。
pub fn matcher_fingerprint(fill: &FillDraft) -> String {
    format!(
        "unit={:?} rect={:?} size={:?} rectv={:?} color={}:{:?} opacity={}:{} blend={}:{:?} layer={}:{:?} rtol={} ctol={} pages={}",
        fill.unit,
        fill.match_rect,
        fill.match_size,
        fill.match_rect_value,
        fill.match_color_on,
        fill.match_color,
        fill.match_opacity_on,
        fill.match_opacity,
        fill.match_blend_on,
        fill.match_blend,
        fill.match_layer_on,
        fill.match_layer,
        fill.rect_tolerance,
        fill.color_tolerance,
        fill.pages,
    )
}

/// 页范围原文（空则按 `all`）。
pub(crate) fn pages_text(fill: &FillDraft) -> String {
    if fill.pages.trim().is_empty() {
        "all".to_string()
    } else {
        fill.pages.clone()
    }
}
