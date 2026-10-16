//! 批量执行：把动作应用到一批文件，支持取消与失败即停。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use crate::action::Action;
use crate::batch::{self, Job, OutputMode};
use crate::error::{BatchError, Result};
use crate::pdf;
use crate::record::RunNote;

/// 一个文件的处理结果。
#[derive(Debug)]
pub struct Outcome {
    pub input: PathBuf,
    pub output: PathBuf,
    pub status: OutcomeStatus,
}

#[derive(Debug)]
pub enum OutcomeStatus {
    /// 处理完成。
    Done(RunNote),
    /// 输入本身就带后缀，按约定跳过（让命令可以重复运行）。
    Skipped(String),
    /// 处理失败。
    Failed(BatchError),
}

impl Outcome {
    pub fn is_failure(&self) -> bool {
        matches!(self.status, OutcomeStatus::Failed(_))
    }
}

/// 逐个文件执行动作；每完成一个就回调一次（命令行立即打印，图形界面更新列表）。
///
/// `cancel` 一旦为真，剩下的文件不再处理（已经处理完的结果保留）。
pub fn run_jobs<F>(
    jobs: &[Job],
    output: &OutputMode,
    password: Option<&str>,
    full_save: bool,
    stop_on_error: bool,
    action: &Action,
    cancel: &AtomicBool,
    mut on_outcome: F,
) -> Vec<Outcome>
where
    F: FnMut(&Outcome),
{
    let mut outcomes = Vec::with_capacity(jobs.len());
    for job in jobs {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        let status = if !action.applies_to(&job.input) {
            // 分段书签：这个文件没有对应的段，跳过。
            OutcomeStatus::Skipped("没有对应的书签段".to_string())
        } else {
            match output {
                // 同目录另存 / 指定目录时，输入已经带上本次命名就跳过，让命令可以重复运行而不叠加。
                OutputMode::SameFolder { naming } | OutputMode::Dir { naming, .. }
                    if batch::already_named(&job.input, naming) =>
                {
                    OutcomeStatus::Skipped(format!("文件名已含{}", naming.describe()))
                }
                _ => process_one(job, password, full_save, action),
            }
        };
        let outcome = Outcome {
            input: job.input.clone(),
            output: job.output.clone(),
            status,
        };
        on_outcome(&outcome);
        let failed = outcome.is_failure();
        outcomes.push(outcome);
        // 「失败即停」：把取消标志立起来，下一轮循环就会退出（已处理的文件保留）。
        if stop_on_error && failed {
            cancel.store(true, Ordering::Relaxed);
        }
    }
    outcomes
}

/// 打开一个文件、按需修改、原子写回。
fn process_one(job: &Job, password: Option<&str>, full_save: bool, action: &Action) -> OutcomeStatus {
    let work = || -> Result<RunNote> {
        let mut doc = pdf::open(&job.input, password).map_err(|e| e.in_file(&job.input))?;
        let note = action.run(&mut doc, &job.input).map_err(|e| e.in_file(&job.input))?;
        let bytes = pdf::save(&mut doc, full_save).map_err(|e| e.in_file(&job.input))?;
        batch::write_atomic(&job.output, &bytes).map_err(|e| e.in_file(&job.output))?;
        Ok(note)
    };
    match work() {
        Ok(note) => OutcomeStatus::Done(note),
        Err(error) => OutcomeStatus::Failed(error),
    }
}

/// 加密 PDF 的密码来自一个文件：文件内容即密码，去掉行尾的换行符。
pub fn read_password_file(path: &Path) -> Result<String> {
    let text = std::fs::read_to_string(path)?;
    Ok(text.trim_end_matches(['\r', '\n']).to_string())
}
