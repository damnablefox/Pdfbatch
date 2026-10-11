//! 批量处理：收集输入、规划输出路径、原子写入。

use std::path::{Path, PathBuf};

use crate::error::{BatchError, Result};

/// 一个输入文件，以及它相对于所属根输入的路径（用于在输出目录里保留目录结构）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Input {
    pub path: PathBuf,
    pub relative: PathBuf,
}

/// 输出文件命名：前缀 + 原名 + 后缀，直接拼接（是否带分隔符由用户自己决定）。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Naming {
    pub prefix: String,
    pub suffix: String,
}

impl Naming {
    /// 前后缀都为空。
    pub fn is_empty(&self) -> bool {
        self.prefix.is_empty() && self.suffix.is_empty()
    }

    /// 给文件名的 stem 套上命名：`前缀 + stem + 后缀`。
    pub fn apply(&self, stem: &str) -> String {
        format!("{}{}{}", self.prefix, stem, self.suffix)
    }

    /// 这个 stem 是否已经带上了本次命名（用于重复运行时跳过，避免叠加）。
    pub fn matches(&self, stem: &str) -> bool {
        (!self.prefix.is_empty() && stem.starts_with(&self.prefix))
            || (!self.suffix.is_empty() && stem.ends_with(&self.suffix))
    }

    /// 给用户看的说明（跳过消息用）。
    pub fn describe(&self) -> String {
        match (self.prefix.is_empty(), self.suffix.is_empty()) {
            (false, false) => format!("前缀「{}」或后缀「{}」", self.prefix, self.suffix),
            (false, true) => format!("前缀「{}」", self.prefix),
            (true, false) => format!("后缀「{}」", self.suffix),
            (true, true) => "命名".to_string(),
        }
    }
}

/// 输出位置。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OutputMode {
    /// 覆盖原文件（先写临时文件，再原子替换）。
    InPlace,
    /// 写到指定目录；目录输入会保留其相对子路径，文件名按命名规则改写。
    Dir { dir: PathBuf, naming: Naming },
    /// 写到一个明确的文件（只允许单个输入）。
    File(PathBuf),
    /// 与原文件同目录，文件名按命名规则改写（默认加后缀，不会覆盖原文件）。
    SameFolder { naming: Naming },
}

/// 一个待处理的任务。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Job {
    pub input: PathBuf,
    pub output: PathBuf,
}

/// 展开输入：文件直接采用，目录按需递归收集 `.pdf`。
pub fn collect(inputs: &[PathBuf], recursive: bool) -> Result<Vec<Input>> {
    let mut found: Vec<Input> = Vec::new();
    for input in inputs {
        let meta = std::fs::metadata(input).map_err(|e| BatchError::Arg(format!("{}：{e}", input.display())))?;
        if meta.is_dir() {
            collect_dir(input, input, recursive, &mut found)?;
        } else {
            let name = input
                .file_name()
                .map(PathBuf::from)
                .ok_or_else(|| BatchError::Arg(format!("{} 没有文件名", input.display())))?;
            found.push(Input {
                path: input.clone(),
                relative: name,
            });
        }
    }
    // 去重（同一文件可能同时被文件参数和目录参数覆盖）后按路径排序，让结果可预测。
    found.sort_by(|a, b| a.path.cmp(&b.path));
    found.dedup_by(|a, b| a.path == b.path);
    if found.is_empty() {
        return Err(BatchError::Arg("没有找到任何 PDF 文件".to_string()));
    }
    Ok(found)
}

fn collect_dir(root: &Path, dir: &Path, recursive: bool, out: &mut Vec<Input>) -> Result<()> {
    let mut entries: Vec<PathBuf> = std::fs::read_dir(dir)?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            if recursive {
                collect_dir(root, &path, recursive, out)?;
            }
        } else if has_pdf_extension(&path) {
            let relative = path.strip_prefix(root).map(PathBuf::from).unwrap_or_else(|_| {
                path.file_name().map(PathBuf::from).unwrap_or_default()
            });
            out.push(Input { path, relative });
        }
    }
    Ok(())
}

fn has_pdf_extension(path: &Path) -> bool {
    path.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("pdf"))
}

/// 给每个输入规划输出路径，并检查会不会互相覆盖。
pub fn plan(inputs: &[Input], mode: &OutputMode) -> Result<Vec<Job>> {
    match mode {
        OutputMode::File(output) => {
            if inputs.len() != 1 {
                return Err(BatchError::Arg(format!(
                    "--out 只能用于单个输入文件，当前有 {} 个",
                    inputs.len()
                )));
            }
            Ok(vec![Job {
                input: inputs[0].path.clone(),
                output: output.clone(),
            }])
        }
        OutputMode::InPlace => Ok(inputs
            .iter()
            .map(|input| Job {
                input: input.path.clone(),
                output: input.path.clone(),
            })
            .collect()),
        OutputMode::SameFolder { naming } => {
            let jobs: Vec<Job> = inputs
                .iter()
                .map(|input| {
                    let stem = input.path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                    let name = format!("{}.pdf", naming.apply(&stem));
                    Job {
                        input: input.path.clone(),
                        output: input.path.with_file_name(name),
                    }
                })
                .collect();
            ensure_distinct(&jobs)?;
            Ok(jobs)
        }
        OutputMode::Dir { dir, naming } => {
            std::fs::create_dir_all(dir)?;
            let jobs: Vec<Job> = inputs
                .iter()
                .map(|input| {
                    // 只改文件名部分，目录输入的子路径原样保留。
                    let stem = input.relative.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                    let name = format!("{}.pdf", naming.apply(&stem));
                    let parent = input.relative.parent().unwrap_or_else(|| Path::new(""));
                    Job {
                        input: input.path.clone(),
                        output: dir.join(parent).join(name),
                    }
                })
                .collect();
            ensure_distinct(&jobs)?;
            Ok(jobs)
        }
    }
}

/// 两个输入映射到同一个输出时必须停下来，而不是互相覆盖。
fn ensure_distinct(jobs: &[Job]) -> Result<()> {
    let mut seen: Vec<&Path> = Vec::new();
    for job in jobs {
        if seen.contains(&job.output.as_path()) {
            return Err(BatchError::Arg(format!(
                "多个输入会写到同一个文件：{}；请改用 --out-dir 或 --in-place",
                job.output.display()
            )));
        }
        seen.push(&job.output);
    }
    Ok(())
}

/// 原子写：先把内容写到同目录下的临时文件，再改名替换目标。
///
/// 目标目录不存在时会先建出来——目录输入保留相对子路径时会落到更深的一层。
pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() && !parent.exists() {
            std::fs::create_dir_all(parent)?;
        }
    }
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .ok_or_else(|| BatchError::Arg(format!("{} 不是有效的文件路径", path.display())))?;
    let temp = path.with_file_name(format!(".{name}.tmp-pdfbatch"));
    std::fs::write(&temp, bytes)?;
    std::fs::rename(&temp, path)?;
    Ok(())
}

/// 文件名已经带上本次命名时，认为它已是本工具的产物（让命令可以重复运行而不叠加）。
pub fn already_named(path: &Path, naming: &Naming) -> bool {
    path.file_stem()
        .map(|stem| naming.matches(&stem.to_string_lossy()))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(path: &str, relative: &str) -> Input {
        Input {
            path: PathBuf::from(path),
            relative: PathBuf::from(relative),
        }
    }

    fn naming(prefix: &str, suffix: &str) -> Naming {
        Naming {
            prefix: prefix.to_string(),
            suffix: suffix.to_string(),
        }
    }

    #[test]
    fn same_folder_adds_the_suffix() {
        let jobs = plan(&[input("D:/a/x.pdf", "x.pdf")], &OutputMode::SameFolder { naming: naming("", "_filled") }).unwrap();
        assert_eq!(jobs[0].output, PathBuf::from("D:/a/x_filled.pdf"));
    }

    #[test]
    fn same_folder_adds_prefix_and_suffix() {
        let jobs = plan(
            &[input("D:/a/x.pdf", "x.pdf")],
            &OutputMode::SameFolder { naming: naming("new_", "_filled") },
        )
        .unwrap();
        assert_eq!(jobs[0].output, PathBuf::from("D:/a/new_x_filled.pdf"));
    }

    #[test]
    fn prefix_only_keeps_the_name_without_a_suffix() {
        // 只填前缀、清空后缀时不该被补上默认后缀。
        let jobs = plan(&[input("D:/a/x.pdf", "x.pdf")], &OutputMode::SameFolder { naming: naming("new_", "") }).unwrap();
        assert_eq!(jobs[0].output, PathBuf::from("D:/a/new_x.pdf"));
    }

    #[test]
    fn dir_keeps_relative_subpaths() {
        let jobs = plan(
            &[input("D:/in/sub/x.pdf", "sub/x.pdf"), input("D:/in/y.pdf", "y.pdf")],
            &OutputMode::Dir {
                dir: PathBuf::from("D:/out"),
                naming: Naming::default(),
            },
        )
        .unwrap();
        // `D:/out/sub` 未必存在（plan 只建了 `D:/out`），这里只比较路径形状。
        assert_eq!(jobs[0].output, PathBuf::from("D:/out/sub/x.pdf"));
        assert_eq!(jobs[1].output, PathBuf::from("D:/out/y.pdf"));
    }

    #[test]
    fn dir_applies_naming_and_keeps_subpaths() {
        let jobs = plan(
            &[input("D:/in/sub/x.pdf", "sub/x.pdf")],
            &OutputMode::Dir {
                dir: PathBuf::from("D:/out"),
                naming: naming("new_", "_done"),
            },
        )
        .unwrap();
        assert_eq!(jobs[0].output, PathBuf::from("D:/out/sub/new_x_done.pdf"));
    }

    #[test]
    fn out_file_requires_a_single_input() {
        let many = [input("D:/a.pdf", "a.pdf"), input("D:/b.pdf", "b.pdf")];
        assert!(plan(&many, &OutputMode::File(PathBuf::from("D:/o.pdf"))).is_err());
        let one = [input("D:/a.pdf", "a.pdf")];
        assert!(plan(&one, &OutputMode::File(PathBuf::from("D:/o.pdf"))).is_ok());
    }

    #[test]
    fn collisions_are_reported() {
        // 两个不同目录下的同名文件落到同一输出目录时，必须报错。
        let jobs = plan(
            &[input("D:/x/a.pdf", "a.pdf"), input("D:/y/a.pdf", "a.pdf")],
            &OutputMode::Dir {
                dir: PathBuf::from("D:/out"),
                naming: Naming::default(),
            },
        );
        assert!(jobs.is_err(), "同名输出应当被发现");
    }

    #[test]
    fn naming_detection() {
        assert!(already_named(Path::new("D:/a/x_filled.pdf"), &naming("", "_filled")));
        assert!(!already_named(Path::new("D:/a/x.pdf"), &naming("", "_filled")));
        assert!(already_named(Path::new("D:/a/new_x.pdf"), &naming("new_", "")));
        assert!(!already_named(Path::new("D:/a/x.pdf"), &naming("new_", "")));
    }

    #[test]
    fn write_atomic_creates_missing_parent_folders() {
        let dir = std::env::temp_dir().join(format!("pdfbatch-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let target = dir.join("deep").join("nested").join("out.bin");
        write_atomic(&target, b"hello").unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"hello");
        // 覆盖写也要成功（先写临时文件再改名替换）。
        write_atomic(&target, b"again").unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"again");
        let _ = std::fs::remove_dir_all(&dir);
    }
}