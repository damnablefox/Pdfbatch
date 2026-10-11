//! 界面小控件：区块标题、模式标签页、药丸按钮、页面示意图、提示条。
//!
//! 形状与配色都对齐 PdfCraft 的控件（线条、圆角、悬停底色），这样两个应用的观感一致。

use egui::{Align2, Color32, CornerRadius, Rect, Response, Sense, Stroke, vec2};

use crate::theme::{self, Tokens};

/// 区块标题：小号、字距略宽、颜色淡。
pub fn section_title(ui: &mut egui::Ui, text: &str) {
    let t = Tokens::get(ui.ctx());
    ui.add_space(10.0);
    ui.label(
        egui::RichText::new(text)
            .font(theme::semibold(10.5))
            .color(t.text_faint)
            .extra_letter_spacing(0.6),
    );
    ui.add_space(2.0);
}

/// 顶栏模式标签页：文字下方一条下划线表示选中。
pub fn mode_tab(ui: &mut egui::Ui, label: &str, active: bool) -> Response {
    let t = Tokens::get(ui.ctx());
    let font = if active { theme::semibold(13.5) } else { theme::medium(13.5) };
    let w = ui.fonts_mut(|f| f.layout_no_wrap(label.to_owned(), font.clone(), t.text).size().x);
    let (rect, resp) = ui.allocate_exact_size(vec2(w + 22.0, 38.0), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, ui.is_enabled(), active, label));
    if resp.hovered() && !active {
        ui.painter().rect_filled(rect.shrink2(vec2(2.0, 7.0)), CornerRadius::same(6), t.hover);
    }
    ui.painter().text(rect.center(), Align2::CENTER_CENTER, label, font, if active { t.text } else { t.text_muted });
    if active {
        let r = Rect::from_min_max(rect.left_bottom() + vec2(11.0, -3.0), rect.right_bottom() - vec2(11.0, 0.0));
        ui.painter().rect_filled(r, CornerRadius::same(1), t.accent);
    }
    resp
}

/// 药丸按钮；`primary` 用强调色填充，否则是描边样式。禁用时置灰且不响应点击。
pub fn pill_button(ui: &mut egui::Ui, label: &str, primary: bool) -> Response {
    let t = Tokens::get(ui.ctx());
    let enabled = ui.is_enabled();
    let font = theme::medium(13.0);
    let w = ui.fonts_mut(|f| f.layout_no_wrap(label.to_owned(), font.clone(), t.text).size().x);
    let (rect, resp) = ui.allocate_exact_size(vec2(w + 28.0, 30.0), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, label));
    let hovered = resp.hovered() && enabled;
    let (fill, stroke, text) = if !enabled {
        (t.hover, Stroke::NONE, t.text_faint)
    } else if primary {
        (if hovered { t.accent_text } else { t.accent }, Stroke::NONE, Color32::WHITE)
    } else {
        (if hovered { t.hover } else { t.card }, Stroke::new(1.0, t.border), t.text)
    };
    ui.painter().rect(rect, CornerRadius::same(15), fill, stroke, egui::StrokeKind::Inside);
    ui.painter().text(rect.center(), Align2::CENTER_CENTER, label, font, text);
    resp
}

/// 简易的页面示意图：按 A4 页估算比例，画出页面轮廓与填充矩形的位置。
///
/// 只用来帮助肉眼确认"矩形大致落在页面哪里"，不做真实 PDF 渲染。
pub fn page_preview(ui: &mut egui::Ui, rect: [f64; 4], color: Color32, opacity: f32, whole_page: bool) {
    let t = Tokens::get(ui.ctx());
    // A4：595×842 点。示意图按这个比例换算。
    const PAGE_W: f64 = 595.0;
    const PAGE_H: f64 = 842.0;
    let display_w = 148.0_f32;
    let scale = (display_w / PAGE_W as f32) as f64;
    let display_h = (PAGE_H * scale) as f32;

    let (page_rect, _) = ui.allocate_exact_size(vec2(display_w, display_h), Sense::hover());
    let painter = ui.painter();
    // 纸面与边框。
    painter.rect(page_rect, CornerRadius::same(2), Color32::WHITE, Stroke::new(1.0, t.border), egui::StrokeKind::Inside);

    let shown = if whole_page {
        [0.0, 0.0, PAGE_W, PAGE_H]
    } else {
        rect
    };
    let x0 = page_rect.left() + (shown[0] * scale) as f32;
    let y_top = page_rect.top() + ((PAGE_H - (shown[1] + shown[3])) * scale) as f32;
    let w = (shown[2] * scale) as f32;
    let h = (shown[3] * scale) as f32;
    // 按不透明度把颜色叠到纸面上（留一点下限，免得全透明时什么都看不见）。
    let alpha = (opacity.clamp(0.0, 1.0) * 255.0).max(20.0) as u8;
    let fill = Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), alpha);
    let rect_area = Rect::from_min_size(egui::pos2(x0, y_top), vec2(w.max(1.0), h.max(1.0))).intersect(page_rect);
    painter.rect_filled(rect_area, CornerRadius::ZERO, fill);
    painter.rect_stroke(rect_area, CornerRadius::ZERO, Stroke::new(1.0, t.accent), egui::StrokeKind::Inside);
    ui.add_space(2.0);
    ui.label(egui::RichText::new("示意图（按 A4 页估算）").font(theme::regular(10.5)).color(t.text_faint));
}

/// 底部的浮动提示条（3.5 秒后由调用方清除）。返回它占用的高度，供摆放进度卡参考。
pub fn toast(ctx: &egui::Context, message: &str) {
    let t = Tokens::get(ctx);
    let screen = ctx.content_rect();
    egui::Area::new(egui::Id::new("pdfbatch-toast"))
        .order(egui::Order::Tooltip)
        .pivot(Align2::CENTER_BOTTOM)
        .fixed_pos(screen.center_bottom() - vec2(0.0, 20.0))
        .show(ctx, |ui| {
            let (fill, text) = if t.dark() {
                (Color32::from_rgb(0xEC, 0xEC, 0xEF), Color32::from_rgb(0x22, 0x22, 0x26))
            } else {
                (Color32::from_rgb(0x2A, 0x2A, 0x2F), Color32::WHITE)
            };
            egui::Frame::NONE
                .fill(fill)
                .corner_radius(CornerRadius::same(8))
                .inner_margin(egui::Margin::symmetric(16, 10))
                .show(ui, |ui| {
                    ui.set_max_width((screen.width() - 64.0).clamp(1.0, 560.0));
                    ui.add(egui::Label::new(egui::RichText::new(message).color(text)).wrap());
                });
        });
}