//! 执行：把界面上的参数整理成一次可运行的请求，并在后台线程里跑批量任务。

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use pdfbatch_core::action::{OutcomeStatus, run_jobs};
use pdfbatch_core::batch::{self, Input, Job, Naming, OutputMode};
use pdfbatch_core::spec::{self, SpecSections};
use pdfbatch_core::{Action, Blend, FillChange, FillOp, FillStyle, Layer, Matcher, RectMatch};

use crate::files::FileStatus;

/// 两种模式。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// 给页面盖纯色填充层。
    Fill,
    /// 从文本导入书签。
    Bookmarks,
}

impl Mode {
    pub const ALL: [Mode; 2] = [Mode::Fill, Mode::Bookmarks];

    pub fn label(self) -> &'static str {
        match self {
            Mode::Fill => "填充区域",
            Mode::Bookmarks => "书签",
        }
    }

    /// 同目录另存时的默认文件名后缀。
    pub fn suffix(self) -> &'static str {
        match self {
            Mode::Fill => "_filled",
            Mode::Bookmarks => "_bookmarked",
        }
    }
}

/// 输出位置的选择。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutputChoice {
    /// 与原文件同目录，文件名加后缀。
    SameFolder,
    /// 写到指定目录（保留目录输入的子路径）。
    Dir,
    /// 覆盖原文件。
    InPlace,
}

/// 填充区域的三种操作：增、删、改。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FillAction {
    Add,
    Erase,
    Modify,
}

impl FillAction {
    pub const ALL: [FillAction; 3] = [FillAction::Add, FillAction::Erase, FillAction::Modify];

    pub fn label(self) -> &'static str {
        match self {
            FillAction::Add => "新增",
            FillAction::Erase => "删除",
            FillAction::Modify => "修改",
        }
    }
}

/// 查找时按哪种粒度比对填充区域。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MatchRect {
    /// 不限区域。
    Any,
    /// 只匹配整页填充。
    FullPage,
    /// 只比宽高（不看左下起点）。
    Size,
    /// 完整匹配左下起点与宽高。
    Rect,
}

impl MatchRect {
    pub const ALL: [MatchRect; 4] =
        [MatchRect::Any, MatchRect::FullPage, MatchRect::Size, MatchRect::Rect];

    pub fn label(self) -> &'static str {
        match self {
            MatchRect::Any => "不限",
            MatchRect::FullPage => "整页",
            MatchRect::Size => "按宽高",
            MatchRect::Rect => "按起点+宽高",
        }
    }
}

/// 要把命中的填充区域改成什么样。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChangeRect {
    /// 保持原样。
    Keep,
    /// 改成整页。
    FullPage,
    /// 改成具体矩形。
    Rect,
}

impl ChangeRect {
    pub const ALL: [ChangeRect; 3] = [ChangeRect::Keep, ChangeRect::FullPage, ChangeRect::Rect];

    pub fn label(self) -> &'static str {
        match self {
            ChangeRect::Keep => "保持",
            ChangeRect::FullPage => "改成整页",
            ChangeRect::Rect => "改成矩形",
        }
    }
}

/// 填充区域模式的参数草稿。
#[derive(Clone, Debug)]
pub struct FillDraft {
    /// 增 / 删 / 改。
    pub action: FillAction,

    // —— 新增用：要盖上去的样式 ——
    pub whole_page: bool,
    /// 用户空间矩形 `[x, y, 宽, 高]`（点，原点在页面左下角）。
    pub rect: [f64; 4],
    /// 颜色，按 sRGB 存储（取色控件与 PDF 的 `rg` 用的是同一套取值）。
    pub color: egui::Color32,
    pub opacity: f64,
    pub blend: Blend,
    pub layer: Layer,
    /// 先摘掉本工具此前加的填充层再重画。
    pub replace: bool,

    // —— 查找用：匹配条件 ——
    pub match_rect: MatchRect,
    /// `MatchRect::Size` 时用到的宽高。
    pub match_size: [f64; 2],
    /// `MatchRect::Rect` 时用到的左下起点与宽高。
    pub match_rect_value: [f64; 4],
    pub match_color_on: bool,
    pub match_color: egui::Color32,
    pub match_opacity_on: bool,
    pub match_opacity: f64,
    pub match_blend_on: bool,
    pub match_blend: Blend,
    pub match_layer_on: bool,
    pub match_layer: Layer,
    /// 数值比较容差（矩形、颜色、透明度都用它）。0 表示精确相等。
    pub tolerance: f64,

    // —— 修改用：更改要求（每项可单独开关） ——
    pub change_rect: ChangeRect,
    pub change_rect_value: [f64; 4],
    pub change_color_on: bool,
    pub change_color: egui::Color32,
    pub change_opacity_on: bool,
    pub change_opacity: f64,
    pub change_blend_on: bool,
    pub change_blend: Blend,
    pub change_layer_on: bool,
    pub change_layer: Layer,

    /// 页范围原文（`all` 或 `1-5,8,10-`）。
    pub pages: String,
}

impl Default for FillDraft {
    fn default() -> Self {
        let default_rect = [72.0, 72.0, 400.0, 150.0];
        let default_color = egui::Color32::from_rgb(255, 235, 140);
        Self {
            action: FillAction::Add,

            whole_page: true,
            rect: default_rect,
            color: default_color,
            opacity: 0.2,
            blend: Blend::Normal,
            layer: Layer::Top,
            replace: false,

            match_rect: MatchRect::Any,
            match_size: [400.0, 150.0],
            match_rect_value: default_rect,
            match_color_on: false,
            match_color: default_color,
            match_opacity_on: false,
            match_opacity: 0.2,
            match_blend_on: false,
            match_blend: Blend::Normal,
            match_layer_on: false,
            match_layer: Layer::Top,
            tolerance: 0.0,

            change_rect: ChangeRect::Keep,
            change_rect_value: default_rect,
            change_color_on: false,
            change_color: default_color,
            change_opacity_on: false,
            change_opacity: 0.2,
            change_blend_on: false,
            change_blend: Blend::Normal,
            change_layer_on: false,
            change_layer: Layer::Top,

            pages: "all".to_string(),
        }
    }
}

/// 把 sRGB 取色控件的颜色转成 PDF 的 0.0–1.0 分量。
fn srgb(color: egui::Color32) -> [f64; 3] {
    [
        f64::from(color.r()) / 255.0,
        f64::from(color.g()) / 255.0,
        f64::from(color.b()) / 255.0,
    ]
}

/// 由草稿里的匹配条件得到查找谓词。
fn fill_matcher(fill: &FillDraft) -> Matcher {
    Matcher {
        rect: match fill.match_rect {
            MatchRect::Any => None,
            MatchRect::FullPage => Some(RectMatch::FullPage),
            MatchRect::Size => Some(RectMatch::Size(fill.match_size)),
            MatchRect::Rect => Some(RectMatch::Rect(fill.match_rect_value)),
        },
        color: fill.match_color_on.then(|| srgb(fill.match_color)),
        opacity: fill.match_opacity_on.then_some(fill.match_opacity),
        blend: fill.match_blend_on.then_some(fill.match_blend),
        layer: fill.match_layer_on.then_some(fill.match_layer),
        tolerance: fill.tolerance,
    }
}

/// 由草稿里的更改要求得到改写内容；全为 `None` 时返回空（表示没勾任何项）。
fn fill_change(fill: &FillDraft) -> FillChange {
    FillChange {
        rect: (fill.change_rect == ChangeRect::Rect).then_some(fill.change_rect_value),
        full_page: (fill.change_rect == ChangeRect::FullPage).then_some(true),
        color: fill.change_color_on.then(|| srgb(fill.change_color)),
        opacity: fill.change_opacity_on.then_some(fill.change_opacity),
        blend: fill.change_blend_on.then_some(fill.change_blend),
        layer: fill.change_layer_on.then_some(fill.change_layer),
    }
}

/// 匹配条件的指纹：条件一变，之前的查找结果就作废，需重新查找。
pub fn matcher_fingerprint(fill: &FillDraft) -> String {
    format!(
        "rect={:?} size={:?} rectv={:?} color={}:{:?} opacity={}:{} blend={}:{:?} layer={}:{:?} tol={} pages={}",
        fill.match_rect,
        fill.match_size,
        fill.match_rect_value,
        fill.match_color_on,
        fill.match_color,
        fill.match_opacity_on,
        fill.match_opacity,
        fill.match_blend_on,
        fill.match_blend,
        fill.match_layer_on,
        fill.match_layer,
        fill.tolerance,
        fill.pages,
    )
}

/// 页范围原文（空则按 `all`）。
fn pages_text(fill: &FillDraft) -> String {
    if fill.pages.trim().is_empty() {
        "all".to_string()
    } else {
        fill.pages.clone()
    }
}

/// 书签模式的参数草稿。
#[derive(Clone, Debug, Default)]
pub struct BookmarkDraft {
    /// 选中的文本文件（仅作记录；实际用的是 `text`）。
    pub source: Option<PathBuf>,
    pub text: String,
    pub zero_based: bool,
    pub strict: bool,
    pub replace: bool,
}

/// 两种模式共用的输出与读取选项。
#[derive(Clone, Debug)]
pub struct CommonOptions {
    pub output: OutputChoice,
    /// 指定输出目录（`OutputChoice::Dir` 时需要）。
    pub dir: Option<PathBuf>,
    /// 输出文件名前缀（同目录另存与指定目录都适用）。
    pub prefix: String,
    /// 输出文件名后缀。
    pub suffix: String,
    /// 加密 PDF 的密码；空字符串表示没有。
    pub password: String,
    /// 整体重写文件（体积更小，但会让已有数字签名失效）。
    pub full_save: bool,
}

impl Default for CommonOptions {
    fn default() -> Self {
        Self {
            output: OutputChoice::SameFolder,
            dir: None,
            prefix: String::new(),
            suffix: Mode::Fill.suffix().to_string(),
            password: String::new(),
            full_save: false,
        }
    }
}

/// 一次任务的不可变快照：执行期间改界面参数不会影响它。
#[derive(Clone, Debug)]
pub struct Request {
    pub action: Action,
    pub inputs: Vec<Input>,
    pub output: OutputMode,
    pub password: Option<String>,
    pub full_save: bool,
}

impl Request {
    /// 把输入规划成逐个文件的输出任务。
    pub fn jobs(&self) -> Result<Vec<Job>, String> {
        batch::plan(&self.inputs, &self.output).map_err(|error| error.to_string())
    }
}

/// 参数是否已经可以执行。返回给用户看的消息（空表示可以）。
///
/// 不碰文件系统，因此可以在每一帧调用（执行按钮的可用状态与原因提示都靠它）。
pub fn preflight(
    mode: Mode,
    fill: &FillDraft,
    bookmarks: &BookmarkDraft,
    common: &CommonOptions,
    inputs: &[Input],
) -> Result<(), String> {
    if inputs.is_empty() {
        return Err("请先添加 PDF 文件或文件夹".to_string());
    }
    if common.output == OutputChoice::Dir && common.dir.is_none() {
        return Err("请先选择输出目录".to_string());
    }
    // 覆盖原文件时不改名，前后缀不参与，不必校验。
    if common.output != OutputChoice::InPlace {
        validate_naming(&common.prefix, &common.suffix)?;
    }
    match mode {
        Mode::Fill => match fill.action {
            FillAction::Add => {
                if !fill.whole_page && !(fill.rect[2] > 0.0 && fill.rect[3] > 0.0) {
                    return Err("填充矩形的宽和高必须大于 0".to_string());
                }
                validate_page_syntax(&fill.pages)?;
            }
            FillAction::Erase => validate_page_syntax(&fill.pages)?,
            FillAction::Modify => {
                if fill_change(fill).is_empty() {
                    return Err("修改模式至少要勾选一项要改的属性".to_string());
                }
                validate_page_syntax(&fill.pages)?;
            }
        },
        Mode::Bookmarks => {
            if bookmarks.text.trim().is_empty() {
                return Err("书签文本是空的".to_string());
            }
            // 结构校验用极大的页数，避免把"页码越界"误判成错误——越界在运行时按各文档裁定。
            match spec::split_sections(&bookmarks.text).map_err(|error| error.to_string())? {
                SpecSections::Shared(text) => {
                    spec::parse_spec(&text, bookmarks.zero_based, bookmarks.strict, usize::MAX)
                        .map_err(|error| error.to_string())?;
                }
                SpecSections::Sections(sections) => {
                    // 段头与文件的绑定必须先成立（找不到 / 冲突都会报错）。
                    spec::bind_sections(&sections, inputs)?;
                    for section in &sections {
                        spec::parse_spec(&section.text, bookmarks.zero_based, bookmarks.strict, usize::MAX)
                            .map_err(|error| error.to_string())?;
                    }
                }
            }
        }
    }
    Ok(())
}

/// 输出文件名的合法性检查：Windows 不允许的字符，以及结尾的空格与点。
pub fn validate_naming(prefix: &str, suffix: &str) -> Result<(), String> {
    const ILLEGAL: [char; 9] = ['\\', '/', ':', '*', '?', '"', '<', '>', '|'];
    for (label, text) in [("前缀", prefix), ("后缀", suffix)] {
        if let Some(ch) = text.chars().find(|c| ILLEGAL.contains(c) || c.is_control()) {
            return Err(format!("文件名{label}不能包含字符 `{ch}`"));
        }
        if text.ends_with(' ') || text.ends_with('.') {
            return Err(format!("文件名{label}不能以空格或点结尾"));
        }
    }
    Ok(())
}

/// 实际生效的命名。
///
/// 同目录另存时前后缀都为空会回退到模式默认后缀，保证绝不覆盖原文件；
/// 指定目录时前后缀都为空则保持原名（与命令行一致）。
pub fn effective_naming(mode: Mode, common: &CommonOptions) -> Naming {
    let prefix = common.prefix.clone();
    let suffix = common.suffix.clone();
    let both_empty = prefix.is_empty() && suffix.trim().is_empty();
    if common.output == OutputChoice::SameFolder && both_empty {
        Naming {
            prefix,
            suffix: mode.suffix().to_string(),
        }
    } else {
        Naming { prefix, suffix }
    }
}

/// 把界面参数整理成一次请求；所有非法输入在这里被拒绝（返回给用户看的消息）。
pub fn build_request(
    mode: Mode,
    fill: &FillDraft,
    bookmarks: &BookmarkDraft,
    common: &CommonOptions,
    inputs: Vec<Input>,
) -> Result<Request, String> {
    preflight(mode, fill, bookmarks, common, &inputs)?;

    let output = match common.output {
        OutputChoice::SameFolder => OutputMode::SameFolder {
            naming: effective_naming(mode, common),
        },
        OutputChoice::Dir => OutputMode::Dir {
            dir: common.dir.clone().expect("preflight 已确认目录存在"),
            naming: effective_naming(mode, common),
        },
        OutputChoice::InPlace => OutputMode::InPlace,
    };

    let action = match mode {
        Mode::Fill => {
            let op = match fill.action {
                FillAction::Add => {
                    let rect = if fill.whole_page { None } else { Some(fill.rect) };
                    FillOp::Add {
                        style: FillStyle {
                            rect,
                            color: srgb(fill.color),
                            opacity: fill.opacity.clamp(0.0, 1.0),
                            blend: fill.blend,
                            layer: fill.layer,
                        },
                        replace: fill.replace,
                    }
                }
                FillAction::Erase => FillOp::Remove(fill_matcher(fill)),
                FillAction::Modify => FillOp::Modify {
                    matcher: fill_matcher(fill),
                    change: fill_change(fill),
                },
            };
            Action::Fill {
                op,
                pages: pages_text(fill),
            }
        }
        Mode::Bookmarks => Action::Bookmarks {
            // 有段头就按文件分配；没有段头则所有文件共用这份文本。
            spec: pdfbatch_core::bookmark_spec(&bookmarks.text, &inputs)?,
            zero_based: bookmarks.zero_based,
            strict: bookmarks.strict,
            replace: bookmarks.replace,
        },
    };

    let password = {
        let trimmed = common.password.trim();
        (!trimmed.is_empty()).then(|| trimmed.to_string())
    };

    Ok(Request {
        action,
        inputs,
        output,
        password,
        full_save: common.full_save,
    })
}

/// 构造一次「只查找」的请求：删/改模式下，执行前先统计会命中多少填充层。
///
/// 与 `build_request` 不同，这里不要求配置更改要求——查找只关心匹配条件本身。
pub fn build_probe_request(
    mode: Mode,
    fill: &FillDraft,
    common: &CommonOptions,
    inputs: Vec<Input>,
) -> Result<Request, String> {
    if inputs.is_empty() {
        return Err("请先添加 PDF 文件或文件夹".to_string());
    }
    if mode != Mode::Fill {
        return Err("只有填充区域需要查找".to_string());
    }
    validate_page_syntax(&fill.pages)?;
    let action = Action::Fill {
        // 用「删除」包裹匹配条件：probe 只看 matcher，不看操作种类。
        op: FillOp::Remove(fill_matcher(fill)),
        pages: pages_text(fill),
    };
    let password = {
        let trimmed = common.password.trim();
        (!trimmed.is_empty()).then(|| trimmed.to_string())
    };
    Ok(Request {
        action,
        inputs,
        // 查找不写文件，输出位置只作占位。
        output: OutputMode::SameFolder {
            naming: Naming {
                prefix: String::new(),
                suffix: String::new(),
            },
        },
        password,
        full_save: false,
    })
}

/// 页范围的**语法**校验：只查写法对不对、起止是否颠倒，不查是否越界
/// （越界要按每个文档的实际页数裁定，运行时逐文件处理）。
pub(crate) fn validate_page_syntax(spec: &str) -> Result<(), String> {
    // 用一个足够大的假页数只做语法检查：first/last、负索引、`..`、except 都能解析；
    // 越不越界留给运行时按各文件的实际页数裁定（例如 999999 在这里不算错）。
    pdfbatch_core::pages::parse_pages(spec, 1_000_000).map(|_| ())
}

/// 后台任务的进度快照。
#[derive(Clone, Debug)]
pub struct JobProgress {
    pub total: usize,
    pub done: usize,
    /// 与文件列表一一对应的逐行状态。
    pub statuses: Vec<FileStatus>,
    /// 正在处理的文件名。
    pub current: String,
    pub finished: bool,
    /// 是否被用户取消。
    pub cancelled: bool,
}

impl JobProgress {
    fn new(total: usize) -> Self {
        Self {
            total,
            done: 0,
            statuses: vec![FileStatus::Pending; total],
            current: String::new(),
            finished: false,
            cancelled: false,
        }
    }
}

/// 一次正在运行（或刚结束）的后台任务。
pub struct JobRun {
    cancel: Arc<AtomicBool>,
    progress: Arc<Mutex<JobProgress>>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl JobRun {
    /// 读取进度快照。
    pub fn snapshot(&self) -> JobProgress {
        self.progress.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    /// 请求取消（已处理的文件保留，剩下的不再处理）。
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    /// 等待工作线程结束。
    pub fn join(&mut self) {
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for JobRun {
    fn drop(&mut self) {
        // 关闭窗口时任务还在跑：置取消标志并等它收尾（写文件是"临时文件 + 改名"，不会写出半个文件）。
        self.cancel();
        self.join();
    }
}

/// 在后台线程里执行请求。非法输入（例如多个输入写到同一个文件）在这里返回错误。
pub fn spawn_job(request: Request, ctx: egui::Context) -> Result<JobRun, String> {
    let jobs = request.jobs()?;
    let total = jobs.len();
    let cancel = Arc::new(AtomicBool::new(false));
    let progress = Arc::new(Mutex::new(JobProgress::new(total)));

    let cancel_worker = cancel.clone();
    let progress_worker = progress.clone();
    let action = request.action.clone();
    let output = request.output.clone();
    let password = request.password.clone();
    let full_save = request.full_save;

    // 先把第一个文件标成"处理中"，让界面一启动就有反馈。
    if let Some(first) = jobs.first() {
        if let Ok(mut p) = progress.lock() {
            p.statuses[0] = FileStatus::Running;
            p.current = file_name(&first.input);
        }
    }

    let handle = std::thread::Builder::new()
        .name("pdfbatch-job".into())
        .spawn(move || {
            let jobs_ref = &jobs;
            let mut next = 0usize;
            run_jobs(
                jobs_ref,
                &output,
                password.as_deref(),
                full_save,
                &action,
                &cancel_worker,
                |outcome| {
                    let status = match &outcome.status {
                        OutcomeStatus::Done(note) if note.warnings.is_empty() => FileStatus::Done(note.summary.clone()),
                        OutcomeStatus::Done(note) => {
                            FileStatus::Done(format!("{}（{} 条警告）", note.summary, note.warnings.len()))
                        }
                        OutcomeStatus::Skipped(why) => FileStatus::Skipped(why.clone()),
                        OutcomeStatus::Failed(error) => FileStatus::Failed(error.to_string()),
                    };
                    if let Ok(mut p) = progress_worker.lock() {
                        if next < p.statuses.len() {
                            p.statuses[next] = status;
                        }
                        next += 1;
                        p.done = next;
                        if next < p.statuses.len() {
                            p.statuses[next] = FileStatus::Running;
                            p.current = file_name(&jobs_ref[next].input);
                        } else {
                            p.current.clear();
                        }
                    }
                    ctx.request_repaint();
                },
            );
            if let Ok(mut p) = progress_worker.lock() {
                p.finished = true;
                p.cancelled = cancel_worker.load(Ordering::Relaxed);
                p.current.clear();
            }
            ctx.request_repaint();
        })
        .map_err(|error| format!("无法启动后台任务：{error}"))?;

    Ok(JobRun {
        cancel,
        progress,
        handle: Some(handle),
    })
}

/// 一次查找的结果。
#[derive(Clone, Debug)]
pub struct ProbeReport {
    /// 生成它时的匹配条件指纹；条件变了这份结果就作废。
    pub fingerprint: String,
    /// 命中的填充层总数。
    pub total: usize,
}

/// 一次正在运行（或刚结束）的查找任务。
pub struct ProbeRun {
    result: Arc<Mutex<Option<Result<ProbeReport, String>>>>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl ProbeRun {
    /// 取出结果；还没跑完时返回 `None`。
    pub fn take(&mut self) -> Option<Result<ProbeReport, String>> {
        self.result.lock().unwrap_or_else(PoisonError::into_inner).take()
    }
}

impl Drop for ProbeRun {
    fn drop(&mut self) {
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

/// 在后台线程里统计匹配数量（只读文件，不改盘）。
pub fn spawn_probe(request: Request, fingerprint: String, ctx: egui::Context) -> Result<ProbeRun, String> {
    let result = Arc::new(Mutex::new(None));
    let result_worker = result.clone();
    let handle = std::thread::Builder::new()
        .name("pdfbatch-probe".into())
        .spawn(move || {
            let outcome = probe_all(&request, &fingerprint);
            *result_worker.lock().unwrap_or_else(PoisonError::into_inner) = Some(outcome);
            ctx.request_repaint();
        })
        .map_err(|error| format!("无法启动查找任务：{error}"))?;
    Ok(ProbeRun { result, handle: Some(handle) })
}

fn probe_all(request: &Request, fingerprint: &str) -> Result<ProbeReport, String> {
    let mut total = 0usize;
    for input in &request.inputs {
        let mut doc = pdfbatch_core::pdf::open(&input.path, request.password.as_deref())
            .map_err(|error| format!("{}：{error}", input.path.display()))?;
        total += request
            .action
            .probe(&mut doc)
            .map_err(|error| format!("{}：{error}", input.path.display()))?;
    }
    Ok(ProbeReport {
        fingerprint: fingerprint.to_string(),
        total,
    })
}

fn file_name(path: &std::path::Path) -> String {
    path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| path.display().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use pdfbatch_core::batch::Input;
    use std::path::PathBuf;

    fn input(path: &str) -> Input {
        Input {
            path: PathBuf::from(path),
            relative: PathBuf::from(PathBuf::from(path).file_name().unwrap()),
        }
    }

    fn fill_request(rect_ok: bool, pages: &str) -> Result<Request, String> {
        let fill = FillDraft {
            whole_page: false,
            rect: if rect_ok { [10.0, 10.0, 100.0, 50.0] } else { [10.0, 10.0, 0.0, 50.0] },
            pages: pages.to_string(),
            ..FillDraft::default()
        };
        build_request(Mode::Fill, &fill, &BookmarkDraft::default(), &CommonOptions::default(), vec![input("D:/a.pdf")])
    }

    #[test]
    fn empty_input_is_rejected() {
        let error = build_request(
            Mode::Fill,
            &FillDraft::default(),
            &BookmarkDraft::default(),
            &CommonOptions::default(),
            vec![],
        )
        .unwrap_err();
        assert!(error.contains("请先添加"), "{error}");
    }

    #[test]
    fn a_zero_sized_rect_is_rejected() {
        assert!(fill_request(false, "all").unwrap_err().contains("宽和高"));
    }

    #[test]
    fn page_syntax_is_checked_before_running() {
        assert!(fill_request(true, "all").is_ok());
        assert!(fill_request(true, "1-5,8").is_ok());
        assert!(fill_request(true, "10-").is_ok());
        assert!(fill_request(true, "5-3").unwrap_err().contains("起点大于终点"));
        assert!(fill_request(true, "0").unwrap_err().contains("从 1 开始"));
        assert!(fill_request(true, "x").unwrap_err().contains("不是数字"));
        // 越界不在这里拦下（没有文档页数），留给运行时按各文件裁定。
        assert!(fill_request(true, "999999").is_ok());
    }

    #[test]
    fn bookmark_text_must_parse() {
        let bad = BookmarkDraft {
            text: "只有标题没有页码\n".to_string(),
            ..BookmarkDraft::default()
        };
        let error =
            build_request(Mode::Bookmarks, &FillDraft::default(), &bad, &CommonOptions::default(), vec![input("D:/a.pdf")])
                .unwrap_err();
        assert!(error.contains("第 1 行"), "{error}");

        let empty = BookmarkDraft::default();
        assert!(
            build_request(Mode::Bookmarks, &FillDraft::default(), &empty, &CommonOptions::default(), vec![input("D:/a.pdf")])
                .unwrap_err()
                .contains("空的")
        );

        let good = BookmarkDraft {
            text: "第一章 | 1\n\t1.1 | 2\n".to_string(),
            ..BookmarkDraft::default()
        };
        assert!(
            build_request(Mode::Bookmarks, &FillDraft::default(), &good, &CommonOptions::default(), vec![input("D:/a.pdf")])
                .is_ok()
        );
    }

    #[test]
    fn bookmark_sections_are_bound_to_files() {
        let bookmarks = BookmarkDraft {
            text: "# 文件：a.pdf\n甲 | 1\n# 文件：b.pdf\n乙 | 2\n".to_string(),
            ..BookmarkDraft::default()
        };
        let request = build_request(
            Mode::Bookmarks,
            &FillDraft::default(),
            &bookmarks,
            &CommonOptions::default(),
            vec![input("D:/x/a.pdf"), input("D:/y/b.pdf")],
        )
        .unwrap();
        match request.action {
            Action::Bookmarks {
                spec: pdfbatch_core::BookmarkSpec::PerFile(entries),
                ..
            } => assert_eq!(entries.len(), 2),
            other => panic!("应当是分段书签：{other:?}"),
        }
    }

    #[test]
    fn a_section_target_without_a_file_is_rejected() {
        let bookmarks = BookmarkDraft {
            text: "# 文件：missing.pdf\n甲 | 1\n".to_string(),
            ..BookmarkDraft::default()
        };
        let error = build_request(
            Mode::Bookmarks,
            &FillDraft::default(),
            &bookmarks,
            &CommonOptions::default(),
            vec![input("D:/x/a.pdf")],
        )
        .unwrap_err();
        assert!(error.contains("找不到"), "{error}");
    }

    #[test]
    fn out_file_mode_is_not_offered_but_dir_requires_a_dir() {
        let common = CommonOptions {
            output: OutputChoice::Dir,
            dir: None,
            ..CommonOptions::default()
        };
        let error =
            build_request(Mode::Fill, &FillDraft::default(), &BookmarkDraft::default(), &common, vec![input("D:/a.pdf")])
                .unwrap_err();
        assert!(error.contains("输出目录"), "{error}");
    }

    #[test]
    fn prefix_only_does_not_fall_back_to_a_default_suffix() {
        // 只填前缀、清空后缀：同目录另存也不该再补默认后缀。
        let common = CommonOptions {
            prefix: "new_".to_string(),
            suffix: String::new(),
            ..CommonOptions::default()
        };
        let request = build_request(
            Mode::Fill,
            &FillDraft::default(),
            &BookmarkDraft::default(),
            &common,
            vec![input("D:/a.pdf")],
        )
        .unwrap();
        match request.output {
            OutputMode::SameFolder { naming } => {
                assert_eq!(naming.prefix, "new_");
                assert!(naming.suffix.is_empty(), "不该补默认后缀");
            }
            other => panic!("应当是同目录另存：{other:?}"),
        }
    }

    #[test]
    fn both_empty_falls_back_to_the_default_suffix_in_same_folder() {
        let common = CommonOptions {
            prefix: String::new(),
            suffix: String::new(),
            ..CommonOptions::default()
        };
        let request = build_request(
            Mode::Fill,
            &FillDraft::default(),
            &BookmarkDraft::default(),
            &common,
            vec![input("D:/a.pdf")],
        )
        .unwrap();
        match request.output {
            OutputMode::SameFolder { naming } => assert_eq!(naming.suffix, "_filled", "回退默认后缀避免覆盖"),
            other => panic!("应当是同目录另存：{other:?}"),
        }
    }

    #[test]
    fn dir_naming_keeps_an_empty_suffix() {
        let common = CommonOptions {
            output: OutputChoice::Dir,
            dir: Some(PathBuf::from("D:/out")),
            prefix: "new_".to_string(),
            suffix: String::new(),
            ..CommonOptions::default()
        };
        let request = build_request(
            Mode::Fill,
            &FillDraft::default(),
            &BookmarkDraft::default(),
            &common,
            vec![input("D:/a.pdf")],
        )
        .unwrap();
        match request.output {
            OutputMode::Dir { naming, .. } => {
                assert_eq!(naming.prefix, "new_");
                assert!(naming.suffix.is_empty(), "指定目录不做默认后缀回退");
            }
            other => panic!("应当是指定目录：{other:?}"),
        }
    }

    #[test]
    fn illegal_naming_is_rejected() {
        let common = CommonOptions {
            prefix: "a/b".to_string(),
            ..CommonOptions::default()
        };
        let error = build_request(
            Mode::Fill,
            &FillDraft::default(),
            &BookmarkDraft::default(),
            &common,
            vec![input("D:/a.pdf")],
        )
        .unwrap_err();
        assert!(error.contains("不能包含"), "{error}");
    }

    /// 取色控件给的是 sRGB 分量，必须原样写进 PDF（不能当线性值处理）。
    #[test]
    fn fill_color_is_written_as_srgb() {
        let fill = FillDraft {
            color: egui::Color32::from_rgb(227, 237, 203),
            ..FillDraft::default()
        };
        let request = build_request(
            Mode::Fill,
            &fill,
            &BookmarkDraft::default(),
            &CommonOptions::default(),
            vec![input("D:/a.pdf")],
        )
        .unwrap();
        match request.action {
            Action::Fill { op: FillOp::Add { style, .. }, .. } => {
                assert!((style.color[0] - 227.0 / 255.0).abs() < 1e-9, "{:?}", style.color);
                assert!((style.color[1] - 237.0 / 255.0).abs() < 1e-9, "{:?}", style.color);
                assert!((style.color[2] - 203.0 / 255.0).abs() < 1e-9, "{:?}", style.color);
            }
            other => panic!("应当是填充动作：{other:?}"),
        }
    }

    /// 两页的最小 PDF，给"真的跑一遍后台任务"的测试用。
    const MINIMAL_PDF: &[u8] = b"%PDF-1.4\n1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n2 0 obj\n<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 >>\nendobj\n3 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 300] >>\nendobj\n4 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 300] >>\nendobj\ntrailer\n<< /Size 5 /Root 1 0 R >>\n%%EOF";

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("pdfbatch-job-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// 等后台任务结束（带超时，避免线程出问题时测试挂死）。
    fn wait(run: &JobRun) -> JobProgress {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        loop {
            let snapshot = run.snapshot();
            if snapshot.finished {
                return snapshot;
            }
            assert!(std::time::Instant::now() < deadline, "后台任务超时");
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }

    #[test]
    fn a_fill_job_runs_end_to_end() {
        let dir = temp_dir("fill");
        let pdf = dir.join("sample.pdf");
        std::fs::write(&pdf, MINIMAL_PDF).unwrap();

        let fill = FillDraft {
            whole_page: true,
            pages: "all".to_string(),
            blend: Blend::Multiply,
            ..FillDraft::default()
        };
        let request = build_request(
            Mode::Fill,
            &fill,
            &BookmarkDraft::default(),
            &CommonOptions::default(),
            vec![input(pdf.to_str().unwrap())],
        )
        .unwrap();

        let run = spawn_job(request, egui::Context::default()).expect("任务应当能启动");
        let snapshot = wait(&run);
        assert_eq!(snapshot.done, 1);
        match &snapshot.statuses[0] {
            FileStatus::Done(note) => assert!(note.contains("第 2 页"), "{note}"),
            other => panic!("应当完成，实际 {other:?}"),
        }
        drop(run);

        let output = dir.join("sample_filled.pdf");
        let bytes = std::fs::read(&output).expect("应当写出结果文件");
        assert!(bytes.starts_with(b"%PDF"), "结果应当是 PDF");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_bookmark_job_runs_end_to_end() {
        let dir = temp_dir("bookmarks");
        let pdf = dir.join("sample.pdf");
        std::fs::write(&pdf, MINIMAL_PDF).unwrap();

        let bookmarks = BookmarkDraft {
            text: "第一章 绪论 | 1\n\t1.1 研究背景 | 2\n".to_string(),
            replace: true,
            ..BookmarkDraft::default()
        };
        let request = build_request(
            Mode::Bookmarks,
            &FillDraft::default(),
            &bookmarks,
            &CommonOptions::default(),
            vec![input(pdf.to_str().unwrap())],
        )
        .unwrap();

        let run = spawn_job(request, egui::Context::default()).expect("任务应当能启动");
        let snapshot = wait(&run);
        match &snapshot.statuses[0] {
            FileStatus::Done(note) => assert!(note.contains("2 条书签"), "{note}"),
            other => panic!("应当完成，实际 {other:?}"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn erase_request_carries_the_match_conditions() {
        let fill = FillDraft {
            action: FillAction::Erase,
            match_rect: MatchRect::FullPage,
            match_color_on: true,
            match_color: egui::Color32::from_rgb(255, 224, 140),
            pages: "all except first,last".to_string(),
            ..FillDraft::default()
        };
        let request = build_request(
            Mode::Fill,
            &fill,
            &BookmarkDraft::default(),
            &CommonOptions::default(),
            vec![input("D:/a.pdf")],
        )
        .unwrap();
        match request.action {
            Action::Fill { op: FillOp::Remove(m), pages } => {
                assert_eq!(m.rect, Some(RectMatch::FullPage));
                assert!(m.color.is_some());
                assert_eq!(pages, "all except first,last");
            }
            other => panic!("应当是删除：{other:?}"),
        }
    }

    #[test]
    fn modify_requires_at_least_one_change() {
        let empty = FillDraft {
            action: FillAction::Modify,
            ..FillDraft::default()
        };
        let error = build_request(
            Mode::Fill,
            &empty,
            &BookmarkDraft::default(),
            &CommonOptions::default(),
            vec![input("D:/a.pdf")],
        )
        .unwrap_err();
        assert!(error.contains("勾选"), "{error}");

        let filled = FillDraft {
            action: FillAction::Modify,
            change_color_on: true,
            change_color: egui::Color32::from_rgb(0, 255, 0),
            ..FillDraft::default()
        };
        let request = build_request(
            Mode::Fill,
            &filled,
            &BookmarkDraft::default(),
            &CommonOptions::default(),
            vec![input("D:/a.pdf")],
        )
        .unwrap();
        match request.action {
            Action::Fill { op: FillOp::Modify { change, .. }, .. } => {
                assert_eq!(change.color, Some([0.0, 1.0, 0.0]));
                assert_eq!(change.opacity, None, "没勾的维度不该动");
                assert_eq!(change.full_page, None);
            }
            other => panic!("应当是修改：{other:?}"),
        }
    }
}