//! 各面板共用的自由函数：布局辅助、单位换算、书签预览与状态配色。

use eframe::egui;

use pdfbatch_core::spec::SpecEntry;
use pdfbatch_core::units::Unit;

use crate::files::FileStatus;
use crate::job;
use crate::theme::{regular, Tokens};

/// 一行「标签 + 控件」：标签与控件都画在同一固定高度的矩形内并垂直居中，
/// 因此同一行的文字基线一致；行内控件数量变化（例如勾选后才出现的下拉框）
/// 也不会改变行高，面板高度因此稳定。
pub(crate) fn labeled_row<R>(ui: &mut egui::Ui, label: &str, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    let row_height = ui.spacing().interact_size.y;
    let label_width = ui
        .painter()
        .layout_no_wrap(label.to_owned(), regular(13.0), egui::Color32::WHITE)
        .size()
        .x
        .max(48.0);
    ui.horizontal(|ui| {
        ui.set_min_height(row_height);
        ui.add_sized([label_width, row_height], egui::Label::new(label).selectable(false));
        ui.scope(add).inner
    })
    .inner
}

/// 输入框宽度：随文本内容增长，但不少于 `min`（空内容时按下限显示提示文字）。
pub(crate) fn text_field_width(ui: &egui::Ui, text: &str, min: f32) -> f32 {
    let measured = ui
        .painter()
        .layout_no_wrap(text.to_owned(), regular(13.0), egui::Color32::WHITE)
        .size()
        .x;
    (measured + 24.0).max(min)
}

/// 「宽 × 高」两格输入（按宽高匹配时用）；数值按界面上的「尺寸单位」解释。
pub(crate) fn size_row(ui: &mut egui::Ui, size: &mut [f64; 2], unit: Unit) {
    egui::Grid::new("fill-size").num_columns(2).spacing([8.0, 6.0]).show(ui, |ui| {
        ui.label(format!("宽（{}）", unit.canonical()));
        ui.add(egui::DragValue::new(&mut size[0]).speed(1.0).range(0.0..=20000.0));
        ui.end_row();
        ui.label(format!("高（{}）", unit.canonical()));
        ui.add(egui::DragValue::new(&mut size[1]).speed(1.0).range(0.0..=20000.0));
        ui.end_row();
    });
}

/// 「左下起点 + 宽高」四格输入（按完整矩形匹配/修改时用）；数值按「尺寸单位」解释。
pub(crate) fn rect_rows(ui: &mut egui::Ui, id: &str, rect: &mut [f64; 4], unit: Unit) {
    egui::Grid::new(id).num_columns(2).spacing([8.0, 6.0]).show(ui, |ui| {
        ui.label(format!("X（左，{}）", unit.canonical()));
        ui.add(egui::DragValue::new(&mut rect[0]).speed(1.0).range(0.0..=20000.0));
        ui.end_row();
        ui.label(format!("Y（下，{}）", unit.canonical()));
        ui.add(egui::DragValue::new(&mut rect[1]).speed(1.0).range(0.0..=20000.0));
        ui.end_row();
        ui.label(format!("宽（{}）", unit.canonical()));
        ui.add(egui::DragValue::new(&mut rect[2]).speed(1.0).range(-20000.0..=20000.0));
        ui.end_row();
        ui.label(format!("高（{}）", unit.canonical()));
        ui.add(egui::DragValue::new(&mut rect[3]).speed(1.0).range(-20000.0..=20000.0));
        ui.end_row();
    });
}

/// 尺寸单位选择行；返回是否发生了切换。
///
/// 切换后调用方必须调 `convert_units` 把已填的尺寸按物理量等值换算，
/// 否则用户会看到「换成 mm 后矩形突然变了」。
pub(crate) fn unit_row(ui: &mut egui::Ui, unit: &mut Unit) -> bool {
    let mut changed = false;
    labeled_row(ui, "单位", |ui| {
        for value in [Unit::Pt, Unit::Mm, Unit::Cm, Unit::In] {
            if ui.selectable_value(unit, value, value.canonical()).changed() {
                changed = true;
            }
        }
    });
    changed
}

/// 切换单位：已填的尺寸与尺寸容差按物理量等值换算，实际矩形不受影响。
pub(crate) fn convert_units(fill: &mut job::FillDraft, from: Unit) {
    let to = fill.unit;
    if from == to {
        return;
    }
    let convert = |value: f64| to.from_pt(from.to_pt(value));
    for value in fill.rect.iter_mut() {
        *value = convert(*value);
    }
    for value in fill.match_size.iter_mut() {
        *value = convert(*value);
    }
    for value in fill.match_rect_value.iter_mut() {
        *value = convert(*value);
    }
    for value in fill.change_rect_value.iter_mut() {
        *value = convert(*value);
    }
    fill.rect_tolerance = convert(fill.rect_tolerance);
}

/// 预览一段书签树（空段与警告一并显示）。
pub(crate) fn preview_one(ui: &mut egui::Ui, parsed: &pdfbatch_core::spec::ParsedSpec, t: Tokens) {
    if parsed.entries.is_empty() {
        ui.label(egui::RichText::new("（还没有书签）").font(regular(11.5)).color(t.text_faint));
    } else {
        preview_tree(ui, &parsed.entries, 0, t);
    }
    for warning in &parsed.warnings {
        ui.add_space(2.0);
        ui.label(egui::RichText::new(warning).font(regular(11.0)).color(warn()));
    }
}

/// 书签树的缩进预览（缩进用 `depth * 16`，与 PdfCraft 的书签面板同风格）。
pub(crate) fn preview_tree(ui: &mut egui::Ui, entries: &[SpecEntry], depth: usize, t: Tokens) {
    for entry in entries {
        ui.horizontal(|ui| {
            ui.add_space(depth as f32 * 16.0);
            ui.label(egui::RichText::new(&entry.title).font(regular(12.5)));
            ui.label(egui::RichText::new(format!("第 {} 页", entry.page + 1)).font(regular(11.0)).color(t.text_faint));
        });
        preview_tree(ui, &entry.children, depth + 1, t);
    }
}

pub(crate) fn counts(statuses: &[FileStatus]) -> (usize, usize, usize) {
    let mut done = 0;
    let mut skipped = 0;
    let mut failed = 0;
    for status in statuses {
        match status {
            FileStatus::Done(_) => done += 1,
            FileStatus::Skipped(_) => skipped += 1,
            FileStatus::Failed(_) => failed += 1,
            FileStatus::Pending | FileStatus::Running => {}
        }
    }
    (done, skipped, failed)
}

pub(crate) fn danger() -> egui::Color32 {
    egui::Color32::from_rgb(0xC0, 0x39, 0x2B)
}

pub(crate) fn warn() -> egui::Color32 {
    egui::Color32::from_rgb(0xB4, 0x6A, 0x00)
}

pub(crate) fn ok() -> egui::Color32 {
    egui::Color32::from_rgb(0x1E, 0x8E, 0x3E)
}
