//! 文件列表面板：增删、上下移、排序，以及提示 / 状态 / 路径列的渲染与双向滚动。

use eframe::egui;

use crate::app::App;
use crate::files::FileStatus;
use crate::helpers::{danger, ok, warn};
use crate::theme::{medium, regular, Tokens};
use crate::widgets;

/// 工具栏上被点击的动作。
enum ToolbarAction {
    AddFiles,
    AddFolder,
    Remove,
    Clear,
}

/// 表格一行预先算好的显示数据（避免在表格闭包里借用 `self`）。
struct RowData {
    id: u64,
    /// 行号（从 1 开始，随当前顺序变化）。
    number: usize,
    name: String,
    /// 提示列的文本与颜色。
    hint: String,
    hint_color: egui::Color32,
    path: String,
    status: String,
    color: egui::Color32,
}

impl App {
    pub(crate) fn files_area(&mut self, ui: &mut egui::Ui, t: Tokens) {
        ui.add_space(6.0);
        // 查找期间也视为忙：列表的增删、排序、上下移都先禁掉，避免与回填错位。
        let busy = self.run.is_some() || self.probe.is_some();
        // 工具栏：只记录被点击的动作，待闭包结束后再派发，避免借用冲突。
        let mut action: Option<ToolbarAction> = None;
        ui.horizontal(|ui| {
            ui.add_enabled_ui(!busy, |ui| {
                if widgets::pill_button(ui, "添加文件", false).clicked() {
                    action = Some(ToolbarAction::AddFiles);
                }
                if widgets::pill_button(ui, "添加文件夹", false).clicked() {
                    action = Some(ToolbarAction::AddFolder);
                }
                if widgets::pill_button(ui, "移除", false).clicked() {
                    action = Some(ToolbarAction::Remove);
                }
                if widgets::pill_button(ui, "清空", false).clicked() {
                    action = Some(ToolbarAction::Clear);
                }
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(egui::RichText::new(format!("共 {} 个文件", self.files.len())).font(regular(12.0)).color(t.text_muted));
            });
        });
        let ctx = ui.ctx().clone();
        match action {
            Some(ToolbarAction::AddFiles) => self.ask_files(&ctx),
            Some(ToolbarAction::AddFolder) => self.ask_folder(&ctx),
            Some(ToolbarAction::Remove) => self.files.remove_selected(),
            Some(ToolbarAction::Clear) => self.files.clear(),
            None => {}
        }

        ui.add_space(6.0);
        if self.files.is_empty() {
            ui.vertical_centered(|ui| {
                ui.add_space(40.0);
                ui.label(egui::RichText::new("把 PDF 拖到这里，或点上面的「添加文件」").font(regular(13.0)).color(t.text_muted));
                ui.add_space(4.0);
                ui.label(egui::RichText::new("支持一次加入多个文件，或整个文件夹（会递归收集其中的 PDF）").font(regular(11.5)).color(t.text_faint));
            });
            return;
        }

        let selected = self.files.selected;
        let name_sort = self.name_sort;
        let rows: Vec<RowData> = self
            .files
            .items
            .iter()
            .enumerate()
            .map(|(index, item)| {
                // 提示列：执行阶段的详情（完成摘要/跳过原因/错误）优先，其次才是查找结果。
                // 不同来源用不同颜色，方便一眼区分「匹配到了」与「没匹配到」。
                let detail = item.status.detail();
                let (hint, hint_color) = if !detail.is_empty() {
                    // 执行详情（完成摘要 / 跳过原因 / 错误）沿用状态列的颜色。
                    (detail.to_string(), status_color(&item.status, &t))
                } else {
                    match item.hint_hit {
                        Some(true) => (item.hint.clone(), ok()),
                        Some(false) => (item.hint.clone(), warn()),
                        None => (item.hint.clone(), t.text_muted),
                    }
                };
                RowData {
                    id: item.id,
                    number: index + 1,
                    name: item
                        .input
                        .path
                        .file_name()
                        .map(|name| name.to_string_lossy().into_owned())
                        .unwrap_or_default(),
                    hint,
                    hint_color,
                    // 路径列只显示所在目录：文件名已经在「文件名」列里了。
                    path: item
                        .input
                        .path
                        .parent()
                        .map(|dir| dir.display().to_string())
                        .unwrap_or_default(),
                    status: item.status.label().to_string(),
                    color: status_color(&item.status, &t),
                }
            })
            .collect();

        // 列宽：按钮与序号固定，提示/状态/路径按内容自适应，文件名按表头分隔条拖出的宽度（无上限）。
        let btn_w = 60.0;
        let num_w = 44.0;
        let name_w = self.col_name_width.max(80.0);
        let hint_w = (max_text_width(ui, rows.iter().map(|row| row.hint.as_str()), regular(12.0)) + 20.0).max(80.0);
        let status_w = max_text_width(ui, rows.iter().map(|row| row.status.as_str()), regular(12.0)) + 20.0;
        let path_w = max_text_width(ui, rows.iter().map(|row| row.path.as_str()), regular(12.0)) + 20.0;
        let gap = ui.spacing().item_spacing.x;
        let total = btn_w + num_w + name_w + hint_w + status_w + path_w + gap * 5.0;

        let row_height = 26.0;
        let header_height = 28.0;
        let mut clicked: Option<u64> = None;
        let mut remove: Option<u64> = None;
        let mut move_up_id: Option<u64> = None;
        let mut move_down_id: Option<u64> = None;
        let mut sort_clicked = false;
        let mut dragged_name_w = name_w;

        // 自己排列表：外层统一用 ScrollArea::both，底部的水平滚动条能看到超宽的列，
        // 垂直滚动条仍固定在右侧。egui_extras 的表格内建不支持水平滚动，所以不用它。
        egui::ScrollArea::both()
            .id_salt("pdfbatch-files")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                // 表头
                let (header_rect, _) =
                    ui.allocate_exact_size(egui::vec2(total, header_height), egui::Sense::hover());
                let mut x = header_rect.left();
                // 最左的「上移 / 下移」按钮列没有标题。
                x += btn_w + gap;
                header_cell(ui, x, header_rect, num_w, "序号", t.text_muted);
                x += num_w + gap;
                // 文件名表头 + 排序按钮（点击在升序 / 降序之间循环）。
                let name_x = x;
                let name_cell = egui::Rect::from_min_size(
                    egui::pos2(name_x, header_rect.top()),
                    egui::vec2(name_w, header_rect.height()),
                );
                ui.scope_builder(
                    egui::UiBuilder::new()
                        .max_rect(name_cell)
                        .layout(egui::Layout::left_to_right(egui::Align::Center)),
                    |ui| {
                        ui.label(egui::RichText::new("文件名").font(medium(12.0)).color(t.text_muted));
                        ui.add_space(8.0);
                        let (label, tip) = match name_sort {
                            Some(true) => ("升序", "再次点击改为从大到小"),
                            Some(false) => ("降序", "再次点击改为从小到大"),
                            None => ("排序", "按文件名排序（从小到大）"),
                        };
                        if ui
                            .add_enabled(
                                !busy,
                                egui::Button::new(egui::RichText::new(label).font(regular(11.5))).small(),
                            )
                            .on_hover_text(tip)
                            .clicked()
                        {
                            sort_clicked = true;
                        }
                    },
                );
                x += name_w + gap;
                header_cell(ui, x, header_rect, hint_w, "提示", t.text_muted);
                x += hint_w + gap;
                header_cell(ui, x, header_rect, status_w, "状态", t.text_muted);
                x += status_w + gap;
                header_cell(ui, x, header_rect, path_w, "路径", t.text_muted);

                // 只有文件名列右边界画分隔条（可拖动）；最右侧不画，避免多余的线。
                let separator_x = name_x + name_w;
                let handle = egui::Rect::from_x_y_ranges(
                    (separator_x - 3.0)..=(separator_x + 3.0),
                    header_rect.y_range(),
                );
                let response = ui.interact(handle, ui.id().with("pdfbatch-col-name"), egui::Sense::drag());
                if response.dragged() {
                    dragged_name_w = (dragged_name_w + response.drag_delta().x).max(80.0);
                }
                if response.hovered() || response.dragged() {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeColumn);
                }
                let stroke = if response.dragged() {
                    ui.visuals().widgets.active.bg_stroke
                } else if response.hovered() {
                    ui.visuals().widgets.hovered.bg_stroke
                } else {
                    ui.visuals().widgets.noninteractive.bg_stroke
                };
                ui.painter().line_segment(
                    [
                        egui::pos2(separator_x, header_rect.top()),
                        egui::pos2(separator_x, header_rect.bottom()),
                    ],
                    stroke,
                );

                // 数据行
                for (index, data) in rows.iter().enumerate() {
                    let (rect, response) =
                        ui.allocate_exact_size(egui::vec2(total, row_height), egui::Sense::click());
                    let background = if selected == Some(data.id) {
                        Some(t.accent_soft)
                    } else if response.hovered() {
                        Some(t.pressed)
                    } else if index % 2 == 1 {
                        Some(t.hover)
                    } else {
                        None
                    };
                    if let Some(color) = background {
                        ui.painter().rect_filled(rect, egui::CornerRadius::ZERO, color);
                    }

                    let mut column_x = rect.left();
                    // 上移 / 下移：左右并排的两个小按钮。
                    let btn_cell = egui::Rect::from_min_size(
                        egui::pos2(column_x, rect.top()),
                        egui::vec2(btn_w, rect.height()),
                    );
                    ui.scope_builder(
                        egui::UiBuilder::new()
                            .max_rect(btn_cell)
                            .layout(egui::Layout::left_to_right(egui::Align::Center)),
                        |ui| {
                            ui.add_enabled_ui(!busy, |ui| {
                                if ui.add(egui::Button::new("上").small()).on_hover_text("上移一位").clicked() {
                                    move_up_id = Some(data.id);
                                }
                                if ui.add(egui::Button::new("下").small()).on_hover_text("下移一位").clicked() {
                                    move_down_id = Some(data.id);
                                }
                            });
                        },
                    );
                    column_x += btn_w + gap;
                    body_cell(ui, column_x, rect, num_w, &data.number.to_string(), regular(12.0), t.text_muted);
                    column_x += num_w + gap;
                    body_cell(ui, column_x, rect, name_w, &data.name, regular(12.5), t.text);
                    column_x += name_w + gap;
                    body_cell(ui, column_x, rect, hint_w, &data.hint, regular(12.0), data.hint_color);
                    column_x += hint_w + gap;
                    body_cell(ui, column_x, rect, status_w, &data.status, regular(12.0), data.color);
                    column_x += status_w + gap;
                    body_cell(ui, column_x, rect, path_w, &data.path, regular(12.0), t.text_faint);

                    if response.clicked() {
                        clicked = Some(data.id);
                    }
                    response.context_menu(|ui| {
                        if ui.button("移除").clicked() {
                            remove = Some(data.id);
                            ui.close();
                        }
                    });
                }
            });

        self.col_name_width = dragged_name_w;
        if let Some(id) = clicked {
            self.files.selected = Some(id);
        }
        if let Some(id) = move_up_id {
            self.files.move_up(id);
        }
        if let Some(id) = move_down_id {
            self.files.move_down(id);
        }
        if sort_clicked {
            // 未排序 → 升序 → 降序 → 升序…… 两种模式之间循环。
            self.name_sort = match self.name_sort {
                Some(true) => Some(false),
                _ => Some(true),
            };
            if let Some(ascending) = self.name_sort {
                self.files.sort_by_name(ascending);
            }
        }
        if let Some(id) = remove {
            self.files.selected = Some(id);
            self.files.remove_selected();
        }
    }
}

/// 表格数据单元格：固定在给定宽度内、单行截断，悬停显示完整文本。
fn body_cell(
    ui: &mut egui::Ui,
    x: f32,
    row: egui::Rect,
    width: f32,
    text: &str,
    font: egui::FontId,
    color: egui::Color32,
) {
    let cell = egui::Rect::from_min_size(egui::pos2(x, row.top()), egui::vec2(width, row.height()));
    ui.scope_builder(
        egui::UiBuilder::new().max_rect(cell).layout(egui::Layout::left_to_right(egui::Align::Center)),
        |ui| {
            ui.add(egui::Label::new(egui::RichText::new(text).font(font).color(color)).truncate())
                .on_hover_text(text.to_owned());
        },
    );
}

/// 表头单元格。
fn header_cell(ui: &mut egui::Ui, x: f32, row: egui::Rect, width: f32, text: &str, color: egui::Color32) {
    let cell = egui::Rect::from_min_size(egui::pos2(x, row.top()), egui::vec2(width, row.height()));
    ui.scope_builder(
        egui::UiBuilder::new().max_rect(cell).layout(egui::Layout::left_to_right(egui::Align::Center)),
        |ui| {
            ui.label(egui::RichText::new(text).font(medium(12.0)).color(color));
        },
    );
}

/// 一组文本里最宽的那个（让状态列、路径列按内容自适应宽度）。
fn max_text_width<'a>(ui: &egui::Ui, texts: impl Iterator<Item = &'a str>, font: egui::FontId) -> f32 {
    let mut widest = 0.0_f32;
    for text in texts {
        let width = ui
            .painter()
            .layout_no_wrap(text.to_owned(), font.clone(), egui::Color32::WHITE)
            .size()
            .x;
        widest = widest.max(width);
    }
    widest
}

fn status_color(status: &FileStatus, t: &Tokens) -> egui::Color32 {
    match status {
        FileStatus::Failed(_) => danger(),
        FileStatus::Skipped(_) => t.text_faint,
        FileStatus::Running => t.accent_text,
        FileStatus::Done(_) => ok(),
        FileStatus::Pending => t.text_muted,
    }
}
