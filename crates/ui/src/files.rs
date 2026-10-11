//! 待处理文件列表：加入、展开目录、按路径去重，以及每个文件的处理状态。

use std::path::PathBuf;

use pdfbatch_core::batch::{self, Input};

/// 一个文件在一次任务里的状态。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FileStatus {
    /// 尚未开始。
    Pending,
    /// 正在处理（进度条上会显示它）。
    Running,
    /// 完成，附一句摘要。
    Done(String),
    /// 按约定跳过（例如文件名已含后缀）。
    Skipped(String),
    /// 处理失败。
    Failed(String),
}

impl FileStatus {
    /// 给表格用的短标签。
    pub fn label(&self) -> String {
        match self {
            FileStatus::Pending => "待处理".to_string(),
            FileStatus::Running => "处理中…".to_string(),
            FileStatus::Done(note) => note.clone(),
            FileStatus::Skipped(why) => format!("跳过：{why}"),
            FileStatus::Failed(why) => format!("失败：{why}"),
        }
    }
}

/// 列表里的一行：一个具体的 PDF，加上它相对于所属根输入的路径。
#[derive(Clone, Debug)]
pub struct JobFile {
    pub id: u64,
    pub input: Input,
    pub status: FileStatus,
}

/// 文件列表。
#[derive(Debug, Default)]
pub struct FileList {
    pub items: Vec<JobFile>,
    pub selected: Option<u64>,
    next_id: u64,
}

impl FileList {
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// 加入文件或目录：目录按需递归展开成其中的 `.pdf`，与已有条目按路径去重。
    ///
    /// 逐个路径收集，单个路径出错（不存在、目录里没有 PDF）不会影响其余路径；返回新增数量
    /// 与第一条错误消息。
    pub fn add_paths(&mut self, paths: &[PathBuf], recursive: bool) -> (usize, Option<String>) {
        let mut added = 0;
        let mut first_error: Option<String> = None;
        for path in paths {
            let inputs = match batch::collect(std::slice::from_ref(path), recursive) {
                Ok(inputs) => inputs,
                Err(error) => {
                    first_error.get_or_insert(error.to_string());
                    continue;
                }
            };
            for input in inputs {
                if self.items.iter().any(|item| item.input.path == input.path) {
                    continue;
                }
                self.next_id += 1;
                self.items.push(JobFile {
                    id: self.next_id,
                    input,
                    status: FileStatus::Pending,
                });
                added += 1;
            }
        }
        if let Some(last) = self.items.last() {
            self.selected = Some(last.id);
        }
        (added, first_error)
    }

    /// 移除当前选中的一行。
    pub fn remove_selected(&mut self) {
        let Some(id) = self.selected else { return };
        let index = self.items.iter().position(|item| item.id == id);
        if let Some(index) = index {
            self.items.remove(index);
            self.selected = self.items.get(index.saturating_sub(1).min(self.items.len().saturating_sub(1))).map(|i| i.id);
            if self.items.is_empty() {
                self.selected = None;
            }
        }
    }

    /// 清空列表。
    pub fn clear(&mut self) {
        self.items.clear();
        self.selected = None;
    }

    /// 把所有行恢复到"待处理"，为一次新任务做准备。
    pub fn reset_statuses(&mut self) {
        for item in &mut self.items {
            item.status = FileStatus::Pending;
        }
    }

    /// 交给批量管线的输入（保持列表顺序）。
    pub fn to_inputs(&self) -> Vec<Input> {
        self.items.iter().map(|item| item.input.clone()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_paths_deduplicates_by_path() {
        let mut list = FileList::default();
        let dir = std::env::temp_dir().join(format!("pdfbatch-list-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let a = dir.join("a.pdf");
        std::fs::write(&a, b"%PDF-1.4\n").unwrap();

        let (added, error) = list.add_paths(&[a.clone()], true);
        assert_eq!(added, 1);
        assert!(error.is_none());
        // 再放一次同一个文件：不重复。
        let (added, _) = list.add_paths(&[a.clone()], true);
        assert_eq!(added, 0);
        assert_eq!(list.len(), 1);
        assert_eq!(list.to_inputs().len(), 1);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_bad_path_does_not_block_the_good_one() {
        let mut list = FileList::default();
        let dir = std::env::temp_dir().join(format!("pdfbatch-list2-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let good = dir.join("good.pdf");
        std::fs::write(&good, b"%PDF-1.4\n").unwrap();
        let missing = dir.join("missing.pdf");

        let (added, error) = list.add_paths(&[missing, good], true);
        assert_eq!(added, 1, "坏路径不该影响好路径");
        assert!(error.is_some(), "坏路径应当给出提示");
        let _ = std::fs::remove_dir_all(&dir);
    }
}