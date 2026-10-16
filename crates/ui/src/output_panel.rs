//! 输出选项面板：三种输出方式、文件名前后缀与预览、密码、整体重写、处理日志与失败即停。

use eframe::egui;

use pdfbatch_core::report::LogFormat;

use crate::app::App;
use crate::helpers::{danger, labeled_row, text_field_width};
use crate::job::{self, OutputChoice};
use crate::theme::{regular, Tokens};
use crate::widgets;

impl App {
    /// 输出选项：面板内始终渲染全部选项（当前输出方式用不到的置灰或换成说明），
    /// 三种输出方式的面板高度因此完全一致，切换时分隔线不会跳动。
    pub(crate) fn output_options(&mut self, ui: &mut egui::Ui, t: Tokens) {
        widgets::section_title(ui, "输出");
        ui.horizontal(|ui| {
            ui.selectable_value(&mut self.common.output, OutputChoice::SameFolder, "同目录另存");
            ui.selectable_value(&mut self.common.output, OutputChoice::Dir, "指定目录");
            ui.selectable_value(&mut self.common.output, OutputChoice::InPlace, "覆盖原文件");
        });
        ui.add_space(2.0);

        // 目录行始终存在：点「选择目录」会自动切到「指定目录」；其余方式下显示该方式的说明。
        let is_dir = self.common.output == OutputChoice::Dir;
        let chosen = self.common.dir.is_some();
        let dir_text = self
            .common
            .dir
            .as_ref()
            .map(|dir| dir.display().to_string())
            .unwrap_or_else(|| "未选择".to_string());
        let mut ask = false;
        // 「选择目录」只在「指定目录」模式下可用；其余方式置灰并说明原因。
        ui.horizontal(|ui| {
            let button = ui
                .add_enabled_ui(is_dir, |ui| widgets::pill_button(ui, "选择目录", false))
                .inner;
            if button.clicked() {
                ask = true;
            }
            if !is_dir {
                button.on_hover_text("只在「指定目录」模式下可用：请先切到「指定目录」");
            }
            let (text, color) = if is_dir {
                (dir_text, if chosen { t.text } else { t.text_faint })
            } else {
                let hint = match self.common.output {
                    OutputChoice::SameFolder => "另存到原文件所在目录，不改动原文件",
                    OutputChoice::InPlace => "会覆盖原文件（先写临时文件，再原子替换）",
                    OutputChoice::Dir => "",
                };
                (hint.to_string(), t.text_muted)
            };
            ui.add(
                egui::Label::new(egui::RichText::new(text).font(regular(11.5)).color(color)).truncate(),
            );
        });
        if ask {
            self.ask_output_dir(ui.ctx());
        }

        self.naming_row(ui, t);

        // 密码、整体重写、处理日志、失败即停：四项互不相关，各自占一行。
        labeled_row(ui, "密码", |ui| {
            ui.add(
                egui::TextEdit::singleline(&mut self.common.password)
                    .password(true)
                    .hint_text("加密 PDF 才需要")
                    .desired_width(150.0),
            );
        });
        ui.checkbox(&mut self.common.full_save, "整体重写（更小，可能使签名失效）");
        // 勾选框在前（与上一行的「整体重写」、下一行的「失败即停」同风格）；
        // 格式下拉始终留在行内（未勾选时置灰），行高因此不随勾选变化。
        ui.horizontal(|ui| {
            let mut logging = self.common.log.is_some();
            if ui.checkbox(&mut logging, "输出处理日志").changed() {
                self.common.log = logging.then_some(LogFormat::Csv);
            }
            let mut format = self.common.log.unwrap_or(LogFormat::Csv);
            ui.add_enabled_ui(self.common.log.is_some(), |ui| {
                egui::ComboBox::from_id_salt("pdfbatch-log-format")
                    .width(80.0)
                    .selected_text(format.canonical())
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut format, LogFormat::Csv, "csv");
                        ui.selectable_value(&mut format, LogFormat::Json, "json");
                    });
            });
            if self.common.log.is_some() {
                self.common.log = Some(format);
            }
            ui.label(
                egui::RichText::new("写到可执行文件同级目录")
                    .font(regular(11.0))
                    .color(t.text_faint),
            );
        });
        ui.checkbox(&mut self.common.stop_on_error, "失败即停");
        // 日志目录提示始终占一行，避免勾选日志时高度跳变。
        ui.add(
            egui::Label::new(
                egui::RichText::new(format!("日志目录：{}", log_dir_hint()))
                    .font(regular(11.0))
                    .color(t.text_faint),
            )
            .truncate(),
        );
    }

    /// 文件名前缀 / 后缀：始终渲染（覆盖原文件时置灰），保证输出面板高度恒定。
    fn naming_row(&mut self, ui: &mut egui::Ui, t: Tokens) {
        let naming_enabled = self.common.output != OutputChoice::InPlace;
        ui.add_enabled_ui(naming_enabled, |ui| {
            // 前缀与后缀各自一行：两个独立输入，不并排。
            let prefix_width = text_field_width(ui, &self.common.prefix, 80.0);
            labeled_row(ui, "文件名前缀", |ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut self.common.prefix)
                        .desired_width(prefix_width)
                        .hint_text("如 new_"),
                );
            });
            let suffix_width = text_field_width(ui, &self.common.suffix, 80.0);
            labeled_row(ui, "文件名后缀", |ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut self.common.suffix)
                        .desired_width(suffix_width)
                        .hint_text("如 _filled"),
                );
            });
        });

        let sample = self
            .files
            .items
            .first()
            .and_then(|item| item.input.path.file_stem().map(|stem| stem.to_string_lossy().into_owned()))
            .unwrap_or_else(|| "文件名".to_string());
        if naming_enabled {
            let naming = job::effective_naming(self.mode, &self.common);
            ui.label(
                egui::RichText::new(format!("预览：{}{}{}.pdf", naming.prefix, sample, naming.suffix))
                    .font(regular(11.5))
                    .color(t.text_muted),
            );
        } else {
            ui.label(
                egui::RichText::new(format!("预览：{sample}.pdf"))
                    .font(regular(11.5))
                    .color(t.text_faint),
            );
        }

        // 提示行始终占一行：覆盖原文件时说明不参与重命名，非法字符给红字，
        // 其余情况给出空前后缀的保护说明。
        let (message, color) = if !naming_enabled {
            ("覆盖原文件，不参与重命名".to_string(), t.text_faint)
        } else if let Err(error) = job::validate_naming(&self.common.prefix, &self.common.suffix) {
            (error, danger())
        } else {
            let both_empty = self.common.prefix.is_empty() && self.common.suffix.trim().is_empty();
            if both_empty && self.common.output == OutputChoice::SameFolder {
                (
                    format!("前后缀都为空，将使用默认后缀 {}，避免覆盖原文件", self.mode.suffix()),
                    t.text_faint,
                )
            } else {
                (String::new(), t.text_faint)
            }
        };
        ui.add(
            egui::Label::new(egui::RichText::new(message).font(regular(11.0)).color(color)).truncate(),
        );
    }
}

/// 处理日志的存放目录（可执行文件同级）。
fn log_dir_hint() -> String {
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.display().to_string()))
        .unwrap_or_else(|| ".".to_string())
}
