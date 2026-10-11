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
    /// 状态列的短标签。
    pub fn label(&self) -> &'static str {
        match self {
            FileStatus::Pending => "待处理",
            FileStatus::Running => "处理中…",
            FileStatus::Done(_) => "已完成",
            FileStatus::Skipped(_) => "跳过",
            FileStatus::Failed(_) => "失败",
        }
    }

    /// 提示列的详情：完成摘要、跳过原因或错误信息（还没有则为空）。
    pub fn detail(&self) -> &str {
        match self {
            FileStatus::Done(note) => note,
            FileStatus::Skipped(why) | FileStatus::Failed(why) => why,
            FileStatus::Pending | FileStatus::Running => "",
        }
    }
}

/// 列表里的一行：一个具体的 PDF，加上它相对于所属根输入的路径。
#[derive(Clone, Debug)]
pub struct JobFile {
    pub id: u64,
    pub input: Input,
    pub status: FileStatus,
    /// 信息提示列的补充内容（例如「查找匹配」得到的命中数量）。
    /// 执行阶段产生的详情由 `status.detail()` 提供，优先于它显示。
    pub hint: String,
    /// 提示的来源：`Some(true)` 查找命中、`Some(false)` 查找无匹配、`None` 没有查找结果。
    /// 用来给提示列配不同的文字颜色。
    pub hint_hit: Option<bool>,
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
                    hint: String::new(),
                    hint_hit: None,
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
            item.hint.clear();
            item.hint_hit = None;
        }
    }

    /// 交给批量管线的输入（保持列表顺序）。
    pub fn to_inputs(&self) -> Vec<Input> {
        self.items.iter().map(|item| item.input.clone()).collect()
    }

    /// 把某一行上移一位（已在首位则不动）。
    pub fn move_up(&mut self, id: u64) {
        if let Some(index) = self.items.iter().position(|item| item.id == id) {
            if index > 0 {
                self.items.swap(index - 1, index);
            }
        }
    }

    /// 把某一行下移一位（已在末位则不动）。
    pub fn move_down(&mut self, id: u64) {
        if let Some(index) = self.items.iter().position(|item| item.id == id) {
            if index + 1 < self.items.len() {
                self.items.swap(index, index + 1);
            }
        }
    }

    /// 按文件名排序（不区分大小写）；`ascending` 为真时 A→Z。
    ///
    /// 同名时保持原有相对顺序（稳定排序），且不改动 `selected`（按 id 追踪）。
    pub fn sort_by_name(&mut self, ascending: bool) {
        self.items.sort_by(|a, b| {
            let ord = file_name_key(a).cmp(&file_name_key(b));
            if ascending { ord } else { ord.reverse() }
        });
    }
}

/// 排序用的文件名键（小写化，便于不区分大小写地比较）。
fn file_name_key(item: &JobFile) -> String {
    item.input
        .path
        .file_name()
        .map(|name| name.to_string_lossy().to_lowercase())
        .unwrap_or_default()
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

    /// 直接塞一行（移动与排序只看路径，不碰文件系统）。
    fn push(list: &mut FileList, path: &str) {
        list.next_id += 1;
        list.items.push(JobFile {
            id: list.next_id,
            input: Input {
                path: PathBuf::from(path),
                relative: PathBuf::from(path),
            },
            status: FileStatus::Pending,
            hint: String::new(),
            hint_hit: None,
        });
    }

    fn names(list: &FileList) -> Vec<String> {
        list.items
            .iter()
            .map(|item| item.input.path.file_name().unwrap().to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn move_up_and_down_swap_neighbours_only() {
        let mut list = FileList::default();
        push(&mut list, "D:/a.pdf");
        push(&mut list, "D:/b.pdf");
        push(&mut list, "D:/c.pdf");

        // 首位上移、末位下移都不动。
        list.move_up(list.items[0].id);
        assert_eq!(names(&list), ["a.pdf", "b.pdf", "c.pdf"]);
        list.move_down(list.items[2].id);
        assert_eq!(names(&list), ["a.pdf", "b.pdf", "c.pdf"]);

        // b 上移一位，再下移回来。
        let b = list.items[1].id;
        list.move_up(b);
        assert_eq!(names(&list), ["b.pdf", "a.pdf", "c.pdf"]);
        list.move_down(b);
        assert_eq!(names(&list), ["a.pdf", "b.pdf", "c.pdf"]);
    }

    #[test]
    fn sort_by_name_orders_case_insensitively() {
        let mut list = FileList::default();
        push(&mut list, "D:/Beta.pdf");
        push(&mut list, "D:/alpha.pdf");
        push(&mut list, "D:/Gamma.pdf");

        list.sort_by_name(true);
        assert_eq!(names(&list), ["alpha.pdf", "Beta.pdf", "Gamma.pdf"]);
        list.sort_by_name(false);
        assert_eq!(names(&list), ["Gamma.pdf", "Beta.pdf", "alpha.pdf"]);
    }
}