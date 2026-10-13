//! 颜色选择器：Photoshop 拾色器风格，**常驻在参数面板里**（不用弹层）。
//!
//! 早先用的是弹出层，点到界面别处就会自动关闭，拖动色域方块时很容易误触消失；
//! 改成常驻后没有这个问题。
//!
//! 左起：色域方块（横轴饱和度、纵轴明度）、竖直色相条（左右三角滑块）、
//! 右侧是带组选择圆点的 HSB 与 RGB 两组数值；下方是归一化数值与十六进制输入。
//!
//! 真值始终是 sRGB 字节值（与 PDF 的 `rg` 语义一致）：只有写入 PDF 时才除以 255。

use egui::ecolor::Hsva;
use egui::epaint::{Mesh, Vertex, WHITE_UV};
use egui::{Color32, CornerRadius, DragValue, Rect, Sense, Shape, Stroke, StrokeKind, pos2, vec2};

use crate::theme::Tokens;

/// 色域方块与色相条的尺寸（嵌入面板，比弹层版小一圈）。
const SV_WIDTH: f32 = 150.0;
const SV_HEIGHT: f32 = 120.0;
const HUE_WIDTH: f32 = 16.0;
/// 色相条两侧要给三角滑块留出空间，否则会被裁剪。
const HUE_PADDING: f32 = 10.0;

/// 选择器的编辑状态。
///
/// **所有输入框绑定的都是这里的持久字段**，而不是每帧重新算出的局部变量：
/// 即时模式下 DragValue 在编辑中（未提交）不会报告 changed，若每帧把它绑回派生值，
/// 用户刚敲进去的数字会被立刻重置，表现为「改不动」。
#[derive(Clone)]
struct PaletteState {
    /// 十六进制文本（用户可能正在编辑，不能被覆盖）。
    hex: String,
    /// 真值：HSV。保留色相，避免明度拖到 0（纯黑）后色相丢失。
    hsva: Hsva,
    /// sRGB 颜色。
    color: Color32,
    /// 与外部颜色一致的快照：不一致说明颜色被别处改了，需要重建。
    seen: Color32,
    /// 当前高亮的数值组。
    group: u8,
    // —— 各输入框的持久数值 ——
    /// 色相 0–360（度）。
    h: f64,
    /// 饱和度 0–100（%）。
    s: f64,
    /// 明度 0–100（%）。
    b: f64,
    /// 红 0–255。
    r: f64,
    /// 绿 0–255。
    g: f64,
    /// 蓝 0–255。
    blue: f64,
    /// 归一化红 0–1。
    nr: f64,
    /// 归一化绿 0–1。
    ng: f64,
    /// 归一化蓝 0–1。
    nb: f64,
}

impl PaletteState {
    fn new(color: Color32) -> Self {
        let hsva = hsva_of(color);
        Self {
            hex: hex_text(color),
            hsva,
            color,
            seen: color,
            group: 0,
            h: f64::from(hsva.h) * 360.0,
            s: f64::from(hsva.s) * 100.0,
            b: f64::from(hsva.v) * 100.0,
            r: f64::from(color.r()),
            g: f64::from(color.g()),
            blue: f64::from(color.b()),
            nr: f64::from(color.r()) / 255.0,
            ng: f64::from(color.g()) / 255.0,
            nb: f64::from(color.b()) / 255.0,
        }
    }

    /// 从颜色重建：HSV 反推 + 全部输入框刷新（RGB / 归一化 / 十六进制改色后调用）。
    fn sync_from_color(&mut self, color: Color32) {
        self.hsva = hsva_of(color);
        self.color = color;
        self.seen = color;
        self.hex = hex_text(color);
        self.refresh();
    }

    /// 按真值刷新输入框的显示值。HSB 取自 HSV（保留精度），RGB 与归一化取自颜色。
    fn refresh(&mut self) {
        self.h = f64::from(self.hsva.h) * 360.0;
        self.s = f64::from(self.hsva.s) * 100.0;
        self.b = f64::from(self.hsva.v) * 100.0;
        self.r = f64::from(self.color.r());
        self.g = f64::from(self.color.g());
        self.blue = f64::from(self.color.b());
        self.nr = f64::from(self.color.r()) / 255.0;
        self.ng = f64::from(self.color.g()) / 255.0;
        self.nb = f64::from(self.color.b()) / 255.0;
    }
}

/// 常驻的颜色选择器：直接画在当前面板里，改任一数值或拖色域方块即可改色。
pub fn picker(ui: &mut egui::Ui, id: &str, color: &mut Color32) {
    let state_id = egui::Id::new(("pdfbatch-palette-state", id));
    let mut state: PaletteState = ui
        .data_mut(|data| data.get_temp(state_id))
        .unwrap_or_else(|| PaletteState::new(*color));
    // 颜色被别处改动（含首次打开）时重建；否则一律保留用户输入中的数值。
    if state.seen != *color {
        state.sync_from_color(*color);
    }

    let mut hsva_edited = false;
    let mut color_edited = false;

    ui.set_min_width(372.0);
    ui.horizontal_top(|ui| {
        hsva_edited |= sv_square(ui, &mut state.hsva);
        hsva_edited |= hue_bar(ui, &mut state.hsva);
        ui.add_space(10.0);
        egui::Grid::new(("pdfbatch-palette-groups", id))
            .num_columns(2)
            .spacing([14.0, 4.0])
            .show(ui, |ui| {
                hsva_edited |= hsb_column(ui, &mut state);
                color_edited |= rgb_column(ui, &mut state);
                ui.end_row();
            });
    });

    // HSV 侧改动：颜色与其余数值跟着走（HSB 的输入值本身保持不变）。
    if hsva_edited {
        state.color = Color32::from(state.hsva);
        state.seen = state.color;
        state.hex = hex_text(state.color);
        state.refresh();
    }

    // 归一化与十六进制是颜色侧输入。
    color_edited |= normalized_row(ui, &mut state);
    let hex_changed = hex_row(ui, &mut state);
    if hex_changed {
        color_edited = true;
    }
    if color_edited {
        // 颜色侧改动：反推 HSV 并刷新全部数值。
        state.sync_from_color(state.color);
    }

    if hsva_edited || color_edited {
        *color = state.color;
    }
    ui.data_mut(|data| data.insert_temp(state_id, state));
}

/// 色域方块：横轴饱和度、纵轴明度；标记是 Photoshop 那样的细白圆环。
fn sv_square(ui: &mut egui::Ui, hsva: &mut Hsva) -> bool {
    const STEPS_X: usize = 32;
    const STEPS_Y: usize = 24;
    let (rect, response) = ui.allocate_exact_size(vec2(SV_WIDTH, SV_HEIGHT), Sense::click_and_drag());
    let t = Tokens::get(ui.ctx());
    let painter = ui.painter();

    // 用顶点色插值画出整块色域（不用纹理，省掉一次上传）。
    let mut mesh = Mesh::default();
    for row in 0..=STEPS_Y {
        let value = 1.0 - row as f32 / STEPS_Y as f32;
        for column in 0..=STEPS_X {
            let saturation = column as f32 / STEPS_X as f32;
            mesh.vertices.push(Vertex {
                pos: pos2(
                    rect.left() + saturation * rect.width(),
                    rect.top() + (1.0 - value) * rect.height(),
                ),
                uv: WHITE_UV,
                color: Color32::from(Hsva::new(hsva.h, saturation, value, 1.0)),
            });
        }
    }
    let stride = STEPS_X as u32 + 1;
    for row in 0..STEPS_Y as u32 {
        for column in 0..STEPS_X as u32 {
            let top_left = row * stride + column;
            let top_right = top_left + 1;
            let bottom_left = top_left + stride;
            let bottom_right = bottom_left + 1;
            mesh.add_triangle(top_left, top_right, bottom_left);
            mesh.add_triangle(top_right, bottom_right, bottom_left);
        }
    }
    painter.add(Shape::mesh(mesh));
    painter.rect_stroke(rect, CornerRadius::ZERO, Stroke::new(1.0, t.border), StrokeKind::Inside);

    let marker = pos2(
        rect.left() + hsva.s * rect.width(),
        rect.top() + (1.0 - hsva.v) * rect.height(),
    );
    painter.circle_stroke(marker, 4.5, Stroke::new(1.4, Color32::WHITE));
    painter.circle_stroke(marker, 4.5, Stroke::new(0.6, Color32::from_black_alpha(120)));

    let mut changed = false;
    if let Some(position) = response.interact_pointer_pos() {
        let saturation = ((position.x - rect.left()) / rect.width()).clamp(0.0, 1.0);
        let value = (1.0 - (position.y - rect.top()) / rect.height()).clamp(0.0, 1.0);
        if (saturation - hsva.s).abs() > f32::EPSILON || (value - hsva.v).abs() > f32::EPSILON {
            hsva.s = saturation;
            hsva.v = value;
            changed = true;
        }
    }
    changed
}

/// 竖直色相条：0° 红在上；滑块是 Photoshop 那样的左右两个小三角。
fn hue_bar(ui: &mut egui::Ui, hsva: &mut Hsva) -> bool {
    const SEGMENTS: usize = 48;
    let (rect, response) = ui.allocate_exact_size(vec2(HUE_WIDTH + HUE_PADDING, SV_HEIGHT), Sense::click_and_drag());
    let t = Tokens::get(ui.ctx());
    let painter = ui.painter();
    let bar = Rect::from_min_size(
        pos2(rect.center().x - HUE_WIDTH / 2.0, rect.top()),
        vec2(HUE_WIDTH, rect.height()),
    );

    let mut mesh = Mesh::default();
    for index in 0..=SEGMENTS {
        let hue = index as f32 / SEGMENTS as f32;
        let color = Color32::from(Hsva::new(hue, 1.0, 1.0, 1.0));
        let y = bar.top() + hue * bar.height();
        mesh.vertices.push(Vertex { pos: pos2(bar.left(), y), uv: WHITE_UV, color });
        mesh.vertices.push(Vertex { pos: pos2(bar.right(), y), uv: WHITE_UV, color });
    }
    for index in 0..SEGMENTS as u32 {
        let top_left = index * 2;
        mesh.add_triangle(top_left, top_left + 1, top_left + 2);
        mesh.add_triangle(top_left + 1, top_left + 3, top_left + 2);
    }
    painter.add(Shape::mesh(mesh));
    painter.rect_stroke(bar, CornerRadius::ZERO, Stroke::new(1.0, t.border), StrokeKind::Inside);

    // 左右两个小三角指示当前色相（Photoshop 的样式）。
    let y = bar.top() + hsva.h * bar.height();
    let fill = Color32::from_gray(235);
    let edge = Stroke::new(1.0, Color32::from_black_alpha(160));
    painter.add(Shape::convex_polygon(
        vec![
            pos2(bar.left() - 7.0, y),
            pos2(bar.left() - 1.0, y - 5.0),
            pos2(bar.left() - 1.0, y + 5.0),
        ],
        fill,
        edge,
    ));
    painter.add(Shape::convex_polygon(
        vec![
            pos2(bar.right() + 7.0, y),
            pos2(bar.right() + 1.0, y - 5.0),
            pos2(bar.right() + 1.0, y + 5.0),
        ],
        fill,
        edge,
    ));

    let mut changed = false;
    if let Some(position) = response.interact_pointer_pos() {
        let hue = ((position.y - bar.top()) / bar.height()).clamp(0.0, 1.0);
        if (hue - hsva.h).abs() > f32::EPSILON {
            hsva.h = hue;
            changed = true;
        }
    }
    changed
}

/// HSB 组（H 0–360 度、S/B 0–100%）。
fn hsb_column(ui: &mut egui::Ui, state: &mut PaletteState) -> bool {
    let mut changed = false;
    ui.vertical(|ui| {
        changed |= value_row(ui, &mut state.group, 0, "hsb-h", "H", &mut state.h, "度", 0.0..=360.0);
        changed |= value_row(ui, &mut state.group, 0, "hsb-s", "S", &mut state.s, "%", 0.0..=100.0);
        changed |= value_row(ui, &mut state.group, 0, "hsb-b", "B", &mut state.b, "%", 0.0..=100.0);
    });
    if changed {
        state.hsva.h = (state.h / 360.0) as f32;
        state.hsva.s = (state.s / 100.0) as f32;
        state.hsva.v = (state.b / 100.0) as f32;
    }
    changed
}

/// RGB 组（0–255）。
fn rgb_column(ui: &mut egui::Ui, state: &mut PaletteState) -> bool {
    let mut changed = false;
    ui.vertical(|ui| {
        changed |= value_row(ui, &mut state.group, 1, "rgb-r", "R", &mut state.r, "", 0.0..=255.0);
        changed |= value_row(ui, &mut state.group, 1, "rgb-g", "G", &mut state.g, "", 0.0..=255.0);
        changed |= value_row(ui, &mut state.group, 1, "rgb-b", "B", &mut state.blue, "", 0.0..=255.0);
    });
    if changed {
        state.color = Color32::from_rgb(state.r as u8, state.g as u8, state.blue as u8);
    }
    changed
}

/// 归一化组（0–1，三个输入框横排）。
fn normalized_row(ui: &mut egui::Ui, state: &mut PaletteState) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        radio_dot(ui, &mut state.group, 2);
        ui.label("归一化");
        changed |= number(ui, "norm-r", &mut state.nr, 0.005, 0.0..=1.0, 3);
        changed |= number(ui, "norm-g", &mut state.ng, 0.005, 0.0..=1.0, 3);
        changed |= number(ui, "norm-b", &mut state.nb, 0.005, 0.0..=1.0, 3);
        ui.label(egui::RichText::new("0–1").color(Tokens::get(ui.ctx()).text_muted));
    });
    if changed {
        let to_byte = |value: f64| (value.clamp(0.0, 1.0) * 255.0).round() as u8;
        state.color = Color32::from_rgb(to_byte(state.nr), to_byte(state.ng), to_byte(state.nb));
    }
    changed
}

/// 十六进制输入（Photoshop 用小写）。
fn hex_row(ui: &mut egui::Ui, state: &mut PaletteState) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.label("#");
        let response = ui.push_id("palette-hex", |ui| {
            ui.add(egui::TextEdit::singleline(&mut state.hex).desired_width(110.0).hint_text("000000"))
        });
        if response.inner.changed() {
            if let Some(parsed) = parse_hex(&state.hex) {
                state.color = parsed;
                changed = true;
            }
        }
    });
    changed
}

/// 数值组里的一行：组选择圆点 + 标签 + 输入框 + 单位。
fn value_row(
    ui: &mut egui::Ui,
    group: &mut u8,
    this_group: u8,
    key: &str,
    tag: &str,
    value: &mut f64,
    suffix: &str,
    range: std::ops::RangeInclusive<f64>,
) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        radio_dot(ui, group, this_group);
        ui.label(tag);
        changed = number(ui, key, value, 1.0, range, 0);
        if !suffix.is_empty() {
            ui.label(egui::RichText::new(suffix).color(Tokens::get(ui.ctx()).text_muted));
        }
    });
    changed
}

/// 一个数值输入框。显式给 id，保证同一面板里多个输入框互不串扰。
fn number(
    ui: &mut egui::Ui,
    key: &str,
    value: &mut f64,
    speed: f64,
    range: std::ops::RangeInclusive<f64>,
    decimals: usize,
) -> bool {
    ui.push_id(key, |ui| {
        ui.add(
            DragValue::new(value)
                .speed(speed)
                .range(range)
                .fixed_decimals(decimals),
        )
        .changed()
    })
    .inner
}

/// 组选择圆点（Photoshop 拾色器样式）：点击即把该组标为当前组。
fn radio_dot(ui: &mut egui::Ui, group: &mut u8, this_group: u8) {
    let t = Tokens::get(ui.ctx());
    let selected = *group == this_group;
    let (rect, response) = ui.allocate_exact_size(vec2(14.0, 14.0), Sense::click());
    if response.clicked() {
        *group = this_group;
    }
    let center = rect.center();
    ui.painter()
        .circle_stroke(center, 5.5, Stroke::new(1.0, if selected { t.accent } else { t.text_muted }));
    if selected {
        ui.painter().circle_filled(center, 2.8, t.accent);
    }
}

/// sRGB 字节值转 HSV。
fn hsva_of(color: Color32) -> Hsva {
    Hsva::from_srgba_unmultiplied([color.r(), color.g(), color.b(), 255])
}

/// 颜色的十六进制写法（直接取 sRGB 分量）。
fn hex_text(color: Color32) -> String {
    format!("{:02x}{:02x}{:02x}", color.r(), color.g(), color.b())
}

/// 解析十六进制写法：`#RRGGBB`、`RRGGBB`、`#RGB`。
fn parse_hex(text: &str) -> Option<Color32> {
    let digits = text.trim().trim_start_matches('#');
    let byte = |slice: &str| u8::from_str_radix(slice, 16).ok();
    match digits.len() {
        6 => Some(Color32::from_rgb(
            byte(&digits[0..2])?,
            byte(&digits[2..4])?,
            byte(&digits[4..6])?,
        )),
        3 => {
            let expand = |ch: char| u8::from_str_radix(&format!("{ch}{ch}"), 16).ok();
            let mut chars = digits.chars();
            Some(Color32::from_rgb(
                expand(chars.next()?)?,
                expand(chars.next()?)?,
                expand(chars.next()?)?,
            ))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_round_trips() {
        let color = Color32::from_rgb(0x94, 0x90, 0xB4);
        assert_eq!(hex_text(color), "9490b4");
        assert_eq!(parse_hex("#9490b4"), Some(color));
        assert_eq!(parse_hex("9490B4"), Some(color));
        assert_eq!(parse_hex("#fff"), Some(Color32::from_rgb(0xFF, 0xFF, 0xFF)));
    }

    #[test]
    fn malformed_hex_is_rejected() {
        assert!(parse_hex("#GGGGGG").is_none());
        assert!(parse_hex("#12345").is_none());
        assert!(parse_hex("").is_none());
        assert!(parse_hex("#1234567").is_none());
    }

    #[test]
    fn hsv_round_trips_through_srgb_bytes() {
        for color in [
            Color32::from_rgb(0x94, 0x90, 0xB4),
            Color32::from_rgb(0x00, 0x00, 0x00),
            Color32::from_rgb(0xFF, 0xFF, 0xFF),
            Color32::from_rgb(0x1B, 0x63, 0xE0),
        ] {
            let back = Color32::from(hsva_of(color));
            assert_eq!(back, color, "HSV 往返应当无损");
        }
    }

    /// 各输入框的初始值应当等于颜色换算出来的值。
    #[test]
    fn state_starts_in_sync_with_the_color() {
        let color = Color32::from_rgb(0x94, 0x90, 0xB4);
        let state = PaletteState::new(color);
        assert_eq!(state.h, f64::from(hsva_of(color).h) * 360.0);
        assert_eq!(state.r, 0x94 as f64);
        assert!((state.nr - 0x94 as f64 / 255.0).abs() < 1e-9);
        assert_eq!(hex_text(state.color), "9490b4");
    }

    /// HSB 输入不该被颜色反推覆盖：纯黑时色相仍要保留在输入框里。
    #[test]
    fn hue_survives_when_value_reaches_zero() {
        let mut state = PaletteState::new(Color32::from_rgb(0xFF, 0x00, 0x00));
        state.h = 240.0;
        state.hsva.h = (state.h / 360.0) as f32;
        state.b = 0.0;
        state.hsva.v = 0.0;
        state.color = Color32::from(state.hsva);
        state.refresh();
        assert!((state.h - 240.0).abs() < 1e-3, "明度归零后色相输入值不能被抹掉：{}", state.h);
    }
}
