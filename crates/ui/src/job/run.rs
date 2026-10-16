//! 后台执行：在独立线程里跑批量任务与「只查找」任务，并把进度快照暴露给界面。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use pdfbatch_core::action::{OutcomeStatus, run_jobs};

use crate::files::FileStatus;

use super::request::Request;

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
    let stop_on_error = request.stop_on_error;
    let log = request.log;

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
            let outcomes = run_jobs(
                jobs_ref,
                &output,
                password.as_deref(),
                full_save,
                stop_on_error,
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
            // 处理日志：任务结束后一次性落盘（位置是可执行文件同级目录）。
            if let Some(format) = log {
                let records = pdfbatch_core::report::collect(&outcomes);
                let path = pdfbatch_core::report::default_path(format);
                let _ = pdfbatch_core::report::write(&records, format, &path);
            }
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
    /// 逐文件的命中数，与请求里的输入一一对应。
    pub per_file: Vec<usize>,
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
    let mut per_file = Vec::with_capacity(request.inputs.len());
    for input in &request.inputs {
        let mut doc = pdfbatch_core::pdf::open(&input.path, request.password.as_deref())
            .map_err(|error| format!("{}：{error}", input.path.display()))?;
        let hits = request
            .action
            .probe(&mut doc)
            .map_err(|error| format!("{}：{error}", input.path.display()))?;
        total += hits;
        per_file.push(hits);
    }
    Ok(ProbeReport {
        fingerprint: fingerprint.to_string(),
        total,
        per_file,
    })
}

fn file_name(path: &std::path::Path) -> String {
    path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| path.display().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::job::{BookmarkDraft, CommonOptions, FillDraft, Mode, PageSizeDraft};
    use pdfbatch_core::batch::Input;
    use pdfbatch_core::Blend;
    use std::path::PathBuf;

    fn input(path: &str) -> Input {
        Input {
            path: PathBuf::from(path),
            relative: PathBuf::from(PathBuf::from(path).file_name().unwrap()),
        }
    }

    /// 测试用的 build_request：页面尺寸草稿取默认值，省得每个用例都写一遍。
    fn build_request(
        mode: Mode,
        fill: &FillDraft,
        bookmarks: &BookmarkDraft,
        common: &CommonOptions,
        inputs: Vec<Input>,
    ) -> Result<Request, String> {
        crate::job::build_request(mode, fill, &PageSizeDraft::default(), bookmarks, common, inputs)
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
}
