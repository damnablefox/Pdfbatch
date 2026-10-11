//! pdfbatch-cli —— 命令行入口。
//!
//! 与图形界面（`apps/pdfbatch`）共享 `pdfbatch-core` 里的同一份实现，行为不会漂移。

mod cli;

use std::process::ExitCode;
use std::sync::atomic::AtomicBool;

use pdfbatch_core::action::{OutcomeStatus, read_password_file, run_jobs};
use pdfbatch_core::batch;
use pdfbatch_core::{Action, BatchError, FillOp};

use cli::{Common, Invocation};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let invocation = match cli::parse(&args) {
        Ok(invocation) => invocation,
        Err(message) => {
            eprintln!("参数错误：{message}");
            eprintln!();
            eprintln!("{}", cli::help());
            return ExitCode::from(2);
        }
    };
    match invocation {
        Invocation::Help => {
            println!("{}", cli::help());
            ExitCode::SUCCESS
        }
        Invocation::Bookmarks(args) => {
            let text = match std::fs::read_to_string(&args.spec) {
                Ok(text) => text,
                Err(error) => {
                    eprintln!("读不到书签文本 {}：{error}", args.spec.display());
                    return ExitCode::from(2);
                }
            };
            run(
                args.common,
                Kind::Bookmarks {
                    text,
                    zero_based: args.zero_based,
                    strict: args.strict,
                    replace: args.replace,
                },
            )
        }
        Invocation::Fill(args) => run(
            args.common,
            Kind::Fill {
                op: args.op,
                pages: args.pages,
            },
        ),
    }
}

/// 还没整理成具体动作的请求：书签要先拿到文件列表，才能按段头绑定到各文件。
enum Kind {
    Bookmarks {
        text: String,
        zero_based: bool,
        strict: bool,
        replace: bool,
    },
    Fill {
        op: FillOp,
        pages: String,
    },
}

/// 收集输入 → 构造动作 → 规划输出 → 执行 → 打印汇总。
fn run(common: Common, kind: Kind) -> ExitCode {
    let inputs = match batch::collect(&common.inputs, common.recursive) {
        Ok(inputs) => inputs,
        Err(error) => return fail(error),
    };
    let action = match kind {
        Kind::Bookmarks { text, zero_based, strict, replace } => {
            let spec = match pdfbatch_core::bookmark_spec(&text, &inputs) {
                Ok(spec) => spec,
                Err(message) => {
                    eprintln!("书签文本有误：{message}");
                    return ExitCode::from(2);
                }
            };
            Action::Bookmarks { spec, zero_based, strict, replace }
        }
        Kind::Fill { op, pages } => Action::Fill { op, pages },
    };
    let jobs = match batch::plan(&inputs, &common.output) {
        Ok(jobs) => jobs,
        Err(error) => return fail(error),
    };
    let password = match &common.password_file {
        Some(path) => match read_password_file(path) {
            Ok(password) => Some(password),
            Err(error) => return fail(error),
        },
        None => None,
    };

    // 命令行不做交互式取消：中断交给终端本身。
    let cancel = AtomicBool::new(false);
    let outcomes = run_jobs(
        &jobs,
        &common.output,
        password.as_deref(),
        common.full_save,
        &action,
        &cancel,
        |outcome| match &outcome.status {
            OutcomeStatus::Done(note) => {
                for warning in &note.warnings {
                    eprintln!("{}：{warning}", outcome.input.display());
                }
                println!(
                    "{} → {}：{}",
                    outcome.input.display(),
                    outcome.output.display(),
                    note.summary
                );
            }
            OutcomeStatus::Skipped(why) => println!("跳过 {}（{why}）", outcome.input.display()),
            // 失败留到最后的汇总里统一打印，免得和进度输出混在一起。
            OutcomeStatus::Failed(_) => {}
        },
    );

    let done = outcomes
        .iter()
        .filter(|o| matches!(o.status, OutcomeStatus::Done(_)))
        .count();
    let skipped = outcomes
        .iter()
        .filter(|o| matches!(o.status, OutcomeStatus::Skipped(_)))
        .count();
    let failures = outcomes.iter().filter(|o| o.is_failure()).count();

    println!();
    println!("共 {} 个文件：成功 {done}，跳过 {skipped}，失败 {failures}", jobs.len());
    for outcome in outcomes.iter().filter(|o| o.is_failure()) {
        if let OutcomeStatus::Failed(error) = &outcome.status {
            eprintln!("  {}：{error}", outcome.input.display());
        }
    }

    if failures == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}

/// 还没开始处理就失败（参数、输入路径等）。
fn fail(error: BatchError) -> ExitCode {
    eprintln!("{error}");
    ExitCode::from(2)
}