//! 长度单位换算与常见纸张尺寸。
//!
//! PDF 的用户空间单位是「点」（1 pt = 1/72 英寸）。用户在界面上可能用毫米、
//! 厘米或英寸输入，这里统一换算成点；纸张预设也在这里给出。

/// 一毫米等于多少点。
const MM: f64 = 72.0 / 25.4;

/// 长度单位。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unit {
    Pt,
    Mm,
    Cm,
    In,
}

impl Unit {
    /// 解析单位写法（大小写不敏感，也认中文）。
    pub fn parse(text: &str) -> std::result::Result<Self, String> {
        match text.trim().to_ascii_lowercase().as_str() {
            "pt" | "point" | "points" | "点" => Ok(Unit::Pt),
            "mm" | "millimeter" | "millimeters" | "毫米" => Ok(Unit::Mm),
            "cm" | "centimeter" | "centimeters" | "厘米" => Ok(Unit::Cm),
            "in" | "inch" | "inches" | "英寸" => Ok(Unit::In),
            _ => Err(format!("未知长度单位 `{text}`；可用：pt、mm、cm、in")),
        }
    }

    pub fn canonical(self) -> &'static str {
        match self {
            Unit::Pt => "pt",
            Unit::Mm => "mm",
            Unit::Cm => "cm",
            Unit::In => "in",
        }
    }

    /// 该单位下的一个值是多少点。
    pub fn to_pt(self, value: f64) -> f64 {
        match self {
            Unit::Pt => value,
            Unit::Mm => value * MM,
            Unit::Cm => value * 10.0 * MM,
            Unit::In => value * 72.0,
        }
    }

    /// 点换算回该单位（界面回显用）。
    pub fn from_pt(self, points: f64) -> f64 {
        match self {
            Unit::Pt => points,
            Unit::Mm => points / MM,
            Unit::Cm => points / (10.0 * MM),
            Unit::In => points / 72.0,
        }
    }
}

/// 预设纸张（纵向，宽 × 高，单位点）。
///
/// A0–A6 按 ISO 216 的毫米尺寸换算；Letter / Legal / Tabloid 按英寸。
pub const PRESETS: [(&str, f64, f64); 10] = [
    ("A0", 841.0 * MM, 1189.0 * MM),
    ("A1", 594.0 * MM, 841.0 * MM),
    ("A2", 420.0 * MM, 594.0 * MM),
    ("A3", 297.0 * MM, 420.0 * MM),
    ("A4", 210.0 * MM, 297.0 * MM),
    ("A5", 148.0 * MM, 210.0 * MM),
    ("A6", 105.0 * MM, 148.0 * MM),
    ("Letter", 612.0, 792.0),
    ("Legal", 612.0, 1008.0),
    ("Tabloid", 792.0, 1224.0),
];

/// 按名字查预设纸张（大小写不敏感），返回纵向的 `(宽, 高)`（点）。
pub fn preset(name: &str) -> Option<(f64, f64)> {
    let want = name.trim().to_ascii_lowercase();
    PRESETS
        .iter()
        .find(|(label, _, _)| label.to_ascii_lowercase() == want)
        .map(|(_, w, h)| (*w, *h))
}

/// 预设名字列表（帮助文本与下拉框用）。
pub fn preset_names() -> Vec<&'static str> {
    PRESETS.iter().map(|(label, _, _)| *label).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn units_round_trip_through_points() {
        for unit in [Unit::Pt, Unit::Mm, Unit::Cm, Unit::In] {
            let points = unit.to_pt(100.0);
            let back = unit.from_pt(points);
            assert!((back - 100.0).abs() < 1e-9, "{unit:?} 往返应当一致");
        }
    }

    #[test]
    fn known_conversions_are_right() {
        // 1 英寸 = 72 点；2.54 厘米 = 1 英寸 = 72 点。
        assert!((Unit::In.to_pt(1.0) - 72.0).abs() < 1e-9);
        assert!((Unit::Cm.to_pt(2.54) - 72.0).abs() < 1e-6);
        assert!((Unit::Mm.to_pt(25.4) - 72.0).abs() < 1e-6);
    }

    #[test]
    fn parsing_accepts_common_spellings() {
        assert_eq!(Unit::parse("PT").unwrap(), Unit::Pt);
        assert_eq!(Unit::parse(" mm ").unwrap(), Unit::Mm);
        assert_eq!(Unit::parse("厘米").unwrap(), Unit::Cm);
        assert_eq!(Unit::parse("inch").unwrap(), Unit::In);
        assert!(Unit::parse("furlong").is_err());
    }

    #[test]
    fn paper_presets_match_the_standards() {
        // A4 = 210×297 mm ≈ 595.28×841.89 pt；Letter = 8.5×11 in = 612×792 pt。
        let (w, h) = preset("a4").unwrap();
        assert!((w - 595.2756).abs() < 0.01, "{w}");
        assert!((h - 841.8898).abs() < 0.01, "{h}");
        assert_eq!(preset("Letter").unwrap(), (612.0, 792.0));
        assert_eq!(preset("TABLOID").unwrap(), (792.0, 1224.0));
        assert!(preset("A7").is_none());
        assert_eq!(preset_names().len(), PRESETS.len());
    }
}
