//! 逐页处理记录，以及一次动作的结果说明。

/// 一页的处理结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RecordResult {
    #[default]
    Done,
    Skipped,
    Failed,
}

impl RecordResult {
    pub fn label(self) -> &'static str {
        match self {
            RecordResult::Done => "成功",
            RecordResult::Skipped => "跳过",
            RecordResult::Failed => "失败",
        }
    }
}

/// 一页的处理记录（目前由「页面尺寸」动作填写，供处理日志使用）。
#[derive(Debug, Clone, Default)]
pub struct PageRecord {
    /// 页码（从 1 开始）。
    pub page: usize,
    pub result: RecordResult,
    /// 原因或摘要。
    pub reason: String,
    /// 处理后的页面尺寸（点），没有就是 `None`。
    pub size: Option<(f64, f64)>,
    /// 扫描件适配：整个文件统一的页面宽度（点）。
    pub scan_width: Option<f64>,
    /// 处理之后该页所有图像的**最小实际 PPI**（按最终页面与绘制区域实测）。
    pub min_ppi: Option<f64>,
}

/// 一次动作的结果说明：一句摘要，加上不致命的警告与逐页记录。
#[derive(Debug, Clone, Default)]
pub struct RunNote {
    pub summary: String,
    pub warnings: Vec<String>,
    /// 逐页记录；不需要细分的动作留空。
    pub records: Vec<PageRecord>,
}
