//! 页面尺寸标签页：预设 / 自定义尺寸、纸张方向、缩放模式与扫描件适配。

use eframe::egui;

use pdfbatch_core::pagesize::{Orientation, ScaleMode};
use pdfbatch_core::units::Unit;

use crate::app::App;
use crate::helpers::{danger, labeled_row};
use crate::job::{self, PageSizeDraft, SizeMethod, SizeSource};
use crate::theme::{regular, Tokens};
use crate::widgets;

impl App {
    /// 「页面尺寸」模式的参数：先选方式，再填各自参数，最后是作用范围。
    pub(crate) fn pagesize_params(&mut self, ui: &mut egui::Ui, t: Tokens) {
        // 参数改动可能让当前模式不再可用（切到预设、或把某一维改成 0）：自动切到可用项，
        // 具体原因由下面的 `mode_restriction` 常驻显示。
        job::legalize(&mut self.pagesize);
        widgets::section_title(ui, "方式");
        ui.horizontal(|ui| {
            for method in SizeMethod::ALL {
                ui.selectable_value(&mut self.pagesize.method, method, method.label());
            }
        });
        ui.add_space(6.0);

        match self.pagesize.method {
            SizeMethod::Scale => self.pagesize_scale(ui, t),
            SizeMethod::Scan => self.pagesize_scan(ui, t),
        }

        widgets::section_title(ui, "作用范围");
        ui.horizontal(|ui| {
            ui.label("页范围");
            ui.add(
                egui::TextEdit::singleline(&mut self.pagesize.pages)
                    .hint_text("all、1-5,8、odd、even、all except first,last")
                    .desired_width(180.0),
            );
        });
        if let Err(message) = job::validate_page_syntax(&self.pagesize.pages) {
            ui.label(egui::RichText::new(message).font(regular(11.5)).color(danger()));
        }
        ui.add_space(8.0);
        ui.label(
            egui::RichText::new("这些参数会应用到左侧列表里的所有文件。")
                .font(regular(11.5))
                .color(t.text_faint),
        );
    }

    /// 缩放方式的参数。
    fn pagesize_scale(&mut self, ui: &mut egui::Ui, t: Tokens) {
        widgets::section_title(ui, "目标尺寸");
        ui.horizontal(|ui| {
            ui.selectable_value(&mut self.pagesize.source, SizeSource::Preset, "预设");
            ui.selectable_value(&mut self.pagesize.source, SizeSource::Custom, "自定义");
        });
        match self.pagesize.source {
            SizeSource::Preset => {
                // 预设：纸张 → 纸张方向 → 纸张示意。
                labeled_row(ui, "纸张", |ui| {
                    egui::ComboBox::from_id_salt("pagesize-preset")
                        .width(210.0)
                        .selected_text(paper_label(&self.pagesize.preset))
                        .show_ui(ui, |ui| {
                            for name in pdfbatch_core::units::preset_names() {
                                ui.selectable_value(
                                    &mut self.pagesize.preset,
                                    name.to_string(),
                                    paper_label(name),
                                );
                            }
                        });
                });
                labeled_row(ui, "纸张方向", |ui| {
                    for (value, label) in [
                        (Orientation::Portrait, "纵向"),
                        (Orientation::Landscape, "横向"),
                        (Orientation::Auto, "跟随原页面"),
                    ] {
                        ui.selectable_value(&mut self.pagesize.orientation, value, label);
                    }
                });
                if let Some((w, h)) = pdfbatch_core::units::preset(&self.pagesize.preset) {
                    let mm = |pt: f64| pt / 72.0 * 25.4;
                    ui.label(
                        egui::RichText::new(format!(
                            "{}（{:.2} × {:.2} mm）",
                            self.pagesize.preset,
                            mm(w),
                            mm(h)
                        ))
                        .font(regular(11.0))
                        .color(t.text_faint),
                    );
                }
            }
            SizeSource::Custom => {
                // 自定义：宽度、高度、页面单位分别一行；任一为 0 表示"这一维不约束"。
                labeled_row(ui, "宽度", |ui| {
                    ui.add(
                        egui::DragValue::new(&mut self.pagesize.width)
                            .speed(1.0)
                            .range(0.0..=20000.0),
                    );
                });
                labeled_row(ui, "高度", |ui| {
                    ui.add(
                        egui::DragValue::new(&mut self.pagesize.height)
                            .speed(1.0)
                            .range(0.0..=20000.0),
                    );
                });
                labeled_row(ui, "页面单位", |ui| {
                    for unit in [Unit::Pt, Unit::Mm, Unit::Cm, Unit::In] {
                        ui.selectable_value(&mut self.pagesize.unit, unit, unit.canonical());
                    }
                });
            }
        }

        widgets::section_title(ui, "缩放模式");
        let allowed = job::allowed_modes(&self.pagesize);
        labeled_row(ui, "模式", |ui| {
            egui::ComboBox::from_id_salt("pagesize-mode")
                .width(210.0)
                .selected_text(self.pagesize.scale_mode.label())
                .show_ui(ui, |ui| {
                    for mode in ScaleMode::ALL {
                        // 当前尺寸设置下不可用的模式置灰（原因写在下面一行）。
                        let enabled = allowed.contains(&mode);
                        if ui
                            .add_enabled(
                                enabled,
                                egui::Button::selectable(
                                    self.pagesize.scale_mode == mode,
                                    mode.label(),
                                ),
                            )
                            .clicked()
                        {
                            self.pagesize.scale_mode = mode;
                        }
                    }
                });
        });
        if self.pagesize.scale_mode == ScaleMode::CenterScaled {
            labeled_row(ui, "缩放比例", |ui| {
                ui.add(
                    egui::DragValue::new(&mut self.pagesize.percent)
                        .speed(1.0)
                        .range(1.0..=1000.0)
                        .suffix("%"),
                );
            });
        }
        // 把"哪些模式为什么点不了"直接写在界面上，别让人猜。
        if let Some(why) = mode_restriction(&self.pagesize) {
            ui.label(egui::RichText::new(why).font(regular(11.0)).color(t.text_faint));
        }
    }

    /// 扫描件适配方式的参数。
    fn pagesize_scan(&mut self, ui: &mut egui::Ui, t: Tokens) {
        widgets::section_title(ui, "扫描件适配");
        ui.label(
            egui::RichText::new(
                "按图像像素与 PPI 算出整个文件统一的页面宽度，再对每一页应用「适应宽度」（高度随内容）。",
            )
            .font(regular(11.0))
            .color(t.text_muted),
        );
        ui.add_space(4.0);
        for (label, value) in [
            ("彩色图像 PPI", &mut self.pagesize.color_ppi),
            ("灰度图像 PPI", &mut self.pagesize.gray_ppi),
            ("位图图像 PPI", &mut self.pagesize.mono_ppi),
        ] {
            labeled_row(ui, label, |ui| {
                ui.add(egui::DragValue::new(value).speed(1.0).range(1.0..=2400.0));
            });
        }
        ui.label(
            egui::RichText::new("位图（1 位黑白）默认给更高的 PPI，避免把线稿放大糊掉。")
                .font(regular(11.0))
                .color(t.text_faint),
        );
        ui.label(
            egui::RichText::new("各页宽度统一、高度随内容；实际 PPI 只会不小于设定值。")
                .font(regular(11.0))
                .color(t.text_faint),
        );
    }
}

/// 纸张下拉项的文本：名称 + 毫米尺寸（与"纸张示意"一致，方便直接对照实物）。
fn paper_label(name: &str) -> String {
    match pdfbatch_core::units::preset(name) {
        Some((w, h)) => {
            let mm = |pt: f64| pt / 72.0 * 25.4;
            format!("{name}（{:.2} × {:.2} mm）", mm(w), mm(h))
        }
        None => name.to_string(),
    }
}

/// 当前尺寸设置下模式受限的原因；没有受限时返回 `None`。
///
/// 这一行是给用户看的「为什么有些模式点不了」，与 `job::allowed_modes` 的规则一一对应。
fn mode_restriction(draft: &PageSizeDraft) -> Option<String> {
    match draft.source {
        SizeSource::Preset => Some(
            "预设纸张下不提供「适应宽度 / 适应高度」：它们会改写输出页面的另一维，输出将不等于所选的纸。需要「不裁不留白」时请改用自定义尺寸（例如宽度 210、高度 0、模式选适应宽度）。"
                .to_string(),
        ),
        SizeSource::Custom => {
            let no_width = !(draft.width > 0.0);
            let no_height = !(draft.height > 0.0);
            match (no_width, no_height) {
                (false, false) => None,
                (true, false) => {
                    Some("宽度为 0：只有「适应高度」可用，输出宽度由内容决定。".to_string())
                }
                (false, true) => {
                    Some("高度为 0：只有「适应宽度」可用，输出高度由内容决定。".to_string())
                }
                (true, true) => Some("宽度与高度不能同时为 0，请至少填写一个。".to_string()),
            }
        }
    }
}
