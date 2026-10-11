//! PDF 标准混合模式（ISO 32000-2 §11.6.3）与 ExtGState `/BM` 的映射。

/// 16 种标准混合模式。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Blend {
    Normal,
    Multiply,
    Screen,
    Overlay,
    Darken,
    Lighten,
    ColorDodge,
    ColorBurn,
    HardLight,
    SoftLight,
    Difference,
    Exclusion,
    Hue,
    Saturation,
    Color,
    Luminosity,
}

/// 全部合法取值，按 PDF 里的拼写，用于 `--help` 与错误提示。
pub const BLEND_NAMES: [&str; 16] = [
    "Normal",
    "Multiply",
    "Screen",
    "Overlay",
    "Darken",
    "Lighten",
    "ColorDodge",
    "ColorBurn",
    "HardLight",
    "SoftLight",
    "Difference",
    "Exclusion",
    "Hue",
    "Saturation",
    "Color",
    "Luminosity",
];

impl Blend {
    /// 解析用户输入：大小写不敏感，允许 `ColorDodge`、`color-dodge`、`color_dodge` 等写法。
    pub fn parse(s: &str) -> std::result::Result<Self, String> {
        let key: String = s
            .trim()
            .chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .collect::<String>()
            .to_ascii_lowercase();
        let blend = match key.as_str() {
            "normal" => Blend::Normal,
            "multiply" => Blend::Multiply,
            "screen" => Blend::Screen,
            "overlay" => Blend::Overlay,
            "darken" => Blend::Darken,
            "lighten" => Blend::Lighten,
            "colordodge" => Blend::ColorDodge,
            "colorburn" => Blend::ColorBurn,
            "hardlight" => Blend::HardLight,
            "softlight" => Blend::SoftLight,
            "difference" => Blend::Difference,
            "exclusion" => Blend::Exclusion,
            "hue" => Blend::Hue,
            "saturation" => Blend::Saturation,
            "color" => Blend::Color,
            "luminosity" => Blend::Luminosity,
            _ => return Err(format!("未知混合模式 `{s}`；可用值：{}", BLEND_NAMES.join("、"))),
        };
        Ok(blend)
    }

    /// 规范拼写。
    pub fn canonical(self) -> &'static str {
        match self {
            Blend::Normal => "Normal",
            Blend::Multiply => "Multiply",
            Blend::Screen => "Screen",
            Blend::Overlay => "Overlay",
            Blend::Darken => "Darken",
            Blend::Lighten => "Lighten",
            Blend::ColorDodge => "ColorDodge",
            Blend::ColorBurn => "ColorBurn",
            Blend::HardLight => "HardLight",
            Blend::SoftLight => "SoftLight",
            Blend::Difference => "Difference",
            Blend::Exclusion => "Exclusion",
            Blend::Hue => "Hue",
            Blend::Saturation => "Saturation",
            Blend::Color => "Color",
            Blend::Luminosity => "Luminosity",
        }
    }

    /// 写进 ExtGState 的 `/BM` 名字；`Normal` 是 PDF 的默认值，不必写。
    pub fn pdf_name(self) -> Option<&'static str> {
        (self != Blend::Normal).then(|| self.canonical())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spellings_are_flexible() {
        assert_eq!(Blend::parse("multiply").unwrap(), Blend::Multiply);
        assert_eq!(Blend::parse("Multiply").unwrap(), Blend::Multiply);
        assert_eq!(Blend::parse("color-dodge").unwrap(), Blend::ColorDodge);
        assert_eq!(Blend::parse("ColorDodge").unwrap(), Blend::ColorDodge);
        assert_eq!(Blend::parse(" color_burn ").unwrap(), Blend::ColorBurn);
    }

    #[test]
    fn normal_is_omitted_from_the_ext_gstate() {
        assert_eq!(Blend::Normal.pdf_name(), None);
        assert_eq!(Blend::Multiply.pdf_name(), Some("Multiply"));
    }

    #[test]
    fn every_name_parses_back() {
        for name in BLEND_NAMES {
            assert_eq!(Blend::parse(name).unwrap().canonical(), name);
        }
        assert!(Blend::parse("nope").is_err());
    }
}