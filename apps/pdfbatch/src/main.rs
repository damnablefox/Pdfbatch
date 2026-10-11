//! pdfbatch —— 图形界面入口。
//!
//! 命令行版本是另一个二进制（`apps/pdfbatch-cli`），两者共享 `pdfbatch-core`。

// Windows 上 Rust 程序默认是 console 子系统，双击时会附带一个黑色控制台窗口。
// release 构建改成 GUI 子系统就不再出现控制台；debug 构建仍保留控制台，方便看 panic 输出。
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

fn main() -> eframe::Result {
    pdfbatch_ui::run()
}