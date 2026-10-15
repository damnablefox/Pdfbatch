//! 处理日志（CSV / JSON）。
//!
//! 任务结束（或被取消）后**一次性落盘**，不逐行写，避免拖慢批量处理。
//! 默认位置是**可执行文件同级目录**，文件名形如 `pdfbatch-log-20261013-153000.csv`。

use std::path::{Path, PathBuf};

use crate::action::{Outcome, OutcomeStatus};

/// 日志文件格式。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LogFormat {
    Csv,
    Json,
}

impl LogFormat {
    pub fn parse(text: &str) -> std::result::Result<Self, String> {
        match text.trim().to_ascii_lowercase().as_str() {
            "csv" => Ok(LogFormat::Csv),
            "json" => Ok(LogFormat::Json),
            _ => Err(format!("未知日志格式 `{text}`；可用：csv、json")),
        }
    }

    pub fn canonical(self) -> &'static str {
        match self {
            LogFormat::Csv => "csv",
            LogFormat::Json => "json",
        }
    }

    pub fn extension(self) -> &'static str {
        self.canonical()
    }
}

/// 一条日志记录。
#[derive(Clone, Debug, PartialEq)]
pub struct LogRecord {
    /// 文件名。
    pub file: String,
    /// 完整路径。
    pub path: String,
    /// 页码（从 1 开始）；文件级记录为 `None`。
    pub page: Option<usize>,
    /// 结果：成功 / 跳过 / 失败。
    pub result: String,
    /// 原因或摘要。
    pub reason: String,
    /// 处理后的页面尺寸，如 `595.28×841.89`；没有就是空串。
    pub size: String,
    /// 扫描件适配的统一页宽（点），如 `187.08`；没有就是空串。
    pub scan_width: String,
    /// 该页所有图像的最小实际 PPI，如 `600.21`；没有就是空串。
    pub min_ppi: String,
}

/// 把执行结果整理成日志记录。
///
/// 带逐页记录的动作会按页展开；其余动作退化成一条文件级记录。
pub fn collect(outcomes: &[Outcome]) -> Vec<LogRecord> {
    let mut out = Vec::new();
    for outcome in outcomes {
        let file = file_name(&outcome.input);
        let path = outcome.input.display().to_string();
        match &outcome.status {
            OutcomeStatus::Done(note) if !note.records.is_empty() => {
                for record in &note.records {
                    out.push(LogRecord {
                        file: file.clone(),
                        path: path.clone(),
                        page: Some(record.page),
                        result: record.result.label().to_string(),
                        reason: record.reason.clone(),
                        size: record
                            .size
                            .map(|(w, h)| format!("{:.2}×{:.2}", w, h))
                            .unwrap_or_default(),
                        // 扫描件适配的统一页宽与该页实测最小 PPI 都只在扫描件时才有值。
                        scan_width: record
                            .scan_width
                            .map(|value| format!("{value:.2}"))
                            .unwrap_or_default(),
                        min_ppi: record
                            .min_ppi
                            .map(|value| format!("{value:.2}"))
                            .unwrap_or_default(),
                    });
                }
            }
            OutcomeStatus::Done(note) => out.push(LogRecord {
                file,
                path,
                page: None,
                result: "成功".to_string(),
                reason: note.summary.clone(),
                size: String::new(),
                scan_width: String::new(),
                min_ppi: String::new(),
            }),
            OutcomeStatus::Skipped(why) => out.push(LogRecord {
                file,
                path,
                page: None,
                result: "跳过".to_string(),
                reason: why.clone(),
                size: String::new(),
                scan_width: String::new(),
                min_ppi: String::new(),
            }),
            OutcomeStatus::Failed(error) => out.push(LogRecord {
                file,
                path,
                page: None,
                result: "失败".to_string(),
                reason: error.to_string(),
                size: String::new(),
                scan_width: String::new(),
                min_ppi: String::new(),
            }),
        }
    }
    out
}

/// 把记录渲染成文本。
pub fn render(records: &[LogRecord], format: LogFormat) -> String {
    match format {
        LogFormat::Csv => render_csv(records),
        LogFormat::Json => render_json(records),
    }
}

/// 默认日志路径：可执行文件同级目录。
pub fn default_path(format: LogFormat) -> PathBuf {
    let dir = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf))
        .unwrap_or_else(|| PathBuf::from("."));
    dir.join(format!("pdfbatch-log-{}.{}", timestamp(), format.extension()))
}

/// 在指定目录下生成一个带时间戳的日志路径。
pub fn path_in(dir: &Path, format: LogFormat) -> PathBuf {
    dir.join(format!("pdfbatch-log-{}.{}", timestamp(), format.extension()))
}

/// 写日志到指定路径（父目录会自动创建）。
pub fn write(records: &[LogRecord], format: LogFormat, path: &Path) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }
    std::fs::write(path, render(records, format))
}

fn file_name(path: &Path) -> String {
    path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

fn render_csv(records: &[LogRecord]) -> String {
    let mut out = String::from("文件名,完整路径,页码,结果,原因,新页面尺寸,统一页宽,最小实际PPI\n");
    for record in records {
        let page = record.page.map(|p| p.to_string()).unwrap_or_default();
        out.push_str(&csv_field(&record.file));
        out.push(',');
        out.push_str(&csv_field(&record.path));
        out.push(',');
        out.push_str(&csv_field(&page));
        out.push(',');
        out.push_str(&csv_field(&record.result));
        out.push(',');
        out.push_str(&csv_field(&record.reason));
        out.push(',');
        out.push_str(&csv_field(&record.size));
        out.push(',');
        out.push_str(&csv_field(&record.scan_width));
        out.push(',');
        out.push_str(&csv_field(&record.min_ppi));
        out.push('\n');
    }
    out
}

fn render_json(records: &[LogRecord]) -> String {
    let mut out = String::from("[\n");
    for (index, record) in records.iter().enumerate() {
        let page = record.page.map(|p| p.to_string()).unwrap_or_else(|| "null".to_string());
        out.push_str("  {\n");
        out.push_str(&format!("    \"file\": \"{}\",\n", json_escape(&record.file)));
        out.push_str(&format!("    \"path\": \"{}\",\n", json_escape(&record.path)));
        out.push_str(&format!("    \"page\": {page},\n"));
        out.push_str(&format!("    \"result\": \"{}\",\n", json_escape(&record.result)));
        out.push_str(&format!("    \"reason\": \"{}\",\n", json_escape(&record.reason)));
        out.push_str(&format!("    \"size\": \"{}\",\n", json_escape(&record.size)));
        out.push_str(&format!("    \"scan_width\": \"{}\",\n", json_escape(&record.scan_width)));
        out.push_str(&format!("    \"min_ppi\": \"{}\"\n", json_escape(&record.min_ppi)));
        out.push_str(if index + 1 == records.len() { "  }\n" } else { "  },\n" });
    }
    out.push_str("]\n");
    out
}

/// CSV 字段转义：含逗号 / 引号 / 换行时用引号包住，内部引号翻倍。
fn csv_field(text: &str) -> String {
    if text.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", text.replace('"', "\"\""))
    } else {
        text.to_string()
    }
}

/// JSON 字符串转义。
fn json_escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

/// 本地时间戳 `YYYYMMDD-HHMMSS`（东八区）。
///
/// 只为了文件名好认，不引入时间库：把 Unix 秒换算成日期，再手动 +8 小时。
fn timestamp() -> String {
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
        + 8 * 3600;
    let days = seconds.div_euclid(86_400);
    let rest = seconds.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    let (hour, minute, second) = (rest / 3600, (rest % 3600) / 60, rest % 60);
    format!("{year:04}{month:02}{day:02}-{hour:02}{minute:02}{second:02}")
}

/// 由「1970-01-01 起的天数」算出公历年月日（Howard Hinnant 的算法）。
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if month <= 2 { year + 1 } else { year }, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(page: Option<usize>, result: &str, reason: &str, size: &str) -> LogRecord {
        LogRecord {
            file: "a.pdf".to_string(),
            path: "D:/x/a.pdf".to_string(),
            page,
            result: result.to_string(),
            reason: reason.to_string(),
            size: size.to_string(),
            // 这两列只在扫描件适配时才有值，这里统一留空。
            scan_width: String::new(),
            min_ppi: String::new(),
        }
    }

    #[test]
    fn csv_has_a_header_and_escapes_specials() {
        let records = vec![
            record(Some(1), "成功", "适应页面 → 595.28×841.89", "595.28×841.89"),
            record(None, "跳过", "没有对应的书签段", ""),
        ];
        let text = render(&records, LogFormat::Csv);
        assert!(
            text.starts_with("文件名,完整路径,页码,结果,原因,新页面尺寸,统一页宽,最小实际PPI\n"),
            "{text}"
        );
        assert!(text.contains("a.pdf,D:/x/a.pdf,1,成功"), "{text}");
        assert!(text.contains("a.pdf,D:/x/a.pdf,,跳过"), "{text}");

        // 含逗号的字段会被引号包住。
        let tricky = vec![record(None, "失败", "读不到文件, 已跳过", "")];
        let text = render(&tricky, LogFormat::Csv);
        assert!(text.contains("\"读不到文件, 已跳过\""), "{text}");
    }

    /// 扫描件适配的两列：统一页宽与最小实际 PPI。
    #[test]
    fn scan_columns_carry_the_width_and_ppi() {
        let mut sample = record(Some(1), "成功", "贴合图像", "187.08×284.16");
        sample.scan_width = "187.08".to_string();
        sample.min_ppi = "600.21".to_string();
        let csv = render(std::slice::from_ref(&sample), LogFormat::Csv);
        assert!(csv.contains("187.08×284.16,187.08,600.21"), "{csv}");
        let json = render(&[sample], LogFormat::Json);
        assert!(json.contains("\"scan_width\": \"187.08\""), "{json}");
        assert!(json.contains("\"min_ppi\": \"600.21\""), "{json}");
    }

    #[test]
    fn json_is_well_formed_and_escapes() {
        let records = vec![record(Some(3), "成功", "贴合 1 张图像\n第二行", "612.00×792.00")];
        let text = render(&records, LogFormat::Json);
        assert!(text.starts_with("[\n"), "{text}");
        assert!(text.contains("\"page\": 3"), "{text}");
        assert!(text.contains("\\n"), "换行应当转义：{text}");
        assert!(text.trim_end().ends_with(']'), "{text}");
        // 字段级空值用 null。
        let empty = render(&[record(None, "成功", "x", "")], LogFormat::Json);
        assert!(empty.contains("\"page\": null"), "{empty}");
    }

    #[test]
    fn format_parsing_and_extensions() {
        assert_eq!(LogFormat::parse("CSV").unwrap(), LogFormat::Csv);
        assert_eq!(LogFormat::parse("json").unwrap().extension(), "json");
        assert!(LogFormat::parse("xml").is_err());
    }

    #[test]
    fn default_path_looks_like_a_timestamped_log() {
        let path = default_path(LogFormat::Csv);
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        assert!(name.starts_with("pdfbatch-log-"), "{name}");
        assert!(name.ends_with(".csv"), "{name}");
        // 形如 pdfbatch-log-20261013-153000.csv
        assert_eq!(name.len(), "pdfbatch-log-20261013-153000.csv".len(), "{name}");
    }

    #[test]
    fn the_date_conversion_matches_known_days() {
        // 1970-01-01。
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        // 2026-10-13 距 1970-01-01 是 20739 天。
        assert_eq!(civil_from_days(20_739), (2026, 10, 13));
        // 闰日：2024-02-29。
        assert_eq!(civil_from_days(19_782), (2024, 2, 29));
    }

    #[test]
    fn writing_creates_the_parent_folder() {
        let dir = std::env::temp_dir().join(format!("pdfbatch-log-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let file = dir.join("nested").join("log.csv");
        let records = vec![record(None, "成功", "测试", "")];
        write(&records, LogFormat::Csv, &file).unwrap();
        let text = std::fs::read_to_string(&file).unwrap();
        assert!(text.contains("测试"), "{text}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
