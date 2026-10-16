//! 「页面尺寸」模式的参数草稿，以及缩放/扫描参数的换算与模式可用性判定。

use pdfbatch_core::pagesize::{Orientation, ScaleMode, ScaleSpec, Target};
use pdfbatch_core::scan::{self, ScanSpec};
use pdfbatch_core::units::{self, Unit};

/// 「页面尺寸」的两种做法。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SizeMethod {
    /// 按目标尺寸缩放内容。
    Scale,
    /// 按扫描件图像适配。
    Scan,
}

impl SizeMethod {
    pub const ALL: [SizeMethod; 2] = [SizeMethod::Scale, SizeMethod::Scan];

    pub fn label(self) -> &'static str {
        match self {
            SizeMethod::Scale => "按目标尺寸缩放",
            SizeMethod::Scan => "扫描件适配",
        }
    }
}

/// 目标尺寸的来源：预设纸张，还是自己填宽高。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SizeSource {
    Preset,
    Custom,
}

/// 「页面尺寸」模式的参数草稿。
#[derive(Clone, Debug)]
pub struct PageSizeDraft {
    pub method: SizeMethod,

    // —— 缩放 ——
    pub source: SizeSource,
    /// 预设纸张名（如 `A4`）。
    pub preset: String,
    /// 自定义宽高（按 `unit` 解释）；任一为 0 表示"这一维不约束"。
    pub width: f64,
    pub height: f64,
    pub unit: Unit,
    /// 纸张方向：只在预设下生效（自定义尺寸下宽度与高度已分别指定）。
    pub orientation: Orientation,
    pub scale_mode: ScaleMode,
    pub percent: f64,

    // —— 扫描件适配 ——
    /// 彩色 / 灰度 / 位图三种图像各自的 PPI。
    pub color_ppi: f64,
    pub gray_ppi: f64,
    pub mono_ppi: f64,

    /// 页范围原文。
    pub pages: String,
}

impl Default for PageSizeDraft {
    fn default() -> Self {
        Self {
            method: SizeMethod::Scale,

            source: SizeSource::Preset,
            preset: "A4".to_string(),
            width: 100.0,
            height: 100.0,
            unit: Unit::Mm,
            // 默认跟随原页面：改尺寸时不会把横向页莫名转成纵向。
            orientation: Orientation::Auto,
            scale_mode: ScaleMode::Fit,
            percent: 100.0,

            color_ppi: scan::DEFAULT_COLOR_PPI,
            gray_ppi: scan::DEFAULT_GRAY_PPI,
            mono_ppi: scan::DEFAULT_MONO_PPI,

            pages: "all".to_string(),
        }
    }
}

/// 当前尺寸设置下可用的缩放模式。
///
/// 规则（与设计方案一致）：
/// - 预设纸张下不能选**适应宽度 / 适应高度**：它们会改写输出页面的另一维，
///   会让"输出不等于所选的纸"。
/// - 自定义尺寸下，某一维为 0 表示"这一维不约束"，此时只有**只依赖另一维**的模式成立；
///   宽高同时为 0 时没有任何模式可用（由校验报错）。
pub fn allowed_modes(draft: &PageSizeDraft) -> Vec<ScaleMode> {
    ScaleMode::ALL
        .iter()
        .copied()
        .filter(|mode| match draft.source {
            SizeSource::Preset => !mode.custom_only(),
            SizeSource::Custom => {
                let no_width = !(draft.width > 0.0);
                let no_height = !(draft.height > 0.0);
                (!no_width || !mode.needs_width()) && (!no_height || !mode.needs_height())
            }
        })
        .collect()
}

/// 把不可用的当前模式自动切到可用项；返回切换说明，未切换时返回 `None`。
///
/// 参数恢复为非 0 后**不会**自动切回，避免下拉项来回跳动。
pub fn legalize(draft: &mut PageSizeDraft) -> Option<String> {
    let allowed = allowed_modes(draft);
    if allowed.contains(&draft.scale_mode) {
        return None;
    }
    let next = allowed.first().copied()?;
    let previous = draft.scale_mode;
    draft.scale_mode = next;
    Some(format!(
        "「{}」在当前的尺寸设置下不可用，已切换到「{}」",
        previous.label(),
        next.label()
    ))
}

/// 由草稿得出缩放参数；宽高按所选单位换算成点。
pub(crate) fn scale_spec(draft: &PageSizeDraft) -> std::result::Result<ScaleSpec, String> {
    let target = match draft.source {
        SizeSource::Preset => {
            let (w, h) = units::preset(&draft.preset)
                .ok_or_else(|| format!("未知纸张尺寸 `{}`", draft.preset))?;
            Target::paper(w, h, draft.orientation)
        }
        SizeSource::Custom => {
            let (w, h) = (draft.unit.to_pt(draft.width), draft.unit.to_pt(draft.height));
            if !(w.is_finite() && h.is_finite() && w >= 0.0 && h >= 0.0) {
                return Err("自定义宽高必须是非负数值".to_string());
            }
            if w == 0.0 && h == 0.0 {
                return Err("宽度与高度不能同时为 0".to_string());
            }
            Target::custom(w, h)
        }
    };
    if !allowed_modes(draft).contains(&draft.scale_mode) {
        return Err(format!("「{}」在当前尺寸设置下不可用", draft.scale_mode.label()));
    }
    if draft.scale_mode == ScaleMode::CenterScaled && !(draft.percent > 0.0) {
        return Err("缩放百分比必须大于 0".to_string());
    }
    Ok(ScaleSpec {
        target,
        mode: draft.scale_mode,
        percent: draft.percent,
    })
}

/// 由草稿得出扫描件适配参数（三种图像各自的 PPI）。
pub(crate) fn scan_spec(draft: &PageSizeDraft) -> std::result::Result<ScanSpec, String> {
    let spec = ScanSpec {
        color_ppi: draft.color_ppi,
        gray_ppi: draft.gray_ppi,
        mono_ppi: draft.mono_ppi,
    };
    spec.validate().map_err(|error| error.to_string())?;
    Ok(spec)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::job::{BookmarkDraft, CommonOptions, FillDraft, Mode};
    use pdfbatch_core::batch::Input;
    use std::path::PathBuf;

    fn input(path: &str) -> Input {
        Input {
            path: PathBuf::from(path),
            relative: PathBuf::from(PathBuf::from(path).file_name().unwrap()),
        }
    }

    /// 预设与自定义尺寸下的模式可用性，以及宽高为 0 时的自动切换。
    #[test]
    fn single_dimension_modes_follow_the_size_source() {
        // 预设纸张下适应宽度 / 适应高度不可用，只剩 5 种。
        let preset = PageSizeDraft::default();
        let available = super::allowed_modes(&preset);
        assert_eq!(available.len(), 5, "{available:?}");
        assert!(!available.contains(&ScaleMode::FitWidth));

        // 自定义 + 宽度为 0：只剩适应高度，且当前模式会被自动切换过去。
        let mut custom = PageSizeDraft {
            source: SizeSource::Custom,
            width: 0.0,
            height: 100.0,
            scale_mode: ScaleMode::Fit,
            ..PageSizeDraft::default()
        };
        assert_eq!(super::allowed_modes(&custom), vec![ScaleMode::FitHeight]);
        let notice = super::legalize(&mut custom).expect("应当自动切换");
        assert!(notice.contains("适应高度"), "{notice}");
        assert_eq!(custom.scale_mode, ScaleMode::FitHeight);

        // 自定义 + 高度为 0：只剩适应宽度；当前若是「适应页面」也会被自动切过去。
        let mut custom = PageSizeDraft {
            source: SizeSource::Custom,
            width: 100.0,
            height: 0.0,
            scale_mode: ScaleMode::Fit,
            ..PageSizeDraft::default()
        };
        assert_eq!(super::allowed_modes(&custom), vec![ScaleMode::FitWidth]);
        let notice = super::legalize(&mut custom).expect("应当自动切换");
        assert!(notice.contains("适应宽度"), "{notice}");
        assert_eq!(custom.scale_mode, ScaleMode::FitWidth);

        // 宽高同时为 0：没有任何模式可用，构建请求时报错。
        let broken = PageSizeDraft {
            source: SizeSource::Custom,
            width: 0.0,
            height: 0.0,
            ..PageSizeDraft::default()
        };
        assert!(super::allowed_modes(&broken).is_empty());
        let error = crate::job::build_request(
            Mode::PageSize,
            &FillDraft::default(),
            &broken,
            &BookmarkDraft::default(),
            &CommonOptions::default(),
            vec![input("D:/a.pdf")],
        )
        .unwrap_err();
        assert!(error.contains("同时为 0"), "{error}");
    }
}
