//! 原生文件选择器：绝不阻塞界面帧。
//!
//! 阻塞式的 `rfd::FileDialog` 会在消息泵里跑一个嵌套循环；在部分平台上会造成重入问题。
//! 这里用 `rfd::AsyncFileDialog`：窗口线程只负责把对话框创建出来（macOS 上"创建即弹出"），
//! 真正的等待放在一个工作线程里（`pollster::block_on`），结果留到下一帧由界面处理。

use std::future::Future;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

/// 一次选择是为了什么。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PickPurpose {
    /// 添加 PDF 文件（可多选）。
    AddFiles,
    /// 添加文件夹。
    AddFolder,
    /// 选择输出目录。
    OutputDir,
    /// 选择书签文本文件。
    BookmarkSpec,
}

/// 一次已完成的选择。
#[derive(Debug)]
pub struct PickResult {
    pub purpose: PickPurpose,
    /// 选中的路径；取消时为空。
    pub paths: Vec<PathBuf>,
}

/// 挂起的选择请求与已完成的结果。
#[derive(Clone, Debug, Default)]
pub struct Pickers {
    /// 有选择器正在显示（或结果尚未被处理），此时不接受第二个请求。
    showing: Arc<AtomicBool>,
    done: Arc<Mutex<Vec<PickResult>>>,
}

impl Pickers {
    /// 是否有选择器正在显示。
    pub fn is_showing(&self) -> bool {
        self.showing.load(Ordering::SeqCst)
    }

    /// 在后台线程等待 `pick` 的结果。`pick` 必须在主线程创建（macOS 上创建即弹出面板）。
    ///
    /// 调用前应先检查 [`Self::is_showing`]，并在为真时不要创建对话框。
    pub fn spawn<F>(&self, purpose: PickPurpose, ctx: Option<egui::Context>, pick: F)
    where
        F: Future<Output = Vec<PathBuf>> + Send + 'static,
    {
        self.showing.store(true, Ordering::SeqCst);
        let mut showing = Showing(Some(self.showing.clone()));
        let done = self.done.clone();
        let _ = std::thread::Builder::new()
            .name("pdfbatch-picker".into())
            .spawn(move || {
                let paths = pollster::block_on(pick);
                done.lock().unwrap_or_else(PoisonError::into_inner).push(PickResult { purpose, paths });
                // 结果已入队：标志交给处理它的那一帧来清。
                showing.hand_over();
                if let Some(ctx) = ctx {
                    ctx.request_repaint();
                }
            });
        // 线程没起来时闭包（连同 `Showing`）已被丢弃，Drop 会把标志复位；无需额外处理。
    }

    /// 取走已完成的选择，并释放"正在显示"的标志。
    pub fn take(&self) -> Vec<PickResult> {
        let results = std::mem::take(&mut *self.done.lock().unwrap_or_else(PoisonError::into_inner));
        if !results.is_empty() {
            self.showing.store(false, Ordering::SeqCst);
        }
        results
    }
}

/// 工作线程在未入队结果就退出（panic）时，把"正在显示"标志清掉。
struct Showing(Option<Arc<AtomicBool>>);

impl Showing {
    /// 结果已入队：交给后续帧处理，本次不再复位标志。
    fn hand_over(&mut self) {
        self.0 = None;
    }
}

impl Drop for Showing {
    fn drop(&mut self) {
        if let Some(flag) = &self.0 {
            flag.store(false, Ordering::SeqCst);
        }
    }
}