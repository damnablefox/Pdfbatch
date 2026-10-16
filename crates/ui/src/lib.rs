//! pdfbatch-ui —— 图形界面层。
//!
//! 观感对齐 PdfCraft（同款配色、字体与控件风格），界面逻辑是本项目自己的：
//! 一个窗口里做两件事——批量盖填充层、批量导入书签。
//!
//! 两个功能都作用在左侧文件列表里的**所有**文件上（"应用到多份文件"），
//! 填充层还额外提供混合模式与置于顶层/底层的选择。
//!
//! 界面按标签页 / 面板拆成若干模块：`app` 是外壳（窗口、顶栏、执行条与参数分发），
//! 各 `*_panel` 是各自的标签页或面板，`helpers` 放跨面板共用的小函数。

mod app;
mod bookmark_panel;
mod files;
mod files_panel;
mod fill_panel;
mod helpers;
mod job;
mod output_panel;
mod palette;
mod pagesize_panel;
mod pickers;
mod theme;
mod widgets;

pub use app::{run, App};
pub use theme::{medium, regular, semibold};
