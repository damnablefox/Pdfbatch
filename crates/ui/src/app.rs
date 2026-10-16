//! 应用外壳：窗口、主题、顶栏、参数面板分发与底部执行条。

use std::path::PathBuf;
use std::time::Duration;

use eframe::egui;

use crate::files::FileList;
use crate::helpers::{counts, danger, warn};
use crate::job::{
    self, BookmarkDraft, CommonOptions, FillAction, FillDraft, JobProgress, Mode, OutputChoice,
    PageSizeDraft,
};
use crate::pickers::{PickPurpose, Pickers};
use crate::theme::{self, regular, semibold, ThemeKind, Tokens};
use crate::widgets;

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
    pub(crate) mode: Mode,
    pub(crate) theme: ThemeKind,
    /// 字体只安装一次。
    pub(crate) fonts_installed: bool,
    /// 已经应用到 egui 的主题（首帧之前是 `None`）。
    pub(crate) applied: Option<ThemeKind>,
    pub(crate) files: FileList,
    pub(crate) fill: FillDraft,
    pub(crate) pagesize: PageSizeDraft,
    pub(crate) bookmarks: BookmarkDraft,
    pub(crate) common: CommonOptions,
    pub(crate) pickers: Pickers,
    pub(crate) run: Option<job::JobRun>,
    /// 正在运行任务的进度快照（用于底部执行条）。
    pub(crate) progress: Option<JobProgress>,
    /// 正在运行的「查找匹配」任务。
    pub(crate) probe: Option<job::ProbeRun>,
    /// 最近一次查找的结果（删/改模式据此判断能否执行）。
    pub(crate) probe_report: Option<job::ProbeReport>,
    /// 浮动提示：消息 + 出现时刻。
    pub(crate) toast: Option<(String, f64)>,
    /// 是否有文件正被拖到窗口上（显示放置提示）。
    pub(crate) hover_drop: bool,
    /// 文件列表「文件名」列的宽度（表头分隔条可拖，无上限）。
    pub(crate) col_name_width: f32,
    /// 按文件名排序的方向：`None` 未排序、`Some(true)` 升序、`Some(false)` 降序。
    pub(crate) name_sort: Option<bool>,
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
            pagesize: PageSizeDraft::default(),
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
            name_sort: None,
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
        // 左面板下限取三个标签页里最宽的一行并排选项（按当前字体实测）：
        // 拖到下限时并排选项也不会被挤到下一行。
        egui::Panel::left("pdfbatch-params")
            .resizable(true)
            .default_size(360.0)
            .min_size(params_min_width(ui))
            .max_size(560.0)
            .show(ui, |ui| self.params_panel(ui, t));
        // 输出区提升为顶层面板（与左侧参数同级）：独占文件列表下方的整条区域，
        // 不再嵌在 CentralPanel 内，避免表格的列宽手柄绘制/交互穿透到输出区。
        // 高度固定：面板内始终渲染全部选项（当前方式用不到的置灰占位），
        // 切换输出方式时分隔线不会跳动。
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
                    self.apply_probe(&report);
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

    /// 把逐文件命中数写进文件列表的「提示」列。
    fn apply_probe(&mut self, report: &job::ProbeReport) {
        for (index, item) in self.files.items.iter_mut().enumerate() {
            match report.per_file.get(index) {
                Some(&hits) if hits > 0 => {
                    item.hint = format!("匹配 {hits} 个");
                    item.hint_hit = Some(true);
                }
                Some(_) => {
                    item.hint = "无匹配".to_string();
                    item.hint_hit = Some(false);
                }
                None => {
                    item.hint.clear();
                    item.hint_hit = None;
                }
            }
        }
    }

    /// 启动一次「查找匹配」（删/改模式执行前的统计）。
    pub(crate) fn start_probe(&mut self, ctx: &egui::Context) {
        if self.probe.is_some() || self.run.is_some() {
            return;
        }
        let inputs = self.files.to_inputs();
        match job::build_probe_request(self.mode, &self.fill, &self.common, inputs) {
            Ok(request) => {
                let fingerprint = job::matcher_fingerprint(&self.fill);
                self.probe_report = None;
                // 重新查找前先清掉上一次的逐文件提示。
                for item in &mut self.files.items {
                    item.hint.clear();
                    item.hint_hit = None;
                }
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
        match job::build_request(
            self.mode,
            &self.fill,
            &self.pagesize,
            &self.bookmarks,
            &self.common,
            inputs,
        ) {
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

    pub(crate) fn ask_files(&mut self, ctx: &egui::Context) {
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

    pub(crate) fn ask_folder(&mut self, ctx: &egui::Context) {
        if self.pickers.is_showing() {
            return;
        }
        let pick = rfd::AsyncFileDialog::new().set_title("选择包含 PDF 的文件夹").pick_folder();
        self.pickers.spawn(PickPurpose::AddFolder, Some(ctx.clone()), async move {
            pick.await.map(|handle| vec![handle.path().to_path_buf()]).unwrap_or_default()
        });
    }

    pub(crate) fn ask_output_dir(&mut self, ctx: &egui::Context) {
        if self.pickers.is_showing() {
            return;
        }
        let pick = rfd::AsyncFileDialog::new().set_title("选择输出目录").pick_folder();
        self.pickers.spawn(PickPurpose::OutputDir, Some(ctx.clone()), async move {
            pick.await.map(|handle| vec![handle.path().to_path_buf()]).unwrap_or_default()
        });
    }

    pub(crate) fn ask_spec(&mut self, ctx: &egui::Context) {
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
                Mode::PageSize => self.pagesize_params(ui, t),
                Mode::Bookmarks => self.bookmark_params(ui, t),
            }
            ui.add_space(16.0);
        });
    }

    fn run_bar(&mut self, ui: &mut egui::Ui, t: Tokens) {
        let busy = self.run.is_some();
        let preflight = job::preflight(
            self.mode,
            &self.fill,
            &self.pagesize,
            &self.bookmarks,
            &self.common,
            &self.files.to_inputs(),
        );
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
                        let (text, color) = match self.mode {
                            Mode::Bookmarks => {
                                (format!("将对 {file_count} 个文件执行「书签」"), t.text_muted)
                            }
                            Mode::PageSize => (
                                format!(
                                    "将对 {file_count} 个文件执行「页面尺寸 · {}」",
                                    self.pagesize.method.label()
                                ),
                                t.text_muted,
                            ),
                            Mode::Fill => match self.fill.action {
                                FillAction::Add => {
                                    (format!("将对 {file_count} 个文件执行「新增填充层」"), t.text_muted)
                                }
                                FillAction::Edit => {
                                    if probe_ok {
                                        let total =
                                            self.probe_report.as_ref().map(|report| report.total).unwrap_or(0);
                                        (
                                            format!(
                                                "将对 {file_count} 个文件执行「{}」，命中 {total} 个填充层",
                                                self.fill.edit_mode.label()
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

/// 左侧参数面板的最小宽度：三个标签页里最宽的一行并排选项 + 面板自身开销。
///
/// 下限取三者最大值并全局统一，这样切换标签页时面板宽度不会被悄悄改动。
fn params_min_width(ui: &egui::Ui) -> f32 {
    let font = regular(13.0);
    // 面板自身开销：垂直滚动条 + 内边距。
    const CHROME: f32 = 26.0;
    let width = |label: &str, options: &[&str]| widgets::row_width(ui, label, options, font.clone());
    [
        // 填充区域：查找条件的「填充区域」四档（通常是最宽的一行）。
        width("填充区域", &["不限", "整页", "按宽高", "按起点+宽高"]),
        width("", &["整页填充", "自定义矩形填充"]),
        width("单位", &["pt", "mm", "cm", "in"]),
        width("", &["删除", "修改"]),
        width("", &["置于顶层", "置于底层"]),
        // 页面尺寸：方式、目标尺寸来源、方向、旋转、图像旋转。
        width("", &["按目标尺寸缩放", "扫描件适配"]),
        width("", &["预设", "自定义"]),
        width("方向", &["纵向", "横向", "跟随原页面"]),
        width("", &["保持", "0°", "90°", "180°", "270°"]),
        width("图像旋转", &["自动", "保持", "0°", "90°", "180°", "270°"]),
        // 书签：顶部三个按钮。
        width("", &["选择 .txt 文件", "填入示例", "清空"]),
    ]
    .into_iter()
    .fold(0.0_f32, f32::max)
        + CHROME
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
