//! pdfbatch-ui —— 图形界面层。
//!
//! 观感对齐 PdfCraft（同款配色、字体与控件风格），界面逻辑是本项目自己的：
//! 一个窗口里做两件事——批量盖填充层、批量导入书签。
//!
//! 两个功能都作用在左侧文件列表里的**所有**文件上（"应用到多份文件"），
//! 填充层还额外提供混合模式与置于顶层/底层的选择。

mod files;
mod job;
mod pickers;
mod theme;
mod widgets;

use std::path::PathBuf;
use std::time::Duration;

use eframe::egui;

use pdfbatch_core::spec::SpecEntry;
use pdfbatch_core::{Blend, Layer};

use files::{FileList, FileStatus};
use job::{
    BookmarkDraft, ChangeRect, CommonOptions, FillAction, FillDraft, JobProgress, MatchRect, Mode,
    OutputChoice,
};
use pickers::{PickPurpose, Pickers};
use theme::{ThemeKind, Tokens};

pub use theme::{medium, regular, semibold};

/// 书签文本的示例（"填入示例"按钮用）。
const SAMPLE: &str = "# 每行一条书签；缩进（Tab 或两个空格）表示层级\n# 标题与页码之间用 Tab 或“ | ”分隔；页码从 1 开始\n第一章 绪论 | 1\n\t1.1 研究背景 | 2\n\t1.2 研究意义 | 3\n第二章 方法 | 10\n";

/// 启动图形界面。
pub fn run() -> eframe::Result {
    let mut viewport = egui::ViewportBuilder::default()
        .with_title("pdfbatch")
        .with_inner_size([1180.0, 760.0])
        .with_min_inner_size([900.0, 560.0])
        .with_drag_and_drop(true);
    if let Some(icon) = window_icon() {
        viewport = viewport.with_icon(icon);
    }
    let options = eframe::NativeOptions {
        viewport,
        renderer: eframe::Renderer::Glow,
        ..Default::default()
    };
    // 字体与主题必须在第一帧之前装好：egui 在帧开始时就取用了字体定义，
    // 若等到第一帧的 `logic` 里才 `set_fonts`，本帧绘制带字重的文字会直接 panic。
    eframe::run_native(
        "pdfbatch",
        options,
        Box::new(|cc| {
            theme::install_fonts(&cc.egui_ctx);
            theme::apply(&cc.egui_ctx, ThemeKind::Light);
            Ok(Box::new(App {
                fonts_installed: true,
                applied: Some(ThemeKind::Light),
                ..App::default()
            }))
        }),
    )
}

/// 窗口图标（任务栏、Alt+Tab、标题栏左上角）。
///
/// 资源管理器里的**文件图标**来自 exe 内嵌的 `.ico`（见 `apps/pdfbatch/build.rs`）；
/// 这里是**窗口图标**——Windows 不会自动复用 exe 的图标资源，必须显式设置。
fn window_icon() -> Option<std::sync::Arc<egui::IconData>> {
    let bytes: &[u8] = include_bytes!("../../../assets/app-icon/pdfbatch-256.png");
    let mut reader = png::Decoder::new(std::io::Cursor::new(bytes)).read_info().ok()?;
    let (width, height, is_rgba8) = {
        let info = reader.info();
        (
            info.width,
            info.height,
            info.color_type == png::ColorType::Rgba && info.bit_depth == png::BitDepth::Eight,
        )
    };
    // 只认我们生成的那种 PNG（8 位 RGBA）；形状不对就退回系统默认图标。
    if !is_rgba8 {
        return None;
    }
    let mut rgba = vec![0u8; width as usize * height as usize * 4];
    reader.next_frame(&mut rgba).ok()?;
    Some(std::sync::Arc::new(egui::IconData { rgba, width, height }))
}

/// 应用状态。
pub struct App {
    mode: Mode,
    theme: ThemeKind,
    /// 字体只安装一次。
    fonts_installed: bool,
    /// 已经应用到 egui 的主题（首帧之前是 `None`）。
    applied: Option<ThemeKind>,
    files: FileList,
    fill: FillDraft,
    bookmarks: BookmarkDraft,
    common: CommonOptions,
    pickers: Pickers,
    run: Option<job::JobRun>,
    /// 正在运行任务的进度快照（用于底部执行条）。
    progress: Option<JobProgress>,
    /// 正在运行的「查找匹配」任务。
    probe: Option<job::ProbeRun>,
    /// 最近一次查找的结果（删/改模式据此判断能否执行）。
    probe_report: Option<job::ProbeReport>,
    /// 浮动提示：消息 + 出现时刻。
    toast: Option<(String, f64)>,
    /// 是否有文件正被拖到窗口上（显示放置提示）。
    hover_drop: bool,
    /// 文件列表「文件名」列的宽度（表头分隔条可拖，无上限）。
    col_name_width: f32,
}

impl Default for App {
    fn default() -> Self {
        Self {
            mode: Mode::Fill,
            theme: ThemeKind::Light,
            fonts_installed: false,
            applied: None,
            files: FileList::default(),
            fill: FillDraft::default(),
            bookmarks: BookmarkDraft::default(),
            common: CommonOptions::default(),
            pickers: Pickers::default(),
            run: None,
            progress: None,
            probe: None,
            probe_report: None,
            toast: None,
            hover_drop: false,
            col_name_width: 260.0,
        }
    }
}

impl eframe::App for App {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if !self.fonts_installed {
            theme::install_fonts(ctx);
            self.fonts_installed = true;
        }
        if self.applied != Some(self.theme) {
            theme::apply(ctx, self.theme);
            self.applied = Some(self.theme);
        }

        self.process_picks(ctx);
        self.handle_dropped(ctx);
        self.handle_shortcuts(ctx);
        self.poll_job(ctx);
        self.poll_probe(ctx);
        self.expire_toast(ctx);
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let t = Tokens::get(ui.ctx());

        egui::Panel::top("pdfbatch-topbar").show(ui, |ui| self.top_bar(ui, t));
        egui::Panel::bottom("pdfbatch-runbar").show(ui, |ui| self.run_bar(ui, t));
        egui::Panel::left("pdfbatch-params")
            .resizable(true)
            .default_size(340.0)
            .min_size(300.0)
            .max_size(470.0)
            .show(ui, |ui| self.params_panel(ui, t));
        // 输出区提升为顶层面板（与左侧参数同级）：独占文件列表下方的整条区域，
        // 不再嵌在 CentralPanel 内，避免表格的列宽手柄绘制/交互穿透到输出区。
        egui::Panel::bottom("pdfbatch-output").show(ui, |ui| self.output_options(ui, t));
        egui::CentralPanel::default().show(ui, |ui| self.files_area(ui, t));

        // 拖放提示与浮动提示都画在最上层。
        if self.hover_drop {
            let rect = ui.ctx().content_rect().shrink(6.0);
            ui.painter().rect_stroke(
                rect,
                egui::CornerRadius::same(10),
                egui::Stroke::new(2.0, t.accent),
                egui::StrokeKind::Inside,
            );
        }
        if let Some((message, _)) = &self.toast {
            widgets::toast(ui.ctx(), message);
        }
    }
}

// ——————————————————————————— 逻辑（非绘制） ———————————————————————————

impl App {
    /// 处理已经完成的选择器结果。
    fn process_picks(&mut self, ctx: &egui::Context) {
        for pick in self.pickers.take() {
            if pick.paths.is_empty() {
                continue;
            }
            match pick.purpose {
                PickPurpose::AddFiles | PickPurpose::AddFolder => self.add_paths(&pick.paths, ctx),
                PickPurpose::OutputDir => {
                    if let Some(dir) = pick.paths.into_iter().next() {
                        self.common.dir = Some(dir);
                        self.common.output = OutputChoice::Dir;
                    }
                }
                PickPurpose::BookmarkSpec => {
                    if let Some(path) = pick.paths.into_iter().next() {
                        match std::fs::read_to_string(&path) {
                            Ok(text) => {
                                self.bookmarks.text = text;
                                self.bookmarks.source = Some(path);
                                self.mode = Mode::Bookmarks;
                            }
                            Err(error) => self.notify(format!("读不到 {}：{error}", path.display()), ctx),
                        }
                    }
                }
            }
        }
    }

    /// 只接受 PDF 文件或文件夹；其余一律拒绝。
    fn add_paths(&mut self, paths: &[PathBuf], ctx: &egui::Context) {
        let mut keep: Vec<PathBuf> = Vec::new();
        let mut rejected = 0usize;
        for path in paths {
            let is_dir = path.is_dir();
            let is_pdf = path.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("pdf"));
            if is_dir || is_pdf {
                keep.push(path.clone());
            } else {
                rejected += 1;
            }
        }
        if keep.is_empty() {
            if rejected > 0 {
                self.notify("只能加入 PDF 文件或文件夹", ctx);
            }
            return;
        }
        let (added, error) = self.files.add_paths(&keep, true);
        if let Some(error) = error {
            if added > 0 {
                self.notify(format!("已加入 {added} 个文件；{error}"), ctx);
            } else {
                self.notify(error, ctx);
            }
        } else if added == 0 {
            self.notify("这些文件已经在列表里了", ctx);
        }
    }

    /// 接收拖放进来的文件。
    fn handle_dropped(&mut self, ctx: &egui::Context) {
        let (dropped, hovering) = ctx.input(|input| {
            let dropped: Vec<PathBuf> =
                input.raw.dropped_files.iter().map(|file| file.path().to_path_buf()).collect();
            (dropped, !input.raw.hovered_files.is_empty())
        });
        self.hover_drop = hovering;
        if !dropped.is_empty() {
            self.add_paths(&dropped, ctx);
        }
    }

    /// 快捷键：Ctrl+O 添加文件、Ctrl+Enter 执行。
    fn handle_shortcuts(&mut self, ctx: &egui::Context) {
        let (open, run) = ctx.input(|input| {
            (
                input.modifiers.command && input.key_pressed(egui::Key::O),
                input.modifiers.command && input.key_pressed(egui::Key::Enter),
            )
        });
        if open {
            self.ask_files(ctx);
        }
        if run {
            self.try_start(ctx);
        }
    }

    /// 回填后台任务的进度；完成时弹提示并收尾。
    fn poll_job(&mut self, ctx: &egui::Context) {
        let Some(run) = self.run.take() else { return };
        let snapshot = run.snapshot();
        self.apply_progress(&snapshot);
        if snapshot.finished {
            // `run` 在这里被丢弃：它的 Drop 会 join 工作线程。
            drop(run);
            let (done, skipped, failed) = counts(&snapshot.statuses);
            let verb = if snapshot.cancelled { "已取消" } else { "执行完成" };
            self.progress = None;
            self.notify(format!("{verb}：成功 {done}，跳过 {skipped}，失败 {failed}"), ctx);
        } else {
            self.progress = Some(snapshot);
            self.run = Some(run);
            ctx.request_repaint_after(Duration::from_millis(120));
        }
    }

    /// 回填后台查找的结果。
    fn poll_probe(&mut self, ctx: &egui::Context) {
        let Some(mut run) = self.probe.take() else { return };
        if let Some(result) = run.take() {
            drop(run);
            match result {
                Ok(report) => {
                    let total = report.total;
                    self.probe_report = Some(report);
                    self.notify(format!("查找完成：命中 {total} 个填充层"), ctx);
                }
                Err(message) => {
                    self.probe_report = None;
                    self.notify(message, ctx);
                }
            }
        } else {
            self.probe = Some(run);
            ctx.request_repaint_after(Duration::from_millis(120));
        }
    }

    /// 启动一次「查找匹配」（删/改模式执行前的统计）。
    fn start_probe(&mut self, ctx: &egui::Context) {
        if self.probe.is_some() || self.run.is_some() {
            return;
        }
        let inputs = self.files.to_inputs();
        match job::build_probe_request(self.mode, &self.fill, &self.common, inputs) {
            Ok(request) => {
                let fingerprint = job::matcher_fingerprint(&self.fill);
                self.probe_report = None;
                match job::spawn_probe(request, fingerprint, ctx.clone()) {
                    Ok(run) => self.probe = Some(run),
                    Err(message) => self.notify(message, ctx),
                }
            }
            Err(message) => self.notify(message, ctx),
        }
    }

    /// 把进度里的逐行状态写回文件列表。
    fn apply_progress(&mut self, snapshot: &JobProgress) {
        for (index, status) in snapshot.statuses.iter().enumerate() {
            if let Some(item) = self.files.items.get_mut(index) {
                item.status = status.clone();
            }
        }
    }

    /// 校验通过就启动任务。
    fn try_start(&mut self, ctx: &egui::Context) {
        if self.run.is_some() {
            return;
        }
        let inputs = self.files.to_inputs();
        match job::build_request(self.mode, &self.fill, &self.bookmarks, &self.common, inputs) {
            Ok(request) => match job::spawn_job(request, ctx.clone()) {
                Ok(run) => {
                    self.files.reset_statuses();
                    self.progress = Some(run.snapshot());
                    self.run = Some(run);
                }
                Err(message) => self.notify(message, ctx),
            },
            Err(message) => self.notify(message, ctx),
        }
    }

    fn notify(&mut self, message: impl Into<String>, ctx: &egui::Context) {
        let now = ctx.input(|input| input.time);
        self.toast = Some((message.into(), now));
    }

    fn expire_toast(&mut self, ctx: &egui::Context) {
        let Some((_, start)) = &self.toast else { return };
        let start = *start;
        let now = ctx.input(|input| input.time);
        if now - start > 3.5 {
            self.toast = None;
        } else {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
    }

    // ——— 文件选择器（都在主线程创建对话框，等待放到工作线程） ———

    fn ask_files(&mut self, ctx: &egui::Context) {
        if self.pickers.is_showing() {
            return;
        }
        let pick = rfd::AsyncFileDialog::new()
            .add_filter("PDF 文件", &["pdf"])
            .set_title("选择 PDF 文件")
            .pick_files();
        self.pickers.spawn(PickPurpose::AddFiles, Some(ctx.clone()), async move {
            pick.await.unwrap_or_default().into_iter().map(|handle| handle.path().to_path_buf()).collect()
        });
    }

    fn ask_folder(&mut self, ctx: &egui::Context) {
        if self.pickers.is_showing() {
            return;
        }
        let pick = rfd::AsyncFileDialog::new().set_title("选择包含 PDF 的文件夹").pick_folder();
        self.pickers.spawn(PickPurpose::AddFolder, Some(ctx.clone()), async move {
            pick.await.map(|handle| vec![handle.path().to_path_buf()]).unwrap_or_default()
        });
    }

    fn ask_output_dir(&mut self, ctx: &egui::Context) {
        if self.pickers.is_showing() {
            return;
        }
        let pick = rfd::AsyncFileDialog::new().set_title("选择输出目录").pick_folder();
        self.pickers.spawn(PickPurpose::OutputDir, Some(ctx.clone()), async move {
            pick.await.map(|handle| vec![handle.path().to_path_buf()]).unwrap_or_default()
        });
    }

    fn ask_spec(&mut self, ctx: &egui::Context) {
        if self.pickers.is_showing() {
            return;
        }
        let pick = rfd::AsyncFileDialog::new()
            .add_filter("文本文件", &["txt"])
            .set_title("选择书签文本")
            .pick_file();
        self.pickers.spawn(PickPurpose::BookmarkSpec, Some(ctx.clone()), async move {
            pick.await.map(|handle| vec![handle.path().to_path_buf()]).unwrap_or_default()
        });
    }
}

// ——————————————————————————— 绘制 ———————————————————————————

impl App {
    fn top_bar(&mut self, ui: &mut egui::Ui, t: Tokens) {
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("pdfbatch").font(semibold(17.0)));
            ui.add_space(2.0);
            ui.label(egui::RichText::new("批量填充层与书签").font(regular(12.0)).color(t.text_muted));
            ui.add_space(20.0);
            for mode in Mode::ALL {
                if widgets::mode_tab(ui, mode.label(), self.mode == mode).clicked() && self.mode != mode {
                    self.mode = mode;
                    self.common.suffix = mode.suffix().to_string();
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let label = match self.theme {
                    ThemeKind::Light => "深色",
                    ThemeKind::Dark => "浅色",
                };
                if widgets::pill_button(ui, label, false).clicked() {
                    self.theme = self.theme.toggled();
                }
            });
        });
        ui.add_space(6.0);
    }

    fn params_panel(&mut self, ui: &mut egui::Ui, t: Tokens) {
        ui.add_space(2.0);
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            match self.mode {
                Mode::Fill => self.fill_params(ui, t),
                Mode::Bookmarks => self.bookmark_params(ui, t),
            }
            ui.add_space(16.0);
        });
    }

    fn fill_params(&mut self, ui: &mut egui::Ui, t: Tokens) {
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
                ui.add_space(4.0);
                ui.checkbox(&mut self.fill.replace, "先摘掉已有的填充层（替换）");
                ui.add_space(8.0);
                ui.label(
                    egui::RichText::new("这些参数会应用到左侧列表里的所有文件。")
                        .font(regular(11.5))
                        .color(t.text_faint),
                );
            }
            FillAction::Erase => {
                self.fill_match_options(ui, t);
                self.fill_pages(ui, t);
                self.fill_probe_row(ui, t);
                ui.add_space(8.0);
                ui.label(
                    egui::RichText::new("查找后按「执行」删除命中的填充层。")
                        .font(regular(11.5))
                        .color(t.text_faint),
                );
            }
            FillAction::Modify => {
                // 上部分：查找选项；下部分：更改要求。
                self.fill_match_options(ui, t);
                self.fill_pages(ui, t);
                self.fill_probe_row(ui, t);
                ui.add_space(10.0);
                widgets::section_title(ui, "更改要求");
                self.fill_change_options(ui, t);
            }
        }
    }

    /// 新增模式：要盖上去的填充层样式。
    fn fill_add_style(&mut self, ui: &mut egui::Ui, t: Tokens) {
        widgets::section_title(ui, "区域");
        ui.horizontal(|ui| {
            ui.selectable_value(&mut self.fill.whole_page, true, "整页");
            ui.selectable_value(&mut self.fill.whole_page, false, "自定义矩形");
        });
        ui.add_space(6.0);

        if !self.fill.whole_page {
            let rect_ok = self.fill.rect[2] > 0.0 && self.fill.rect[3] > 0.0;
            rect_rows(ui, "fill-rect", &mut self.fill.rect);
            if !rect_ok {
                ui.label(egui::RichText::new("宽和高必须大于 0").font(regular(11.5)).color(danger()));
            }
        }
        ui.add_space(4.0);
        widgets::page_preview(ui, self.fill.rect, self.fill.color, self.fill.opacity as f32, self.fill.whole_page);

        widgets::section_title(ui, "颜色");
        ui.horizontal(|ui| {
            ui.label("颜色");
            // 用 sRGB 取色控件：返回的就是 sRGB 分量，和 PDF 的 `rg` 语义一致。
            egui::color_picker::color_edit_button_srgba(ui, &mut self.fill.color, egui::color_picker::Alpha::Opaque);
            ui.label(egui::RichText::new(hex(self.fill.color)).font(regular(12.0)).color(t.text_muted));
        });
        ui.horizontal(|ui| {
            ui.label("不透明度");
            ui.add(egui::Slider::new(&mut self.fill.opacity, 0.0..=1.0).fixed_decimals(2));
        });

        widgets::section_title(ui, "混合与层级");
        ui.horizontal(|ui| {
            ui.label("混合模式");
            blend_combo(ui, "fill-blend", &mut self.fill.blend);
        });
        ui.horizontal(|ui| {
            ui.label("层级");
            ui.selectable_value(&mut self.fill.layer, Layer::Top, "置于顶层");
            ui.selectable_value(&mut self.fill.layer, Layer::Bottom, "置于底层");
        });
    }

    /// 作用范围（三种操作共用）。
    fn fill_pages(&mut self, ui: &mut egui::Ui, _t: Tokens) {
        widgets::section_title(ui, "作用范围");
        ui.horizontal(|ui| {
            ui.label("页范围");
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

    /// 查找条件（删/改模式共用）：填充区域、颜色、透明度、混合模式、层级。
    fn fill_match_options(&mut self, ui: &mut egui::Ui, t: Tokens) {
        widgets::section_title(ui, "查找条件");
        ui.label(
            egui::RichText::new("不勾任何项表示匹配本工具加的全部填充层。")
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
            MatchRect::Size => size_row(ui, &mut self.fill.match_size),
            MatchRect::Rect => rect_rows(ui, "fill-match-rect", &mut self.fill.match_rect_value),
            MatchRect::Any | MatchRect::FullPage => {}
        }
        ui.horizontal(|ui| {
            ui.checkbox(&mut self.fill.match_color_on, "填充颜色");
            if self.fill.match_color_on {
                egui::color_picker::color_edit_button_srgba(
                    ui,
                    &mut self.fill.match_color,
                    egui::color_picker::Alpha::Opaque,
                );
                ui.label(egui::RichText::new(hex(self.fill.match_color)).font(regular(12.0)).color(t.text_muted));
            }
        });
        ui.horizontal(|ui| {
            ui.checkbox(&mut self.fill.match_opacity_on, "透明度");
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
        ui.horizontal(|ui| {
            ui.label("数值容差");
            ui.add(egui::DragValue::new(&mut self.fill.tolerance).speed(0.001).range(0.0..=1.0));
            ui.label(egui::RichText::new("0 表示精确相等").font(regular(11.0)).color(t.text_faint));
        });
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
    fn fill_change_options(&mut self, ui: &mut egui::Ui, t: Tokens) {
        ui.horizontal_wrapped(|ui| {
            ui.label("填充区域");
            for choice in ChangeRect::ALL {
                ui.selectable_value(&mut self.fill.change_rect, choice, choice.label());
            }
        });
        if self.fill.change_rect == ChangeRect::Rect {
            rect_rows(ui, "fill-change-rect", &mut self.fill.change_rect_value);
        }
        ui.horizontal(|ui| {
            ui.checkbox(&mut self.fill.change_color_on, "改成颜色");
            if self.fill.change_color_on {
                egui::color_picker::color_edit_button_srgba(
                    ui,
                    &mut self.fill.change_color,
                    egui::color_picker::Alpha::Opaque,
                );
                ui.label(egui::RichText::new(hex(self.fill.change_color)).font(regular(12.0)).color(t.text_muted));
            }
        });
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

    fn bookmark_params(&mut self, ui: &mut egui::Ui, t: Tokens) {
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

    fn files_area(&mut self, ui: &mut egui::Ui, t: Tokens) {
        ui.add_space(6.0);
        let busy = self.run.is_some();
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
        let rows: Vec<RowData> = self
            .files
            .items
            .iter()
            .map(|item| RowData {
                id: item.id,
                name: item
                    .input
                    .path
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                path: item.input.path.display().to_string(),
                status: item.status.label(),
                color: status_color(&item.status, &t),
            })
            .collect();

        // 列宽：状态与路径按内容自适应，文件名按表头分隔条拖出的宽度（可拖、无上限）。
        let name_w = self.col_name_width.max(80.0);
        let status_w = max_text_width(ui, rows.iter().map(|row| row.status.as_str()), regular(12.0)) + 20.0;
        let path_w = max_text_width(ui, rows.iter().map(|row| row.path.as_str()), regular(12.0)) + 20.0;
        let gap = ui.spacing().item_spacing.x;
        let total = name_w + status_w + path_w + gap * 2.0;

        let row_height = 26.0;
        let header_height = 28.0;
        let mut clicked: Option<u64> = None;
        let mut remove: Option<u64> = None;
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
                header_cell(ui, x, header_rect, name_w, "文件名", t.text_muted);
                x += name_w + gap;
                header_cell(ui, x, header_rect, status_w, "状态", t.text_muted);
                x += status_w + gap;
                header_cell(ui, x, header_rect, path_w, "路径", t.text_muted);

                // 只有文件名列右边界画分隔条（可拖动）；最右侧不画，避免多余的线。
                let separator_x = header_rect.left() + name_w;
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
                    body_cell(ui, column_x, rect, name_w, &data.name, regular(12.5), t.text);
                    column_x += name_w + gap;
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
        if let Some(id) = remove {
            self.files.selected = Some(id);
            self.files.remove_selected();
        }
    }

    fn output_options(&mut self, ui: &mut egui::Ui, t: Tokens) {
        widgets::section_title(ui, "输出");
        ui.horizontal(|ui| {
            ui.selectable_value(&mut self.common.output, OutputChoice::SameFolder, "同目录另存");
            ui.selectable_value(&mut self.common.output, OutputChoice::Dir, "指定目录");
            ui.selectable_value(&mut self.common.output, OutputChoice::InPlace, "覆盖原文件");
        });
        ui.add_space(2.0);
        match self.common.output {
            OutputChoice::SameFolder => {
                ui.label(
                    egui::RichText::new("另存到原文件所在目录，不改动原文件")
                        .font(regular(11.5))
                        .color(t.text_muted),
                );
            }
            OutputChoice::Dir => {
                let dir_text = self
                    .common
                    .dir
                    .as_ref()
                    .map(|dir| dir.display().to_string())
                    .unwrap_or_else(|| "未选择".to_string());
                let chosen = self.common.dir.is_some();
                let mut ask = false;
                ui.horizontal(|ui| {
                    if widgets::pill_button(ui, "选择目录", false).clicked() {
                        ask = true;
                    }
                    let color = if chosen { t.text } else { t.text_faint };
                    ui.label(egui::RichText::new(dir_text).font(regular(12.0)).color(color));
                });
                if ask {
                    self.ask_output_dir(ui.ctx());
                }
            }
            OutputChoice::InPlace => {
                ui.label(egui::RichText::new("会覆盖原文件（先写临时文件，再原子替换）").font(regular(11.5)).color(t.text_muted));
            }
        }
        // 命名只在会写出新文件名时才有意义（覆盖原文件不改名）。
        if self.common.output != OutputChoice::InPlace {
            self.naming_row(ui, t);
        }
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.label("密码");
            ui.add(
                egui::TextEdit::singleline(&mut self.common.password)
                    .password(true)
                    .hint_text("加密 PDF 才需要")
                    .desired_width(150.0),
            );
            ui.checkbox(&mut self.common.full_save, "整体重写（更小，可能使签名失效）");
        });
    }

    /// 文件名前缀 / 后缀：同目录另存与指定目录共用，覆盖原文件时不显示。
    fn naming_row(&mut self, ui: &mut egui::Ui, t: Tokens) {
        ui.horizontal(|ui| {
            ui.label("文件名前缀");
            ui.add(egui::TextEdit::singleline(&mut self.common.prefix).desired_width(80.0).hint_text("如 new_"));
            ui.add_space(6.0);
            ui.label("后缀");
            ui.add(egui::TextEdit::singleline(&mut self.common.suffix).desired_width(80.0).hint_text("如 _filled"));
        });

        let naming = job::effective_naming(self.mode, &self.common);
        let sample = self
            .files
            .items
            .first()
            .and_then(|item| item.input.path.file_stem().map(|stem| stem.to_string_lossy().into_owned()))
            .unwrap_or_else(|| "文件名".to_string());
        ui.label(
            egui::RichText::new(format!("预览：{}{}{}.pdf", naming.prefix, sample, naming.suffix))
                .font(regular(11.5))
                .color(t.text_muted),
        );

        let both_empty = self.common.prefix.is_empty() && self.common.suffix.trim().is_empty();
        if both_empty && self.common.output == OutputChoice::SameFolder {
            ui.label(
                egui::RichText::new(format!("前后缀都为空，将使用默认后缀 {}，避免覆盖原文件", self.mode.suffix()))
                    .font(regular(11.0))
                    .color(t.text_faint),
            );
        }
        if let Err(message) = job::validate_naming(&self.common.prefix, &self.common.suffix) {
            ui.label(egui::RichText::new(message).font(regular(11.5)).color(danger()));
        }
    }

    fn run_bar(&mut self, ui: &mut egui::Ui, t: Tokens) {
        let busy = self.run.is_some();
        let preflight = job::preflight(self.mode, &self.fill, &self.bookmarks, &self.common, &self.files.to_inputs());
        let progress = self.progress.clone();
        let file_count = self.files.len();
        let mut clicked_run = false;
        let mut clicked_cancel = false;

        // 删/改必须先查找匹配，且命中数非零才允许执行。
        let needs_probe = self.mode == Mode::Fill && self.fill.action != FillAction::Add;
        let probe_ok = self.probe_report.as_ref().is_some_and(|report| {
            report.total > 0 && report.fingerprint == job::matcher_fingerprint(&self.fill)
        });
        let ready = preflight.is_ok() && (!needs_probe || probe_ok);

        ui.add_space(6.0);
        ui.horizontal(|ui| {
            if busy {
                let p = progress.unwrap_or_else(|| JobProgress {
                    total: 0,
                    done: 0,
                    statuses: Vec::new(),
                    current: String::new(),
                    finished: false,
                    cancelled: false,
                });
                let fraction = if p.total == 0 { 0.0 } else { p.done as f32 / p.total as f32 };
                ui.add(
                    egui::ProgressBar::new(fraction)
                        .desired_width(260.0)
                        .text(format!("{}/{}", p.done, p.total)),
                );
                if !p.current.is_empty() {
                    ui.label(egui::RichText::new(format!("正在处理 {}", p.current)).font(regular(12.0)).color(t.text_muted));
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if widgets::pill_button(ui, "取消", false).clicked() {
                        clicked_cancel = true;
                    }
                });
            } else {
                let response = ui
                    .add_enabled_ui(ready, |ui| widgets::pill_button(ui, "▶ 执行", true))
                    .inner;
                if response.clicked() {
                    clicked_run = true;
                }
                ui.add_space(10.0);
                match &preflight {
                    Ok(()) => {
                        let (text, color) = if self.mode == Mode::Bookmarks {
                            (format!("将对 {file_count} 个文件执行「书签」"), t.text_muted)
                        } else {
                            match self.fill.action {
                                FillAction::Add => {
                                    (format!("将对 {file_count} 个文件执行「新增填充层」"), t.text_muted)
                                }
                                FillAction::Erase | FillAction::Modify => {
                                    if probe_ok {
                                        let total =
                                            self.probe_report.as_ref().map(|report| report.total).unwrap_or(0);
                                        (
                                            format!(
                                                "将对 {file_count} 个文件执行「{}」，命中 {total} 个填充层",
                                                self.fill.action.label()
                                            ),
                                            t.text_muted,
                                        )
                                    } else {
                                        ("请先在左侧点「查找匹配」，命中非零才可执行".to_string(), warn())
                                    }
                                }
                            }
                        };
                        ui.label(egui::RichText::new(text).font(regular(12.0)).color(color));
                    }
                    Err(message) => {
                        ui.label(egui::RichText::new(message).font(regular(12.0)).color(danger()));
                    }
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(egui::RichText::new("Ctrl+Enter 执行").font(regular(11.0)).color(t.text_faint));
                });
            }
        });
        ui.add_space(6.0);

        if clicked_cancel {
            if let Some(run) = &self.run {
                run.cancel();
            }
        }
        if clicked_run {
            self.try_start(ui.ctx());
        }
    }
}

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
    name: String,
    path: String,
    status: String,
    color: egui::Color32,
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

fn danger() -> egui::Color32 {
    egui::Color32::from_rgb(0xC0, 0x39, 0x2B)
}

fn warn() -> egui::Color32 {
    egui::Color32::from_rgb(0xB4, 0x6A, 0x00)
}

fn ok() -> egui::Color32 {
    egui::Color32::from_rgb(0x1E, 0x8E, 0x3E)
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

/// 「宽 × 高」两格输入（按宽高匹配时用）。
fn size_row(ui: &mut egui::Ui, size: &mut [f64; 2]) {
    egui::Grid::new("fill-size").num_columns(2).spacing([8.0, 6.0]).show(ui, |ui| {
        ui.label("宽");
        ui.add(egui::DragValue::new(&mut size[0]).speed(1.0).range(0.0..=20000.0));
        ui.end_row();
        ui.label("高");
        ui.add(egui::DragValue::new(&mut size[1]).speed(1.0).range(0.0..=20000.0));
        ui.end_row();
    });
}

/// 「左下起点 + 宽高」四格输入（按完整矩形匹配/修改时用）。
fn rect_rows(ui: &mut egui::Ui, id: &str, rect: &mut [f64; 4]) {
    egui::Grid::new(id).num_columns(2).spacing([8.0, 6.0]).show(ui, |ui| {
        ui.label("X（左）");
        ui.add(egui::DragValue::new(&mut rect[0]).speed(1.0).range(0.0..=20000.0));
        ui.end_row();
        ui.label("Y（下）");
        ui.add(egui::DragValue::new(&mut rect[1]).speed(1.0).range(0.0..=20000.0));
        ui.end_row();
        ui.label("宽");
        ui.add(egui::DragValue::new(&mut rect[2]).speed(1.0).range(-20000.0..=20000.0));
        ui.end_row();
        ui.label("高");
        ui.add(egui::DragValue::new(&mut rect[3]).speed(1.0).range(-20000.0..=20000.0));
        ui.end_row();
    });
}

/// 预览一段书签树（空段与警告一并显示）。
fn preview_one(ui: &mut egui::Ui, parsed: &pdfbatch_core::spec::ParsedSpec, t: Tokens) {
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
fn preview_tree(ui: &mut egui::Ui, entries: &[SpecEntry], depth: usize, t: Tokens) {
    for entry in entries {
        ui.horizontal(|ui| {
            ui.add_space(depth as f32 * 16.0);
            ui.label(egui::RichText::new(&entry.title).font(regular(12.5)));
            ui.label(egui::RichText::new(format!("第 {} 页", entry.page + 1)).font(regular(11.0)).color(t.text_faint));
        });
        preview_tree(ui, &entry.children, depth + 1, t);
    }
}

/// 颜色的十六进制写法（直接取 sRGB 分量）。
fn hex(color: egui::Color32) -> String {
    format!("#{:02X}{:02X}{:02X}", color.r(), color.g(), color.b())
}

fn counts(statuses: &[FileStatus]) -> (usize, usize, usize) {
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

#[cfg(test)]
mod tests {
    /// 窗口图标必须能解码成 256×256 的 RGBA，且取到的是我们自己的图标（蓝色底块）。
    #[test]
    fn window_icon_decodes_to_the_brand_tile() {
        let icon = super::window_icon().expect("图标 PNG 应当能解码");
        assert_eq!((icon.width, icon.height), (256, 256));
        assert_eq!(icon.rgba.len(), 256 * 256 * 4, "RGBA 长度应当与尺寸匹配");
        // 左侧中部（页面之外）应当是品牌蓝 #1B63E0，说明不是空图、也不是系统默认图标。
        let index = (128 * 256 + 20) * 4;
        assert_eq!(&icon.rgba[index..index + 4], &[27, 99, 224, 255], "应当取到品牌蓝底块");
    }
}