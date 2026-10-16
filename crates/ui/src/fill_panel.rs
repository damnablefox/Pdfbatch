//! 填充区域标签页：新增（样式 + 页范围）与删改（查找条件 + 删改模式 + 更改要求）。

use eframe::egui;

use pdfbatch_core::{Blend, Layer};

use crate::app::App;
use crate::helpers::{convert_units, danger, labeled_row, ok, rect_rows, size_row, unit_row, warn};
use crate::job::{self, ChangeRect, FillAction, FillEditMode, MatchRect};
use crate::palette;
use crate::theme::{regular, Tokens};
use crate::widgets;

impl App {
    pub(crate) fn fill_params(&mut self, ui: &mut egui::Ui, t: Tokens) {
        widgets::section_title(ui, "操作");
        ui.horizontal(|ui| {
            for action in FillAction::ALL {
                ui.selectable_value(&mut self.fill.action, action, action.label());
            }
        });
        ui.add_space(6.0);

        match self.fill.action {
            FillAction::Add => {
                self.fill_add_style(ui, t);
                self.fill_pages(ui, t);
                ui.add_space(8.0);
                ui.label(
                    egui::RichText::new("这些参数会应用到左侧列表里的所有文件。")
                        .font(regular(11.5))
                        .color(t.text_faint),
                );
            }
            FillAction::Edit => {
                // 上半部分：查找条件；下半部分：删改模式与（选「修改」时的）更改要求。
                widgets::section_title(ui, "尺寸单位");
                let before = self.fill.unit;
                if unit_row(ui, &mut self.fill.unit) {
                    convert_units(&mut self.fill, before);
                }
                self.fill_match_options(ui, t);
                self.fill_pages(ui, t);
                self.fill_probe_row(ui, t);

                widgets::section_title(ui, "删改模式");
                ui.horizontal(|ui| {
                    for mode in FillEditMode::ALL {
                        ui.selectable_value(&mut self.fill.edit_mode, mode, mode.label());
                    }
                });

                if self.fill.edit_mode == FillEditMode::Modify {
                    ui.add_space(10.0);
                    widgets::section_title(ui, "更改要求");
                    self.fill_change_options(ui, t);
                } else {
                    ui.add_space(8.0);
                    ui.label(
                        egui::RichText::new("查找后按「执行」删除命中的填充层。")
                            .font(regular(11.5))
                            .color(t.text_faint),
                    );
                }
            }
        }
    }

    /// 新增模式：要盖上去的填充层样式。
    fn fill_add_style(&mut self, ui: &mut egui::Ui, _t: Tokens) {
        widgets::section_title(ui, "区域");
        ui.horizontal(|ui| {
            ui.selectable_value(&mut self.fill.whole_page, true, "整页填充");
            ui.selectable_value(&mut self.fill.whole_page, false, "自定义矩形填充");
        });
        ui.add_space(6.0);

        if !self.fill.whole_page {
            let before = self.fill.unit;
            if unit_row(ui, &mut self.fill.unit) {
                convert_units(&mut self.fill, before);
            }
            let rect_ok = self.fill.rect[2] > 0.0 && self.fill.rect[3] > 0.0;
            rect_rows(ui, "fill-rect", &mut self.fill.rect, self.fill.unit);
            if !rect_ok {
                ui.label(egui::RichText::new("宽和高必须大于 0").font(regular(11.5)).color(danger()));
            }
        }
        ui.add_space(4.0);
        widgets::page_preview(ui, self.fill.rect, self.fill.color, self.fill.opacity as f32, self.fill.whole_page);

        widgets::section_title(ui, "颜色");
        // 拾色器常驻在面板里（不用弹层：弹层被点到别处会关闭，拖动色域时容易误触消失）。
        palette::picker(ui, "fill-color", &mut self.fill.color);
        labeled_row(ui, "不透明度", |ui| {
            ui.add(egui::Slider::new(&mut self.fill.opacity, 0.0..=1.0).fixed_decimals(2));
        });

        widgets::section_title(ui, "混合与层级");
        labeled_row(ui, "混合模式", |ui| {
            blend_combo(ui, "fill-blend", &mut self.fill.blend);
        });
        labeled_row(ui, "层级", |ui| {
            ui.selectable_value(&mut self.fill.layer, Layer::Top, "置于顶层");
            ui.selectable_value(&mut self.fill.layer, Layer::Bottom, "置于底层");
        });
    }

    /// 作用范围（三种操作共用）。
    fn fill_pages(&mut self, ui: &mut egui::Ui, _t: Tokens) {
        widgets::section_title(ui, "作用范围");
        labeled_row(ui, "页范围", |ui| {
            ui.add(
                egui::TextEdit::singleline(&mut self.fill.pages)
                    .hint_text("all、1-5,8,10-、all except first,last")
                    .desired_width(180.0),
            );
        });
        if let Err(message) = job::validate_page_syntax(&self.fill.pages) {
            ui.label(egui::RichText::new(message).font(regular(11.5)).color(danger()));
        }
    }

    /// 查找条件（删改模式）：填充区域（必选）、尺寸容差、颜色与色值容差、不透明度、混合模式、层级。
    fn fill_match_options(&mut self, ui: &mut egui::Ui, t: Tokens) {
        widgets::section_title(ui, "查找条件");
        ui.label(
            egui::RichText::new("「填充区域」必选，取「不限」即不限制尺寸；其余条件不勾选表示不限制。")
                .font(regular(11.0))
                .color(t.text_faint),
        );
        ui.add_space(4.0);
        ui.horizontal_wrapped(|ui| {
            ui.label("填充区域");
            for choice in MatchRect::ALL {
                ui.selectable_value(&mut self.fill.match_rect, choice, choice.label());
            }
        });
        match self.fill.match_rect {
            MatchRect::Size => {
                size_row(ui, &mut self.fill.match_size, self.fill.unit);
                self.fill_rect_tolerance_row(ui, t);
            }
            MatchRect::Rect => {
                rect_rows(ui, "fill-match-rect", &mut self.fill.match_rect_value, self.fill.unit);
                self.fill_rect_tolerance_row(ui, t);
            }
            MatchRect::Any | MatchRect::FullPage => {}
        }
        ui.checkbox(&mut self.fill.match_color_on, "填充颜色");
        if self.fill.match_color_on {
            palette::picker(ui, "fill-match-color", &mut self.fill.match_color);
        }
        ui.horizontal(|ui| {
            ui.checkbox(&mut self.fill.match_opacity_on, "不透明度");
            if self.fill.match_opacity_on {
                ui.add(egui::Slider::new(&mut self.fill.match_opacity, 0.0..=1.0).fixed_decimals(2));
            }
        });
        ui.horizontal(|ui| {
            ui.checkbox(&mut self.fill.match_blend_on, "混合模式");
            if self.fill.match_blend_on {
                blend_combo(ui, "fill-match-blend", &mut self.fill.match_blend);
            }
        });
        ui.horizontal(|ui| {
            ui.checkbox(&mut self.fill.match_layer_on, "层级");
            if self.fill.match_layer_on {
                ui.selectable_value(&mut self.fill.match_layer, Layer::Top, "顶层");
                ui.selectable_value(&mut self.fill.match_layer, Layer::Bottom, "底层");
            }
        });
        // 色值容差只对颜色与不透明度有意义（两者共用 0–1 归一化容差）。
        if self.fill.match_color_on || self.fill.match_opacity_on {
            labeled_row(ui, "色值容差", |ui| {
                ui.add(egui::DragValue::new(&mut self.fill.color_tolerance).speed(0.005).range(0.0..=1.0));
            });
            ui.label(
                egui::RichText::new("0–1 归一化，逐通道 ±；0 表示精确相等")
                    .font(regular(11.0))
                    .color(t.text_faint),
            );
        }
    }

    /// 尺寸容差：单位跟随「尺寸单位」，0 表示精确相等。
    fn fill_rect_tolerance_row(&mut self, ui: &mut egui::Ui, t: Tokens) {
        labeled_row(ui, "尺寸容差", |ui| {
            ui.add(egui::DragValue::new(&mut self.fill.rect_tolerance).speed(0.1).range(0.0..=f64::MAX));
            ui.label(egui::RichText::new(self.fill.unit.canonical()).color(t.text_muted));
        });
        ui.label(
            egui::RichText::new("0 表示精确相等；单位跟随上面的「尺寸单位」")
                .font(regular(11.0))
                .color(t.text_faint),
        );
    }

    /// 查找按钮与命中数量提示。
    fn fill_probe_row(&mut self, ui: &mut egui::Ui, t: Tokens) {
        ui.add_space(6.0);
        let busy = self.probe.is_some();
        let mut clicked = false;
        ui.horizontal(|ui| {
            ui.add_enabled_ui(!busy, |ui| {
                if widgets::pill_button(ui, "查找匹配", false).clicked() {
                    clicked = true;
                }
            });
            if busy {
                ui.label(egui::RichText::new("查找中…").font(regular(12.0)).color(t.text_muted));
            }
        });
        if let Some(report) = &self.probe_report {
            let fresh = report.fingerprint == job::matcher_fingerprint(&self.fill);
            let (text, color) = if !fresh {
                ("匹配条件已变，请重新查找".to_string(), t.text_faint)
            } else if report.total == 0 {
                ("没有匹配到填充层".to_string(), warn())
            } else {
                (format!("命中 {} 个填充层", report.total), ok())
            };
            ui.label(egui::RichText::new(text).font(regular(12.0)).color(color));
        }
        if clicked {
            self.start_probe(ui.ctx());
        }
    }

    /// 更改要求（改模式）：命中的填充层要改成什么样。
    fn fill_change_options(&mut self, ui: &mut egui::Ui, _t: Tokens) {
        ui.horizontal_wrapped(|ui| {
            ui.label("填充区域");
            for choice in ChangeRect::ALL {
                ui.selectable_value(&mut self.fill.change_rect, choice, choice.label());
            }
        });
        if self.fill.change_rect == ChangeRect::Rect {
            rect_rows(ui, "fill-change-rect", &mut self.fill.change_rect_value, self.fill.unit);
        }
        ui.checkbox(&mut self.fill.change_color_on, "改成颜色");
        if self.fill.change_color_on {
            palette::picker(ui, "fill-change-color", &mut self.fill.change_color);
        }
        ui.horizontal(|ui| {
            ui.checkbox(&mut self.fill.change_opacity_on, "改成不透明度");
            if self.fill.change_opacity_on {
                ui.add(egui::Slider::new(&mut self.fill.change_opacity, 0.0..=1.0).fixed_decimals(2));
            }
        });
        ui.horizontal(|ui| {
            ui.checkbox(&mut self.fill.change_blend_on, "改成混合模式");
            if self.fill.change_blend_on {
                blend_combo(ui, "fill-change-blend", &mut self.fill.change_blend);
            }
        });
        ui.horizontal(|ui| {
            ui.checkbox(&mut self.fill.change_layer_on, "改成层级");
            if self.fill.change_layer_on {
                ui.selectable_value(&mut self.fill.change_layer, Layer::Top, "顶层");
                ui.selectable_value(&mut self.fill.change_layer, Layer::Bottom, "底层");
            }
        });
    }
}

/// 混合模式下拉框（新增、查找、更改共用）。
fn blend_combo(ui: &mut egui::Ui, id: &str, value: &mut Blend) {
    egui::ComboBox::from_id_salt(id)
        .width(150.0)
        .selected_text(value.canonical())
        .show_ui(ui, |ui| {
            for name in pdfbatch_core::blend::BLEND_NAMES {
                if let Ok(blend) = Blend::parse(name) {
                    ui.selectable_value(value, blend, name);
                }
            }
        });
}
