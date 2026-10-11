//! `#RRGGBB` / `#RGB` 颜色解析。

/// 解析 `#RRGGBB`、`RRGGBB`、`#RGB` 或 `RGB`（大小写均可）形式的颜色，
/// 返回 0.0–1.0 的 RGB 分量。
pub fn parse_color(s: &str) -> std::result::Result<[f64; 3], String> {
    let t = s.trim();
    let hex = t.strip_prefix('#').unwrap_or(t);
    let expanded: String = match hex.len() {
        3 => hex.chars().flat_map(|c| [c, c]).collect(),
        6 => hex.to_string(),
        n => return Err(format!("颜色 `{s}` 应为 3 位或 6 位十六进制（如 #FFF3B0），实际 {n} 位")),
    };
    if !expanded.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(format!("颜色 `{s}` 含有非十六进制字符"));
    }
    let v = u32::from_str_radix(&expanded, 16).map_err(|e| format!("颜色 `{s}` 无法解析：{e}"))?;
    Ok([
        f64::from((v >> 16) & 0xFF) / 255.0,
        f64::from((v >> 8) & 0xFF) / 255.0,
        f64::from(v & 0xFF) / 255.0,
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn six_and_three_digit_forms_agree() {
        assert_eq!(parse_color("#FFF3B0").unwrap(), parse_color("#fff3b0").unwrap());
        let [r, g, b] = parse_color("#FFFFFF").unwrap();
        assert_eq!([r, g, b], [1.0, 1.0, 1.0]);
        // 三位简写展开：每个字符重复一次。
        assert_eq!(parse_color("#F0A").unwrap(), [1.0, 0.0, 0xAA as f64 / 255.0]);
        assert_eq!(parse_color("000000").unwrap(), [0.0, 0.0, 0.0]);
    }

    #[test]
    fn bad_input_is_rejected() {
        assert!(parse_color("#FFFF").is_err());
        assert!(parse_color("#GGGGGG").is_err());
        assert!(parse_color("").is_err());
    }
}