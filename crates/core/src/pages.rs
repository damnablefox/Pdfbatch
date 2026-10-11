//! 页范围解析："all"、"1-5,8,10-" → 0 基页索引。
//!
//! `--pages` 一律按**从 1 开始**的页码书写（与用户看到的页码一致）。

/// 把页范围文本解析成升序去重的 0 基页索引。
///
/// 支持：`all`（或留空）、单个页码 `3`、闭区间 `1-5`、开区间 `10-`（到末页）、
/// 以及用逗号组合的任意多项。
pub fn parse_pages(spec: &str, page_count: usize) -> std::result::Result<Vec<usize>, String> {
    let text = spec.trim();
    if text.is_empty() || text.eq_ignore_ascii_case("all") {
        return Ok((0..page_count).collect());
    }
    let mut out: Vec<usize> = Vec::new();
    for part in text.split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let (start_text, end_text) = match part.split_once('-') {
            Some((a, b)) => (a.trim(), b.trim()),
            None => (part, part),
        };
        let start: usize = start_text
            .parse()
            .map_err(|_| format!("页范围 `{part}` 的起点不是数字"))?;
        if start == 0 {
            return Err(format!("页范围 `{part}`：页码从 1 开始"));
        }
        let end: usize = if end_text.is_empty() {
            page_count
        } else {
            end_text
                .parse()
                .map_err(|_| format!("页范围 `{part}` 的终点不是数字"))?
        };
        if start > end {
            return Err(format!("页范围 `{part}`：起点大于终点"));
        }
        if end > page_count {
            return Err(format!("页范围 `{part}`：超出文档页数（共 {page_count} 页）"));
        }
        out.extend(start - 1..end);
    }
    out.sort_unstable();
    out.dedup();
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_and_empty_mean_every_page() {
        assert_eq!(parse_pages("all", 3).unwrap(), vec![0, 1, 2]);
        assert_eq!(parse_pages("ALL", 3).unwrap(), vec![0, 1, 2]);
        assert_eq!(parse_pages("", 2).unwrap(), vec![0, 1]);
        assert_eq!(parse_pages("all", 0).unwrap(), Vec::<usize>::new());
    }

    #[test]
    fn ranges_lists_and_open_ends() {
        assert_eq!(parse_pages("1-5,8", 10).unwrap(), vec![0, 1, 2, 3, 4, 7]);
        assert_eq!(parse_pages("10-", 10).unwrap(), vec![9]);
        assert_eq!(parse_pages("3", 5).unwrap(), vec![2]);
        // 重叠与乱序都会被整理成升序去重。
        assert_eq!(parse_pages("5,1-3,3", 6).unwrap(), vec![0, 1, 2, 4]);
    }

    #[test]
    fn out_of_range_is_an_error() {
        assert!(parse_pages("6", 5).is_err());
        assert!(parse_pages("1-6", 5).is_err());
        assert!(parse_pages("5-3", 5).is_err());
        assert!(parse_pages("0", 5).is_err());
        assert!(parse_pages("x", 5).is_err());
    }
}