//! 颜色解析：支持十六进制、0–255 整数与 0.0–1.0 归一化浮点三种写法。
//!
//! - 十六进制：`#RRGGBB`、`RRGGBB`、`#RGB`、`RGB`（大小写均可）；
//! - 0–255 整数：`R,G,B`（如 `255,237,203`）；
//! - 归一化浮点：`R,G,B`（如 `1.0,0.93,0.8`）。
//!
//! 逗号分隔的写法里，**只要有一个分量带小数点就按 0.0–1.0 解释**，否则按 0–255 解释。
//! 因此 `128,128,128` 是中灰、`0.5,0.5,0.5` 也是中灰。

/// 解析颜色，返回 0.0–1.0 的 RGB 分量。
pub fn parse_color(s: &str) -> std::result::Result<[f64; 3], String> {
    let t = s.trim();
    if t.contains(',') {
        return parse_triplet(t, s);
    }
    parse_hex(t, s)
}

/// 十六进制写法（可带 `#`）。
fn parse_hex(t: &str, original: &str) -> std::result::Result<[f64; 3], String> {
    let hex = t.strip_prefix('#').unwrap_or(t);
    let expanded: String = match hex.len() {
        3 => hex.chars().flat_map(|c| [c, c]).collect(),
        6 => hex.to_string(),
        n => {
            return Err(format!(
                "颜色 `{original}` 应为 3 位或 6 位十六进制（如 #FFF3B0）、R,G,B 整数（0–255）或 R,G,B 浮点（0.0–1.0），实际 {n} 位"
            ))
        }
    };
    if !expanded.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(format!("颜色 `{original}` 含有非十六进制字符"));
    }
    let v = u32::from_str_radix(&expanded, 16).map_err(|e| format!("颜色 `{original}` 无法解析：{e}"))?;
    Ok([
        f64::from((v >> 16) & 0xFF) / 255.0,
        f64::from((v >> 8) & 0xFF) / 255.0,
        f64::from(v & 0xFF) / 255.0,
    ])
}

/// `R,G,B` 写法：带小数点按 0.0–1.0，否则按 0–255。
fn parse_triplet(t: &str, original: &str) -> std::result::Result<[f64; 3], String> {
    let parts: Vec<&str> = t.split(',').map(str::trim).collect();
    if parts.len() != 3 {
        return Err(format!(
            "颜色 `{original}` 的 R,G,B 需要 3 个分量（实际给了 {} 个）",
            parts.len()
        ));
    }
    let float = parts.iter().any(|p| p.contains('.'));
    let (low, high, scale) = if float { (0.0, 1.0, 1.0) } else { (0.0, 255.0, 255.0) };
    let mut out = [0.0_f64; 3];
    for (index, part) in parts.iter().enumerate() {
        let value: f64 = part
            .parse()
            .map_err(|_| format!("颜色 `{original}` 的第 {} 个分量 `{part}` 不是数字", index + 1))?;
        if !value.is_finite() || value < low || value > high {
            return Err(format!(
                "颜色 `{original}` 的第 {} 个分量 `{part}` 超出 {} 范围",
                index + 1,
                if float { "0.0–1.0" } else { "0–255" }
            ));
        }
        out[index] = value / scale;
    }
    Ok(out)
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
    fn integer_and_float_triplets_agree() {
        // 0–255 整数与归一化浮点描述同一种颜色时结果一致。
        assert_eq!(parse_color("227,237,203").unwrap(), parse_color("#E3EDCB").unwrap());
        let by_float = parse_color("0.890196,0.929412,0.796078").unwrap();
        let by_int = parse_color("227,237,203").unwrap();
        for (a, b) in by_float.iter().zip(by_int.iter()) {
            assert!((a - b).abs() < 1e-5, "{by_float:?} vs {by_int:?}");
        }
        // 恰好在 0..1 的整数写法按 0–255 解释，而不是归一化。
        assert_eq!(parse_color("1,1,1").unwrap(), [1.0 / 255.0, 1.0 / 255.0, 1.0 / 255.0]);
    }

    #[test]
    fn bad_input_is_rejected() {
        assert!(parse_color("#FFFF").is_err());
        assert!(parse_color("#GGGGGG").is_err());
        assert!(parse_color("").is_err());
        // 分量个数不对、越界、非数字都要报错。
        assert!(parse_color("1,2").is_err());
        assert!(parse_color("300,0,0").is_err());
        assert!(parse_color("1.5,0,0").is_err());
        assert!(parse_color("a,b,c").is_err());
    }
}