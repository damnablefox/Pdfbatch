//! 视觉主题：配色、字体与控件风格。
//!
//! 配色与控件风格照搬 PdfCraft 的设计（同为 MIT OR Apache-2.0），这样两个应用的
//! 观感一致；字体也用同一套自带字体 + craft-fonts 的中文面（霞鹜文楷）。

use std::sync::Arc;

use egui::{Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Stroke, Visuals};

/// 存放当前配色的临时存储键。
const THEME_ID: &str = "pdfbatch-theme";

/// 明暗主题。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ThemeKind {
    Light,
    Dark,
}

impl ThemeKind {
    pub const fn dark(self) -> bool {
        matches!(self, ThemeKind::Dark)
    }

    /// 切换到的另一套主题。
    pub const fn toggled(self) -> Self {
        match self {
            ThemeKind::Light => ThemeKind::Dark,
            ThemeKind::Dark => ThemeKind::Light,
        }
    }
}

/// 一套配色。
#[derive(Clone, Copy, Debug)]
pub struct Tokens {
    pub kind: ThemeKind,
    pub panel: Color32,
    pub card: Color32,
    pub border: Color32,
    pub divider: Color32,
    pub text: Color32,
    pub text_muted: Color32,
    pub text_faint: Color32,
    pub hover: Color32,
    pub pressed: Color32,
    pub accent: Color32,
    pub accent_text: Color32,
    pub accent_soft: Color32,
    pub field: Color32,
    pub radius: u8,
}

/// 便捷构造：`c(0xE9, 0xE9, 0xEB)`。
const fn c(r: u8, g: u8, b: u8) -> Color32 {
    Color32::from_rgb(r, g, b)
}

impl Tokens {
    pub fn dark(&self) -> bool {
        self.kind.dark()
    }

    /// 明暗两套取值（与 PdfCraft 一致）。
    pub fn for_kind(kind: ThemeKind) -> Self {
        match kind {
            ThemeKind::Light => Self {
                kind,
                panel: c(0xFF, 0xFF, 0xFF),
                card: c(0xFF, 0xFF, 0xFF),
                border: c(0xDA, 0xDA, 0xDE),
                divider: c(0xE8, 0xE8, 0xEB),
                text: c(0x22, 0x22, 0x26),
                text_muted: c(0x5E, 0x5E, 0x66),
                text_faint: c(0x6B, 0x6B, 0x73),
                hover: c(0xF0, 0xF0, 0xF3),
                pressed: c(0xE4, 0xE4, 0xE9),
                accent: c(0x1B, 0x63, 0xE0),
                accent_text: c(0x17, 0x55, 0xC4),
                accent_soft: c(0xE3, 0xEC, 0xFD),
                field: c(0xFF, 0xFF, 0xFF),
                radius: 6,
            },
            ThemeKind::Dark => Self {
                kind,
                panel: c(0x26, 0x26, 0x2A),
                card: c(0x2E, 0x2E, 0x33),
                border: c(0x3C, 0x3C, 0x43),
                divider: c(0x33, 0x33, 0x39),
                text: c(0xEC, 0xEC, 0xEF),
                text_muted: c(0xAE, 0xAE, 0xB6),
                text_faint: c(0x97, 0x97, 0x9E),
                hover: c(0x34, 0x34, 0x3A),
                pressed: c(0x3E, 0x3E, 0x45),
                accent: c(0x4B, 0x8B, 0xF5),
                accent_text: c(0x8C, 0xB6, 0xFA),
                accent_soft: c(0x24, 0x36, 0x57),
                field: c(0x1E, 0x1E, 0x22),
                radius: 6,
            },
        }
    }

    /// 取出当前配色；首帧尚未安装时回退到浅色。
    pub fn get(ctx: &egui::Context) -> Self {
        ctx.data(|d| d.get_temp::<Self>(egui::Id::new(THEME_ID)))
            .unwrap_or_else(|| Self::for_kind(ThemeKind::Light))
    }
}

/// 把配色与控件风格装进 egui。
pub fn apply(ctx: &egui::Context, kind: ThemeKind) {
    // 弹窗与菜单要和自定义外观用同一套明暗。
    ctx.set_theme(if kind.dark() { egui::Theme::Dark } else { egui::Theme::Light });
    let t = Tokens::for_kind(kind);
    ctx.data_mut(|d| d.insert_temp(egui::Id::new(THEME_ID), t));

    let mut v = if t.dark() { Visuals::dark() } else { Visuals::light() };
    v.panel_fill = t.panel;
    v.window_fill = t.card;
    v.window_stroke = Stroke::new(1.0, t.border);
    v.extreme_bg_color = t.field;
    v.faint_bg_color = t.hover;
    v.selection.bg_fill = t.accent_soft;
    v.selection.stroke = Stroke::new(1.0, t.accent);
    v.hyperlink_color = t.accent_text;
    v.override_text_color = Some(t.text);
    v.window_corner_radius = CornerRadius::same(10);
    v.menu_corner_radius = CornerRadius::same(8);
    v.window_shadow = egui::Shadow {
        offset: [0, 8],
        blur: 28,
        spread: 0,
        color: Color32::from_black_alpha(if t.dark() { 110 } else { 38 }),
    };
    v.popup_shadow = egui::Shadow {
        offset: [0, 4],
        blur: 16,
        spread: 0,
        color: Color32::from_black_alpha(if t.dark() { 90 } else { 30 }),
    };
    for w in [
        &mut v.widgets.noninteractive,
        &mut v.widgets.inactive,
        &mut v.widgets.hovered,
        &mut v.widgets.active,
        &mut v.widgets.open,
    ] {
        w.corner_radius = CornerRadius::same(t.radius);
        w.fg_stroke.color = t.text;
    }
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, t.divider);
    v.widgets.inactive.weak_bg_fill = Color32::TRANSPARENT;
    v.widgets.inactive.bg_fill = t.field;
    v.widgets.inactive.bg_stroke = Stroke::NONE;
    v.widgets.hovered.weak_bg_fill = t.hover;
    v.widgets.hovered.bg_fill = t.hover;
    v.widgets.hovered.bg_stroke = Stroke::NONE;
    v.widgets.active.weak_bg_fill = t.pressed;
    v.widgets.active.bg_fill = t.pressed;
    v.widgets.open.weak_bg_fill = t.hover;
    // 字形落在整像素上更清晰；浅色主题再压一点边缘灰度（与 PdfCraft 相同）。
    v.text_options.subpixel_binning = false;
    if !t.dark() {
        v.text_options.color_transfer_function = egui::epaint::FontColorTransferFunction::Gamma(0.75);
    }
    ctx.set_visuals(v);

    ctx.global_style_mut(|s| {
        s.spacing.item_spacing = egui::vec2(8.0, 6.0);
        // 交互控件统一高度、按钮内边距收紧：同一行里标签、按钮、下拉框的文字才在同一基线上。
        s.spacing.interact_size = egui::vec2(24.0, 22.0);
        s.spacing.button_padding = egui::vec2(10.0, 3.0);
        s.spacing.menu_margin = egui::Margin::same(6);
        s.spacing.scroll.bar_width = 8.0;
        s.spacing.scroll.floating = true;
        s.text_styles.insert(egui::TextStyle::Body, regular(13.0));
        s.text_styles.insert(egui::TextStyle::Button, regular(13.0));
        s.text_styles.insert(egui::TextStyle::Small, regular(11.0));
        s.text_styles.insert(egui::TextStyle::Heading, semibold(17.0));
        s.interaction.tooltip_delay = 0.35;
    });
}

/// 正文。
pub fn regular(size: f32) -> FontId {
    FontId::proportional(size)
}

/// 中等字重（标题、强调）。
pub fn medium(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("medium".into()))
}

/// 半粗（大标题）。
pub fn semibold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("semibold".into()))
}

/// 界面字体：中文字面（霞鹜文楷）内置在 `assets/fonts`，任何构建都能正常显示中文，
/// 不再依赖可选的 craft-fonts。比例字体最前面是霞鹜文楷、其后 Inter；
/// 等宽字体仍是 JetBrains Mono 在前、霞鹜文楷兜底中文。
pub fn font_definitions() -> FontDefinitions {
    let mut fonts = FontDefinitions::default();
    let add = |fonts: &mut FontDefinitions, name: &str, bytes: &'static [u8]| {
        fonts
            .font_data
            .insert(name.to_owned(), Arc::new(FontData::from_static(bytes)));
    };
    add(&mut fonts, "Inter", include_bytes!("../../../assets/fonts/Inter-Regular.ttf"));
    add(&mut fonts, "Inter-Medium", include_bytes!("../../../assets/fonts/Inter-Medium.ttf"));
    add(&mut fonts, "Inter-SemiBold", include_bytes!("../../../assets/fonts/Inter-SemiBold.ttf"));
    add(
        &mut fonts,
        "JetBrainsMono",
        include_bytes!("../../../assets/fonts/JetBrainsMono-Regular.ttf"),
    );
    // 内置霞鹜文楷（SIL OFL 1.1）：界面中文的统一字面。
    add(
        &mut fonts,
        "LXGWWenKai",
        include_bytes!("../../../assets/fonts/LXGWWenKai-Regular.ttf"),
    );

    // 比例字体：霞鹜文楷最前、Inter 次之，其后是 egui 自带兜底。
    let proportional = fonts.families.entry(FontFamily::Proportional).or_default();
    proportional.insert(0, "LXGWWenKai".to_owned());
    proportional.insert(1, "Inter".to_owned());
    // 等宽字体：JetBrains Mono 在前，中文用霞鹜文楷兜底。
    let monospace = fonts.families.entry(FontFamily::Monospace).or_default();
    monospace.insert(0, "JetBrainsMono".to_owned());
    monospace.push("LXGWWenKai".to_owned());

    // 带字重的两个族：半粗体在前，其余回退到比例字体栈。
    let fallback: Vec<String> = fonts.families[&FontFamily::Proportional].clone();
    for (family, primary) in [("medium", "Inter-Medium"), ("semibold", "Inter-SemiBold")] {
        let mut stack = vec![primary.to_owned()];
        stack.extend(fallback.iter().cloned());
        fonts.families.insert(FontFamily::Name(family.into()), stack);
    }
    fonts
}

/// 安装界面字体（只在第一帧调用一次）。
pub fn install_fonts(ctx: &egui::Context) {
    ctx.set_fonts(font_definitions());
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::epaint::text::{Fonts, TextOptions};

    /// 明暗两套都要给出可用配色，且文字与背景有足够反差。
    #[test]
    fn both_themes_are_usable() {
        for kind in [ThemeKind::Light, ThemeKind::Dark] {
            let t = Tokens::for_kind(kind);
            assert_eq!(t.kind, kind);
            assert_ne!(t.text, t.panel, "{kind:?}: 文字与面板同色");
            assert_ne!(t.accent, t.panel, "{kind:?}: 强调色与面板同色");
            assert_eq!(t.radius, 6);
        }
        assert_eq!(ThemeKind::Light.toggled(), ThemeKind::Dark);
        assert_eq!(ThemeKind::Dark.toggled(), ThemeKind::Light);
    }

    /// 中文面（霞鹜文楷）内置在字体栈最前，中文才有真字形。
    #[test]
    fn chinese_interface_text_has_real_glyphs() {
        let defs = font_definitions();
        let stack = &defs.families[&FontFamily::Proportional];
        assert_eq!(stack[0], "LXGWWenKai", "内置中文字体应当排在比例字体最前：{stack:?}");

        let mut fonts = Fonts::new(TextOptions::default(), defs);
        let id = FontId::proportional(13.0);
        // 用排版宽度判断有没有真字形：汉字接近一个全宽，替换框会明显更窄。
        // （egui 的 `has_glyphs` 在"主字体即替换字体"时给不出可信答案，所以不采用。）
        let width = fonts
            .with_pixels_per_point(2.0)
            .layout_no_wrap("简体中文".to_owned(), id, Color32::BLACK)
            .size()
            .x;
        assert!(width > 13.0 * 0.8 * 4.0, "宽度 {width} 不像真实的汉字字形");
    }

    /// 等宽字体仍以 JetBrains Mono 打头，代码区不变楷体。
    #[test]
    fn monospace_keeps_its_own_font_first() {
        let defs = font_definitions();
        let stack = &defs.families[&FontFamily::Monospace];
        assert_eq!(stack[0], "JetBrainsMono", "{stack:?}");
    }
}