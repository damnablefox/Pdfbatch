//! 书签标签页：文本输入、高级选项与解析预览树。

use eframe::egui;

use crate::app::App;
use crate::helpers::{danger, preview_one};
use crate::theme::{medium, regular, Tokens};
use crate::widgets;

/// 书签文本的示例（"填入示例"按钮用）。
const SAMPLE: &str = "# 每行一条书签；缩进（Tab 或两个空格）表示层级\n# 标题与页码之间用 Tab 或“ | ”分隔；页码从 1 开始\n第一章 绪论 | 1\n\t1.1 研究背景 | 2\n\t1.2 研究意义 | 3\n第二章 方法 | 10\n";

impl App {
    pub(crate) fn bookmark_params(&mut self, ui: &mut egui::Ui, t: Tokens) {
        widgets::section_title(ui, "书签文本");
        ui.label(
            egui::RichText::new("每行一条；缩进（Tab 或两个空格）表示层级；标题与页码用 Tab 或“ | ”分隔，页码从 1 开始。")
                .font(regular(11.5))
                .color(t.text_muted),
        );
        ui.add_space(4.0);
        ui.horizontal_wrapped(|ui| {
            if widgets::pill_button(ui, "选择 .txt 文件", false).clicked() {
                self.ask_spec(ui.ctx());
            }
            if widgets::pill_button(ui, "填入示例", false).clicked() {
                self.bookmarks.text = SAMPLE.to_string();
                self.bookmarks.source = None;
            }
            if widgets::pill_button(ui, "清空", false).clicked() {
                self.bookmarks.text.clear();
                self.bookmarks.source = None;
            }
        });
        if let Some(source) = &self.bookmarks.source {
            ui.add_space(2.0);
            ui.label(egui::RichText::new(format!("来源：{}", source.display())).font(regular(11.0)).color(t.text_faint));
        }
        ui.add_space(6.0);
        egui::ScrollArea::vertical()
            .id_salt("pdfbatch-bookmark-text")
            .max_height(200.0)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.add(
                    egui::TextEdit::multiline(&mut self.bookmarks.text)
                        .desired_rows(10)
                        .desired_width(f32::INFINITY)
                        .font(egui::TextStyle::Monospace)
                        .hint_text("# 注释\n第一章 绪论 | 1\n\t1.1 研究背景 | 2"),
                );
            });

        widgets::section_title(ui, "选项");
        ui.checkbox(&mut self.bookmarks.replace, "先删除原有书签");
        ui.checkbox(&mut self.bookmarks.strict, "严格模式（缩进跳级、页码越界一律报错）");
        ui.checkbox(&mut self.bookmarks.zero_based, "页码从 0 开始");

        widgets::section_title(ui, "解析预览");
        self.bookmark_preview(ui, t);
    }

    /// 书签解析预览：没有段头时显示一棵树；有段头时按段分组，并标出未覆盖的文件。
    fn bookmark_preview(&mut self, ui: &mut egui::Ui, t: Tokens) {
        use pdfbatch_core::spec::{SectionTarget, SpecSections};
        match pdfbatch_core::spec::split_sections(&self.bookmarks.text) {
            Err(error) => {
                ui.label(egui::RichText::new(error.to_string()).font(regular(11.5)).color(danger()));
            }
            Ok(SpecSections::Shared(text)) => {
                if !text.trim().is_empty() {
                    ui.label(
                        egui::RichText::new("模式：无段头 → 应用到所有文件")
                            .font(regular(11.0))
                            .color(t.text_muted),
                    );
                    ui.add_space(2.0);
                }
                match pdfbatch_core::spec::parse_spec(&text, self.bookmarks.zero_based, self.bookmarks.strict, usize::MAX) {
                    Ok(parsed) => preview_one(ui, &parsed, t),
                    Err(error) => {
                        ui.label(egui::RichText::new(error.to_string()).font(regular(11.5)).color(danger()));
                    }
                }
            }
            Ok(SpecSections::Sections(sections)) => {
                ui.label(
                    egui::RichText::new(format!("模式：分段（{} 段）→ 按段头匹配文件", sections.len()))
                        .font(regular(11.0))
                        .color(t.text_muted),
                );
                ui.add_space(2.0);
                let inputs = self.files.to_inputs();
                match pdfbatch_core::spec::bind_sections(&sections, &inputs) {
                    Err(message) => {
                        ui.label(egui::RichText::new(message).font(regular(11.5)).color(danger()));
                    }
                    Ok(binding) => {
                        for section in &sections {
                            let header = match &section.target {
                                SectionTarget::Name(name) => format!("文件：{name}"),
                                SectionTarget::Index(index) => format!("文件 #{index}"),
                            };
                            ui.add_space(3.0);
                            ui.label(egui::RichText::new(header).font(medium(12.0)).color(t.text));
                            match pdfbatch_core::spec::parse_spec(
                                &section.text,
                                self.bookmarks.zero_based,
                                self.bookmarks.strict,
                                usize::MAX,
                            ) {
                                Ok(parsed) => preview_one(ui, &parsed, t),
                                Err(error) => {
                                    ui.label(egui::RichText::new(error.to_string()).font(regular(11.5)).color(danger()));
                                }
                            }
                        }
                        for &index in &binding.uncovered {
                            let name = inputs[index]
                                .path
                                .file_name()
                                .map(|name| name.to_string_lossy().into_owned())
                                .unwrap_or_default();
                            ui.add_space(2.0);
                            ui.label(
                                egui::RichText::new(format!("未覆盖：{name}（将跳过）"))
                                    .font(regular(11.0))
                                    .color(t.text_faint),
                            );
                        }
                    }
                }
            }
        }
    }
}
