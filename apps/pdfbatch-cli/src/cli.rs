//! 命令行参数解析与帮助文本。

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use pdfbatch_core::batch::{Naming, OutputMode};
use pdfbatch_core::blend::Blend;
use pdfbatch_core::color::parse_color;
use pdfbatch_core::fill::Layer;

/// 需要取值的开关；其余 `--xxx` 一律当作布尔开关。
const VALUE_FLAGS: [&str; 12] = [
    "spec",
    "out",
    "out-dir",
    "prefix",
    "suffix",
    "password-file",
    "rect",
    "color",
    "opacity",
    "blend",
    "layer",
    "pages",
];

/// 两个子命令共用的参数。
pub struct Common {
    pub inputs: Vec<PathBuf>,
    pub output: OutputMode,
    pub password_file: Option<PathBuf>,
    pub full_save: bool,
    /// 目录输入是否递归（`--no-recursive` 关闭）。
    pub recursive: bool,
}

/// `bookmarks` 子命令的参数。
pub struct Bookmarks {
    pub common: Common,
    pub spec: PathBuf,
    pub zero_based: bool,
    pub replace: bool,
    pub strict: bool,
}

/// `fill` 子命令的参数。
pub struct Fill {
    pub common: Common,
    /// 用户空间矩形 `[x, y, 宽, 高]`；`None` 表示整页。
    pub rect: Option<[f64; 4]>,
    pub color: [f64; 3],
    pub opacity: f64,
    pub blend: Blend,
    pub layer: Layer,
    /// 页范围原文，处理时按各文档的实际页数解析。
    pub pages: String,
    pub replace: bool,
}

pub enum Invocation {
    Bookmarks(Bookmarks),
    Fill(Fill),
    Help,
}

/// 解析命令行（不含程序名）。
pub fn parse(args: &[String]) -> Result<Invocation, String> {
    if args.is_empty() || args.iter().any(|a| a == "--help" || a == "-h") {
        return Ok(Invocation::Help);
    }
    let (subcommand, rest) = args.split_first().expect("上面已判过非空");
    let raw = RawArgs::parse(rest)?;
    match subcommand.as_str() {
        "bookmarks" | "bookmark" => parse_bookmarks(raw).map(Invocation::Bookmarks),
        "fill" => parse_fill(raw).map(Invocation::Fill),
        other => Err(format!("未知子命令 `{other}`；可用：bookmarks、fill。用 --help 查看用法")),
    }
}

fn parse_bookmarks(raw: RawArgs) -> Result<Bookmarks, String> {
    let common = common_args(&raw, "_bookmarked")?;
    let spec = raw
        .value("spec")
        .ok_or_else(|| "bookmarks 需要 --spec <书签文本文件>".to_string())?;
    Ok(Bookmarks {
        common,
        spec: PathBuf::from(spec),
        zero_based: raw.has("zero-based"),
        replace: raw.has("replace"),
        strict: raw.has("strict"),
    })
}

fn parse_fill(raw: RawArgs) -> Result<Fill, String> {
    let common = common_args(&raw, "_filled")?;
    if raw.has("full-page") && raw.value("rect").is_some() {
        return Err("--rect 与 --full-page 不能同时使用".to_string());
    }
    let rect = match raw.value("rect") {
        Some(text) => Some(parse_rect(text)?),
        None => None,
    };
    let color = match raw.value("color") {
        Some(text) => parse_color(text)?,
        None => [1.0, 0.92, 0.55], // 默认淡黄
    };
    let opacity = match raw.value("opacity") {
        Some(text) => {
            let value: f64 = text
                .trim()
                .parse()
                .map_err(|_| format!("--opacity 的值 `{text}` 不是数字"))?;
            if !value.is_finite() {
                return Err("--opacity 的值必须是有限数".to_string());
            }
            value.clamp(0.0, 1.0)
        }
        None => 0.2,
    };
    let blend = match raw.value("blend") {
        Some(text) => Blend::parse(text)?,
        None => Blend::Normal,
    };
    let layer = match raw.value("layer") {
        Some(text) => Layer::parse(text)?,
        None => Layer::Top,
    };
    Ok(Fill {
        common,
        rect,
        color,
        opacity,
        blend,
        layer,
        pages: raw.value("pages").unwrap_or("all").to_string(),
        replace: raw.has("replace"),
    })
}

fn common_args(raw: &RawArgs, default_suffix: &str) -> Result<Common, String> {
    let inputs: Vec<PathBuf> = raw.positional.iter().map(PathBuf::from).collect();
    if inputs.is_empty() {
        return Err("至少要给一个输入文件或目录（PDF 文件，或包含 PDF 的目录）".to_string());
    }
    let prefix = raw.value("prefix").unwrap_or("").to_string();
    let suffix_given = raw.value("suffix");
    // 命名只对“会写出新文件名”的输出位置有效。
    let reject_naming = |what: &str| -> Result<(), String> {
        if raw.value("prefix").is_some() || suffix_given.is_some() {
            Err(format!("{what}，不能用 --prefix/--suffix 命名（它们只对同目录另存与 --out-dir 有效）"))
        } else {
            Ok(())
        }
    };
    let output = match (raw.value("out"), raw.value("out-dir")) {
        (Some(_), Some(_)) => return Err("--out 与 --out-dir 不能同时使用".to_string()),
        (Some(file), None) => {
            reject_naming("--out 指向明确的输出文件")?;
            OutputMode::File(PathBuf::from(file))
        }
        (None, Some(dir)) => OutputMode::Dir {
            dir: PathBuf::from(dir),
            naming: Naming {
                prefix,
                // 指定目录不叠加默认后缀；用户给了才用。
                suffix: suffix_given.unwrap_or("").to_string(),
            },
        },
        (None, None) if raw.has("in-place") => {
            reject_naming("--in-place 覆盖原文件")?;
            OutputMode::InPlace
        }
        (None, None) => {
            let suffix = match suffix_given {
                Some(text) => text.to_string(),
                // 只给了前缀、没给后缀时不再补默认后缀，让“只用前缀”成为可能。
                None if !prefix.is_empty() => String::new(),
                None => default_suffix.to_string(),
            };
            OutputMode::SameFolder {
                naming: Naming { prefix, suffix },
            }
        }
    };
    Ok(Common {
        inputs,
        output,
        password_file: raw.value("password-file").map(PathBuf::from),
        full_save: raw.has("full-save"),
        recursive: !raw.has("no-recursive"),
    })
}

/// `x,y,宽,高`（PDF 用户空间坐标，单位点）。
fn parse_rect(text: &str) -> Result<[f64; 4], String> {
    let parts: Vec<&str> = text.split(',').map(str::trim).collect();
    if parts.len() != 4 {
        return Err(format!(
            "--rect 需要 4 个用逗号分开的数：x,y,宽,高（实际给了 {} 个）",
            parts.len()
        ));
    }
    let mut rect = [0.0_f64; 4];
    for (index, part) in parts.iter().enumerate() {
        rect[index] = part
            .parse()
            .map_err(|_| format!("--rect 的第 {} 个值 `{part}` 不是数字", index + 1))?;
    }
    Ok(rect)
}

/// 把参数分成位置参数、取值开关、布尔开关三类。
struct RawArgs {
    positional: Vec<String>,
    values: HashMap<String, String>,
    switches: HashSet<String>,
}

impl RawArgs {
    fn parse(args: &[String]) -> Result<Self, String> {
        let mut raw = RawArgs {
            positional: Vec::new(),
            values: HashMap::new(),
            switches: HashSet::new(),
        };
        let mut index = 0;
        while index < args.len() {
            let arg = &args[index];
            let Some(rest) = arg.strip_prefix("--") else {
                raw.positional.push(arg.clone());
                index += 1;
                continue;
            };
            let (key, inline) = match rest.split_once('=') {
                Some((key, value)) => (key.to_string(), Some(value.to_string())),
                None => (rest.to_string(), None),
            };
            if !VALUE_FLAGS.contains(&key.as_str()) {
                raw.switches.insert(key);
                index += 1;
                continue;
            }
            match inline {
                Some(value) => {
                    raw.values.insert(key, value);
                    index += 1;
                }
                None => {
                    let value = args
                        .get(index + 1)
                        .filter(|v| !v.starts_with("--"))
                        .ok_or_else(|| format!("`--{key}` 缺少取值"))?;
                    raw.values.insert(key, value.clone());
                    index += 2;
                }
            }
        }
        Ok(raw)
    }

    fn value(&self, key: &str) -> Option<&str> {
        self.values.get(key).map(String::as_str)
    }

    fn has(&self, key: &str) -> bool {
        self.switches.contains(key)
    }
}

/// 帮助文本。
pub fn help() -> String {
    format!(
        r##"pdfbatch —— 给 PDF 批量加书签、批量盖纯色填充层

用法：
  pdfbatch bookmarks <输入.pdf|目录>… --spec <书签文本> [选项]
  pdfbatch fill      <输入.pdf|目录>… [--rect x,y,宽,高 | --full-page] [选项]

两个子命令都接受多个文件或目录；目录默认递归收集其中的 .pdf。
默认不会改动原文件，而是在同目录另存（文件名加后缀）；用 --in-place 才覆盖原文件。

bookmarks 专用：
  --spec <文件>       书签文本（必需）。格式：
                        缩进表示层级（一个 Tab 或两个空格），
                        标题与页码用最后一个 Tab 或 ` | ` 分隔，
                        页码从 1 开始（--zero-based 改为从 0 开始），
                        `#` 开头是注释，空行忽略。例如：
                          第一章 绪论 | 1
                            1.1 研究背景 | 2
                          第二章 方法 | 10
                        一份文本可以写多个「文件段头」，把书签分给不同的文件：
                          # 文件：报告.pdf    （写完整文件名，含扩展名，大小写不敏感）
                          第一章 绪论 | 1
                          # 文件：#2          （也可按输入顺序指定，从 1 开始）
                          附录 A | 1
                        没有段头时，整份文本应用到所有文件；段头之前不能有书签行。
  --replace           先删掉原有书签（默认追加到已有书签之后）
  --strict            缩进跳级、页码越界等一律报错
  --zero-based        书签文本里的页码按 0 开始解释

fill 专用：
  --rect x,y,宽,高    填充矩形（PDF 用户空间坐标，单位点，原点在页面左下角）
  --full-page         整页填充（默认，等价于用页面 CropBox）
  --pages <范围>      要处理的页，例如 1-5,8,10- ；默认 all
  --color <颜色>      #RRGGBB 或 #RGB，默认 #FFE08C
  --opacity <0..1>    不透明度，默认 0.2
  --blend <模式>      混合模式：{}（默认 Normal）
  --layer <层级>      top = 盖在原有内容之上（默认）；bottom = 垫在原有内容之下
  --replace           先摘掉本工具此前加在该页的填充层（用来批量改混合模式/层级）

共用：
  --out <文件>        写到指定文件（只允许单个输入）
  --out-dir <目录>    写到指定目录（目录输入会保留其相对子路径）
  --in-place          覆盖原文件（先写临时文件，再原子替换）
  --prefix <前缀>     输出文件名前缀（同目录另存与 --out-dir 都可用）
  --suffix <后缀>     输出文件名后缀（同目录另存默认 _bookmarked / _filled；--out-dir 默认不加）
  --full-save         整体重写文件（体积更小，但会让已有数字签名失效）
  --password-file <文件>  加密 PDF 的密码（文件内容即为密码）
  --no-recursive      目录输入时不进入子目录
  -h, --help          显示本帮助

示例：
  pdfbatch bookmarks "D:\pdfs" --spec "D:\outline.txt" --out-dir "D:\out"
  pdfbatch fill "D:\pdfs" --full-page --color "#FFF3B0" --opacity 0.25 --blend Multiply --layer top
  pdfbatch fill "D:\a.pdf" --rect "72,72,400,120" --layer bottom --pages "1-5,8" --in-place
"##,
        pdfbatch_core::blend::BLEND_NAMES.join("、")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn bookmarks_parses_spec_and_flags() {
        let parsed = parse(&args(&["bookmarks", "D:/a.pdf", "--spec", "D:/o.txt", "--replace", "--strict"])).unwrap();
        match parsed {
            Invocation::Bookmarks(b) => {
                assert_eq!(b.spec, PathBuf::from("D:/o.txt"));
                assert!(b.replace);
                assert!(b.strict);
                assert!(!b.zero_based);
                assert_eq!(b.common.inputs.len(), 1);
                assert!(b.common.recursive);
            }
            _ => panic!("应当是 bookmarks"),
        }
    }

    #[test]
    fn fill_parses_geometry_and_style() {
        let parsed = parse(&args(&[
            "fill", "D:/a.pdf", "--rect", "10,20,100,50", "--color", "#88CCFF", "--opacity", "0.35", "--blend",
            "multiply", "--layer", "bottom", "--pages", "1-5",
        ]))
        .unwrap();
        match parsed {
            Invocation::Fill(f) => {
                assert_eq!(f.rect, Some([10.0, 20.0, 100.0, 50.0]));
                assert_eq!(f.color[2], 1.0);
                assert_eq!(f.opacity, 0.35);
                assert_eq!(f.blend, Blend::Multiply);
                assert_eq!(f.layer, Layer::Bottom);
                assert_eq!(f.pages, "1-5");
            }
            _ => panic!("应当是 fill"),
        }
    }

    #[test]
    fn defaults_are_sane() {
        match parse(&args(&["fill", "D:/a.pdf"])).unwrap() {
            Invocation::Fill(f) => {
                assert_eq!(f.rect, None, "默认整页");
                assert_eq!(f.blend, Blend::Normal);
                assert_eq!(f.layer, Layer::Top);
                assert_eq!(f.pages, "all");
                assert!(f.opacity > 0.0 && f.opacity <= 1.0);
                assert_eq!(
                    f.common.output,
                    OutputMode::SameFolder {
                        naming: Naming {
                            prefix: String::new(),
                            suffix: "_filled".into()
                        }
                    }
                );
            }
            _ => panic!("应当是 fill"),
        }
    }

    #[test]
    fn prefix_only_does_not_append_the_default_suffix() {
        match parse(&args(&["fill", "D:/a.pdf", "--prefix", "new_"])).unwrap() {
            Invocation::Fill(f) => assert_eq!(
                f.common.output,
                OutputMode::SameFolder {
                    naming: Naming {
                        prefix: "new_".into(),
                        suffix: String::new()
                    }
                }
            ),
            _ => panic!("应当是 fill"),
        }
    }

    #[test]
    fn naming_applies_to_out_dir() {
        match parse(&args(&[
            "fill", "D:/a.pdf", "--out-dir", "D:/out", "--prefix", "new_", "--suffix", "_done",
        ]))
        .unwrap()
        {
            Invocation::Fill(f) => assert_eq!(
                f.common.output,
                OutputMode::Dir {
                    dir: PathBuf::from("D:/out"),
                    naming: Naming {
                        prefix: "new_".into(),
                        suffix: "_done".into()
                    }
                }
            ),
            _ => panic!("应当是 fill"),
        }
    }

    #[test]
    fn naming_is_rejected_with_in_place_and_out_file() {
        assert!(parse(&args(&["fill", "D:/a.pdf", "--in-place", "--prefix", "x"])).is_err());
        assert!(parse(&args(&["fill", "D:/a.pdf", "--out", "D:/o.pdf", "--suffix", "_x"])).is_err());
    }

    #[test]
    fn inline_values_and_conflicts() {
        let parsed = parse(&args(&["fill", "D:/a.pdf", "--color=#FFFFFF", "--opacity=1"])).unwrap();
        match parsed {
            Invocation::Fill(f) => {
                assert_eq!(f.color, [1.0, 1.0, 1.0]);
                assert_eq!(f.opacity, 1.0);
            }
            _ => panic!("应当是 fill"),
        }
        assert!(parse(&args(&["fill", "D:/a.pdf", "--out", "x.pdf", "--out-dir", "d"])).is_err());
        assert!(parse(&args(&["fill", "D:/a.pdf", "--rect", "1,2,3", "--full-page"])).is_err());
        assert!(parse(&args(&["fill"])).is_err(), "没有输入应当报错");
        assert!(parse(&args(&["bookmarks", "D:/a.pdf"])).is_err(), "缺 --spec 应当报错");
        assert!(matches!(parse(&args(&["--help"])).unwrap(), Invocation::Help));
        assert!(parse(&args(&["nope", "D:/a.pdf"])).is_err());
    }

    #[test]
    fn opacity_is_clamped() {
        match parse(&args(&["fill", "D:/a.pdf", "--opacity", "7"])).unwrap() {
            Invocation::Fill(f) => assert_eq!(f.opacity, 1.0),
            _ => panic!("应当是 fill"),
        }
        match parse(&args(&["fill", "D:/a.pdf", "--opacity", "-3"])).unwrap() {
            Invocation::Fill(f) => assert_eq!(f.opacity, 0.0),
            _ => panic!("应当是 fill"),
        }
    }
}