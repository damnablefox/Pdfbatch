//! 页范围解析："all"、"1-5,8,10-"、"all except first,last" → 0 基页索引。
//!
//! `--pages` 一律按**从 1 开始**的页码书写（与用户看到的页码一致）。
//! 为了适配「不同 PDF 页数不同、但结构相似」的场景（首页=封面、末页=封底），
//! 支持与页数无关的相对写法：
//!
//! - 负索引：`-1` 末页、`-2` 倒数第 2 页；
//! - 关键字：`first`（=1）、`last`（=-1）；
//! - 新范围符 `..`，端点可为负数：`2..-2`（第 2 页到倒数第 2 页）、`5..`（到末页）；
//! - 排除：`all except first,last`（跳过封面封底）、`1..100 except 1,-1`；
//! - 旧写法 `1-5,8,10-` 完全兼容。

/// 把页范围文本解析成升序去重的 0 基页索引。
pub fn parse_pages(spec: &str, page_count: usize) -> std::result::Result<Vec<usize>, String> {
    let text = spec.trim();
    let (main, except) = split_except(text)?;
    let mut out = parse_set(main, page_count)?;
    if let Some(excluded_text) = except {
        let excluded = parse_set(excluded_text, page_count)?;
        out.retain(|page| !excluded.contains(page));
    }
    Ok(out)
}

/// 解析一组页（逗号分隔、可含 `all`），结果升序去重。
fn parse_set(text: &str, page_count: usize) -> std::result::Result<Vec<usize>, String> {
    let text = text.trim();
    if text.is_empty() || text.eq_ignore_ascii_case("all") {
        return Ok((0..page_count).collect());
    }
    let mut out: Vec<usize> = Vec::new();
    for part in text.split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        out.extend(parse_item(part, page_count)?);
    }
    out.sort_unstable();
    out.dedup();
    Ok(out)
}

/// 把一个「项」解析成 0 基页索引列表：单页、`a..b`、`a-b`（旧式）。
fn parse_item(part: &str, page_count: usize) -> std::result::Result<Vec<usize>, String> {
    // 新范围符 `..`：端点可以是负数、`first`/`last`，终点留空表示到末页。
    if let Some((start_text, end_text)) = part.split_once("..") {
        let (start, end) = parse_range_ends(start_text, end_text, part, page_count)?;
        return Ok((start - 1..end).collect());
    }
    // 旧式 `a-b`：仅当整体不是负索引时才按范围解释（避免与 `-1` 冲突）。
    if !is_negative_page(part) {
        if let Some((start_text, end_text)) = part.split_once('-') {
            let (start, end) = parse_range_ends(start_text, end_text, part, page_count)?;
            return Ok((start - 1..end).collect());
        }
    }
    Ok(vec![parse_page(part, part, page_count)? - 1])
}

/// 解析范围两端（1 基、含端），起点留空视为非法，终点留空视为末页。
fn parse_range_ends(
    start_text: &str,
    end_text: &str,
    part: &str,
    page_count: usize,
) -> std::result::Result<(usize, usize), String> {
    let start_text = start_text.trim();
    if start_text.is_empty() {
        return Err(format!("页范围 `{part}`：缺少起点"));
    }
    let start = parse_page(start_text, part, page_count)?;
    let end = if end_text.trim().is_empty() {
        page_count
    } else {
        parse_page(end_text.trim(), part, page_count)?
    };
    if start > end {
        return Err(format!("页范围 `{part}`：起点大于终点"));
    }
    Ok((start, end))
}

/// 解析单个页码（1 基）：`3`、`-1`、`first`、`last`。
fn parse_page(text: &str, part: &str, page_count: usize) -> std::result::Result<usize, String> {
    let text = text.trim();
    if text.eq_ignore_ascii_case("first") {
        if page_count == 0 {
            return Err(format!("页范围 `{part}`：文档没有页面"));
        }
        return Ok(1);
    }
    if text.eq_ignore_ascii_case("last") {
        if page_count == 0 {
            return Err(format!("页范围 `{part}`：文档没有页面"));
        }
        return Ok(page_count);
    }
    if let Some(rest) = text.strip_prefix('-') {
        let valid = !rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit());
        let n: usize = if valid {
            rest.parse()
                .map_err(|_| format!("页范围 `{part}`：`{text}` 不是合法的页码"))?
        } else {
            return Err(format!("页范围 `{part}`：`{text}` 不是合法的页码"));
        };
        if n == 0 {
            return Err(format!("页范围 `{part}`：`-0` 不是合法的页码"));
        }
        if n > page_count {
            return Err(format!(
                "页范围 `{part}`：倒数第 {n} 页超出文档页数（共 {page_count} 页）"
            ));
        }
        return Ok(page_count + 1 - n);
    }
    let n: usize = text
        .parse()
        .map_err(|_| format!("页范围 `{part}`：`{text}` 不是数字，也不是 first/last"))?;
    if n == 0 {
        return Err(format!("页范围 `{part}`：页码从 1 开始"));
    }
    if n > page_count {
        return Err(format!("页范围 `{part}`：超出文档页数（共 {page_count} 页）"));
    }
    Ok(n)
}

/// 形如 `-3`（整体是一个负索引页码）而不是范围。
fn is_negative_page(text: &str) -> bool {
    text.strip_prefix('-')
        .is_some_and(|rest| !rest.is_empty() && rest.chars().all(|c| c.is_ascii_digit()))
}

/// 按词边界的 `except` 把文本切成「主集合」与「排除集合」。
fn split_except(text: &str) -> std::result::Result<(&str, Option<&str>), String> {
    let lower = text.to_ascii_lowercase();
    let mut found: Option<usize> = None;
    for (index, _) in lower.match_indices("except") {
        let before_ok = index == 0
            || text[..index]
                .chars()
                .next_back()
                .is_some_and(char::is_whitespace);
        let after_index = index + "except".len();
        let after_ok = after_index >= text.len()
            || text[after_index..]
                .chars()
                .next()
                .is_some_and(char::is_whitespace);
        if before_ok && after_ok {
            if found.is_some() {
                return Err("页范围：`except` 只能出现一次".to_string());
            }
            found = Some(index);
        }
    }
    match found {
        None => Ok((text, None)),
        Some(index) => {
            let main = text[..index].trim();
            let excluded = text[index + "except".len()..].trim();
            if excluded.is_empty() {
                return Err("页范围：`except` 后面没有写要排除的页".to_string());
            }
            Ok((main, Some(excluded)))
        }
    }
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

    #[test]
    fn negative_indexes_and_keywords() {
        assert_eq!(parse_pages("-1", 5).unwrap(), vec![4]);
        assert_eq!(parse_pages("last", 5).unwrap(), vec![4]);
        assert_eq!(parse_pages("first", 5).unwrap(), vec![0]);
        assert_eq!(parse_pages("-2,-1", 5).unwrap(), vec![3, 4]);
        assert_eq!(parse_pages("LAST", 5).unwrap(), vec![4]);
    }

    #[test]
    fn dotdot_ranges_and_except() {
        assert_eq!(parse_pages("2..-2", 5).unwrap(), vec![1, 2, 3]);
        assert_eq!(parse_pages("2..", 5).unwrap(), vec![1, 2, 3, 4]);
        assert_eq!(parse_pages("..3", 5).is_err(), true, "缺少起点的范围应当报错");
        // 与页数无关的「跳过封面封底」。
        assert_eq!(parse_pages("all except first,last", 5).unwrap(), vec![1, 2, 3]);
        assert_eq!(parse_pages("all except -1", 5).unwrap(), vec![0, 1, 2, 3]);
        assert_eq!(parse_pages("1..-2 except 2", 5).unwrap(), vec![0, 2, 3]);
        assert_eq!(parse_pages("all except last,first", 400).unwrap().len(), 398);
    }

    #[test]
    fn bad_relative_pages_are_errors() {
        assert!(parse_pages("-1", 0).is_err());
        assert!(parse_pages("-6", 5).is_err());
        assert!(parse_pages("all except", 5).is_err());
        assert!(parse_pages("1 except 2 except 3", 5).is_err());
        assert!(parse_pages("-0", 5).is_err());
        assert!(parse_pages("-x", 5).is_err());
    }
}