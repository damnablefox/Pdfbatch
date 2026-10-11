//! 命令行参数解析与帮助文本。

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use pdfbatch_core::batch::{Naming, OutputMode};
use pdfbatch_core::color::parse_color;
use pdfbatch_core::{Blend, FillChange, FillOp, FillStyle, Layer, Matcher, RectMatch};

/// 需要取值的开关；其余 `--xxx` 一律当作布尔开关。
const VALUE_FLAGS: [&str; 24] = [
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
    "match-size",
    "match-rect",
    "match-color",
    "match-opacity",
    "match-blend",
    "match-layer",
    "tolerance",
    "set-rect",
    "set-color",
    "set-opacity",
    "set-blend",
    "set-layer",
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
    /// 增 / 删 / 改三种操作之一。
    pub op: FillOp,
    /// 页范围原文，处理时按各文档的实际页数解析。
    pub pages: String,
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
    let erase = raw.has("erase");
    let modify = raw.has("modify");
    if erase && modify {
        return Err("--erase 与 --modify 不能同时使用".to_string());
    }
    let pages = raw.value("pages").unwrap_or("all").to_string();
    let op = if erase {
        reject_add_flags(&raw, "--erase")?;
        FillOp::Remove(parse_matcher(&raw)?)
    } else if modify {
        reject_add_flags(&raw, "--modify")?;
        let change = parse_change(&raw)?;
        if change.is_empty() {
            return Err("--modify 需要至少一个 --set-* 指明要改成什么".to_string());
        }
        FillOp::Modify {
            matcher: parse_matcher(&raw)?,
            change,
        }
    } else {
        reject_match_flags(&raw, "新增填充层")?;
        FillOp::Add {
            style: parse_style(&raw)?,
            replace: raw.has("replace"),
        }
    };
    Ok(Fill { common, op, pages })
}

/// 新增填充层的样式参数。
fn parse_style(raw: &RawArgs) -> Result<FillStyle, String> {
    if raw.has("full-page") && raw.value("rect").is_some() {
        return Err("--rect 与 --full-page 不能同时使用".to_string());
    }
    let rect = match raw.value("rect") {
        Some(text) => Some(parse_numbers::<4>(text, "--rect")?),
        None => None,
    };
    let color = match raw.value("color") {
        Some(text) => parse_color(text)?,
        None => [1.0, 0.92, 0.55], // 默认淡黄
    };
    let opacity = match raw.value("opacity") {
        Some(text) => parse_unit(text, "--opacity")?,
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
    Ok(FillStyle {
        rect,
        color,
        opacity,
        blend,
        layer,
    })
}

/// 查找匹配条件（`--erase` 与 `--modify` 共用）。
fn parse_matcher(raw: &RawArgs) -> Result<Matcher, String> {
    let rect = parse_rect_match(raw)?;
    let color = match raw.value("match-color") {
        Some(text) => Some(parse_color(text)?),
        None => None,
    };
    let opacity = match raw.value("match-opacity") {
        Some(text) => Some(parse_unit(text, "--match-opacity")?),
        None => None,
    };
    let blend = match raw.value("match-blend") {
        Some(text) => Some(Blend::parse(text)?),
        None => None,
    };
    let layer = match raw.value("match-layer") {
        Some(text) => Some(Layer::parse(text)?),
        None => None,
    };
    let tolerance = match raw.value("tolerance") {
        Some(text) => {
            let value: f64 = text
                .trim()
                .parse()
                .map_err(|_| format!("--tolerance 的值 `{text}` 不是数字"))?;
            if !value.is_finite() || value < 0.0 {
                return Err("--tolerance 必须是非负有限数".to_string());
            }
            value
        }
        None => 0.0,
    };
    Ok(Matcher {
        rect,
        color,
        opacity,
        blend,
        layer,
        tolerance,
    })
}

/// 区域的三种匹配粒度，三者互斥。
fn parse_rect_match(raw: &RawArgs) -> Result<Option<RectMatch>, String> {
    let used: Vec<&str> = [
        ("--match-full-page", raw.has("match-full-page")),
        ("--match-size", raw.value("match-size").is_some()),
        ("--match-rect", raw.value("match-rect").is_some()),
    ]
    .into_iter()
    .filter_map(|(name, on)| on.then_some(name))
    .collect();
    if used.len() > 1 {
        return Err(format!("{} 只能用一个", used.join(" / ")));
    }
    if raw.has("match-full-page") {
        return Ok(Some(RectMatch::FullPage));
    }
    if let Some(text) = raw.value("match-size") {
        return Ok(Some(RectMatch::Size(parse_numbers::<2>(text, "--match-size")?)));
    }
    if let Some(text) = raw.value("match-rect") {
        return Ok(Some(RectMatch::Rect(parse_numbers::<4>(text, "--match-rect")?)));
    }
    Ok(None)
}

/// 修改目标（`--modify` 专用）。
fn parse_change(raw: &RawArgs) -> Result<FillChange, String> {
    if raw.has("set-full-page") && raw.value("set-rect").is_some() {
        return Err("--set-full-page 与 --set-rect 不能同时使用".to_string());
    }
    let rect = match raw.value("set-rect") {
        Some(text) => Some(parse_numbers::<4>(text, "--set-rect")?),
        None => None,
    };
    let full_page = raw.has("set-full-page").then_some(true);
    let color = match raw.value("set-color") {
        Some(text) => Some(parse_color(text)?),
        None => None,
    };
    let opacity = match raw.value("set-opacity") {
        Some(text) => Some(parse_unit(text, "--set-opacity")?),
        None => None,
    };
    let blend = match raw.value("set-blend") {
        Some(text) => Some(Blend::parse(text)?),
        None => None,
    };
    let layer = match raw.value("set-layer") {
        Some(text) => Some(Layer::parse(text)?),
        None => None,
    };
    Ok(FillChange {
        rect,
        full_page,
        color,
        opacity,
        blend,
        layer,
    })
}

/// 删/改模式下，新增专用开关没有意义，出现即报错。
fn reject_add_flags(raw: &RawArgs, mode: &str) -> Result<(), String> {
    const ADD: [&str; 7] = ["rect", "full-page", "color", "opacity", "blend", "layer", "replace"];
    for key in ADD {
        if raw.has(key) || raw.value(key).is_some() {
            return Err(format!("{mode} 模式不能用 `--{key}`（它只属于新增填充层）"));
        }
    }
    Ok(())
}

/// 新增模式下，匹配与修改开关没有意义，出现即报错。
fn reject_match_flags(raw: &RawArgs, mode: &str) -> Result<(), String> {
    const MATCH: [&str; 14] = [
        "match-full-page",
        "match-size",
        "match-rect",
        "match-color",
        "match-opacity",
        "match-blend",
        "match-layer",
        "tolerance",
        "set-full-page",
        "set-rect",
        "set-color",
        "set-opacity",
        "set-blend",
        "set-layer",
    ];
    for key in MATCH {
        if raw.has(key) || raw.value(key).is_some() {
            return Err(format!("{mode} 时不能用 `--{key}`；它只在 --erase / --modify 下有效"));
        }
    }
    Ok(())
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

/// 把 `a,b,c,…` 解析成 N 个数字。
fn parse_numbers<const N: usize>(text: &str, flag: &str) -> Result<[f64; N], String> {
    let parts: Vec<&str> = text.split(',').map(str::trim).collect();
    if parts.len() != N {
        return Err(format!(
            "{flag} 需要 {N} 个用逗号分开的数（实际给了 {} 个）",
            parts.len()
        ));
    }
    let mut out = [0.0_f64; N];
    for (index, part) in parts.iter().enumerate() {
        out[index] = part
            .parse()
            .map_err(|_| format!("{flag} 的第 {} 个值 `{part}` 不是数字", index + 1))?;
    }
    Ok(out)
}

/// 解析 0..1 的小数（透明度），越界会被夹到范围内。
fn parse_unit(text: &str, flag: &str) -> Result<f64, String> {
    let value: f64 = text
        .trim()
        .parse()
        .map_err(|_| format!("{flag} 的值 `{text}` 不是数字"))?;
    if !value.is_finite() {
        return Err(format!("{flag} 的值必须是有限数"));
    }
    Ok(value.clamp(0.0, 1.0))
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
        r##"pdfbatch —— 给 PDF 批量加书签、批量增删改纯色填充层

用法：
  pdfbatch bookmarks <输入.pdf|目录>… --spec <书签文本> [选项]
  pdfbatch fill      <输入.pdf|目录>… [--rect x,y,宽,高 | --full-page] [选项]
  pdfbatch fill      <输入.pdf|目录>… --erase  [匹配条件] [选项]
  pdfbatch fill      <输入.pdf|目录>… --modify [匹配条件] [--set-* …] [选项]

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

fill 专用 —— 三种操作，默认「新增」：

  新增（把纯色填充层盖到指定页）：
  --rect x,y,宽,高    填充矩形（PDF 用户空间坐标，单位点，原点在页面左下角）
  --full-page         整页填充（默认，等价于用页面 CropBox）
  --color <颜色>      #RRGGBB / R,G,B（0–255）/ R,G,B（0.0–1.0），默认 #FFE08C
  --opacity <0..1>    不透明度，默认 0.2
  --blend <模式>      混合模式：{}（默认 Normal）
  --layer <层级>      top = 盖在原有内容之上（默认）；bottom = 垫在原有内容之下
  --replace           先摘掉本工具此前加在该页的填充层（用来批量改混合模式/层级）

  删除（先按匹配条件查找，命中数非零才删；不给条件表示本工具加的全部层）：
  --erase             删除匹配的填充层

  修改（同样先查找，再按 --set-* 改写命中的层）：
  --modify            修改匹配的填充层，至少给一个 --set-*

  匹配条件（--erase 与 --modify 共用，都不给表示匹配全部层）：
  --match-full-page         只匹配整页填充
  --match-size w,h          只比宽高（不看左下起点）
  --match-rect x,y,宽,高     完整匹配左下起点与宽高
  --match-color <颜色>       匹配颜色（写法同 --color）
  --match-opacity <0..1>    匹配不透明度
  --match-blend <模式>       匹配混合模式
  --match-layer <层级>       匹配层级（top / bottom）
  --tolerance <数值>         数值比较容差，默认 0（精确相等）

  修改目标（--modify 专用，至少给一个）：
  --set-rect x,y,宽,高    改成这个矩形
  --set-full-page         改成整页
  --set-color <颜色>      改成这个颜色
  --set-opacity <0..1>    改成这个不透明度
  --set-blend <模式>      改成这个混合模式
  --set-layer <层级>      改成这个层级

  --pages <范围>      要处理的页，例如 1-5,8,10- ；默认 all
                      也支持相对写法：all except first,last、-1（末页）、2..-2

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
  pdfbatch fill "D:\pdfs" --full-page --color "#FFF3B0" --opacity 0.25 --blend Multiply
  pdfbatch fill "D:\a.pdf" --rect "72,72,400,120" --layer bottom --pages "1-5,8" --in-place
  pdfbatch fill "D:\pdfs" --erase --match-color "255,224,140" --pages "all except first,last"
  pdfbatch fill "D:\pdfs" --modify --match-blend Multiply --set-opacity 0.5 --set-layer bottom
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

    /// 取新增操作的样式与替换标志。
    fn add_parts(op: &FillOp) -> (&FillStyle, bool) {
        match op {
            FillOp::Add { style, replace } => (style, *replace),
            other => panic!("应当是新增：{other:?}"),
        }
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
                let (style, replace) = add_parts(&f.op);
                assert_eq!(style.rect, Some([10.0, 20.0, 100.0, 50.0]));
                assert_eq!(style.color[2], 1.0);
                assert_eq!(style.opacity, 0.35);
                assert_eq!(style.blend, Blend::Multiply);
                assert_eq!(style.layer, Layer::Bottom);
                assert!(!replace);
                assert_eq!(f.pages, "1-5");
            }
            _ => panic!("应当是 fill"),
        }
    }

    #[test]
    fn defaults_are_sane() {
        match parse(&args(&["fill", "D:/a.pdf"])).unwrap() {
            Invocation::Fill(f) => {
                let (style, replace) = add_parts(&f.op);
                assert_eq!(style.rect, None, "默认整页");
                assert_eq!(style.blend, Blend::Normal);
                assert_eq!(style.layer, Layer::Top);
                assert!(!replace);
                assert_eq!(f.pages, "all");
                assert!(style.opacity > 0.0 && style.opacity <= 1.0);
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
    fn erase_collects_the_match_conditions() {
        let parsed = parse(&args(&[
            "fill", "D:/a.pdf", "--erase", "--match-rect", "10,20,100,50", "--match-color", "255,224,140",
            "--match-opacity", "0.2", "--match-blend", "multiply", "--match-layer", "top", "--tolerance", "0.01",
        ]))
        .unwrap();
        match parsed {
            Invocation::Fill(f) => match &f.op {
                FillOp::Remove(m) => {
                    assert_eq!(m.rect, Some(RectMatch::Rect([10.0, 20.0, 100.0, 50.0])));
                    assert!(m.color.is_some());
                    assert_eq!(m.opacity, Some(0.2));
                    assert_eq!(m.blend, Some(Blend::Multiply));
                    assert_eq!(m.layer, Some(Layer::Top));
                    assert_eq!(m.tolerance, 0.01);
                }
                other => panic!("应当是删除：{other:?}"),
            },
            _ => panic!("应当是 fill"),
        }
        // 整页与按宽高两种粒度。
        match parse(&args(&["fill", "D:/a.pdf", "--erase", "--match-full-page"])).unwrap() {
            Invocation::Fill(f) => assert!(matches!(f.op, FillOp::Remove(Matcher { rect: Some(RectMatch::FullPage), .. }))),
            _ => panic!("应当是 fill"),
        }
        match parse(&args(&["fill", "D:/a.pdf", "--erase", "--match-size", "100,50"])).unwrap() {
            Invocation::Fill(f) => {
                assert!(matches!(f.op, FillOp::Remove(Matcher { rect: Some(RectMatch::Size([100.0, 50.0])), .. })))
            }
            _ => panic!("应当是 fill"),
        }
        // 不给条件也算合法（等价于匹配本工具加的全部层）。
        match parse(&args(&["fill", "D:/a.pdf", "--erase"])).unwrap() {
            Invocation::Fill(f) => assert!(matches!(f.op, FillOp::Remove(m) if m.is_empty())),
            _ => panic!("应当是 fill"),
        }
    }

    #[test]
    fn modify_collects_match_and_change() {
        let parsed = parse(&args(&[
            "fill", "D:/a.pdf", "--modify", "--match-color", "#FFE08C", "--set-color", "0.0,1.0,0.0", "--set-opacity",
            "0.5", "--set-blend", "screen", "--set-layer", "bottom",
        ]))
        .unwrap();
        match parsed {
            Invocation::Fill(f) => match &f.op {
                FillOp::Modify { matcher, change } => {
                    assert!(matcher.color.is_some());
                    assert!(change.color.is_some());
                    assert_eq!(change.opacity, Some(0.5));
                    assert_eq!(change.blend, Some(Blend::Screen));
                    assert_eq!(change.layer, Some(Layer::Bottom));
                }
                other => panic!("应当是修改：{other:?}"),
            },
            _ => panic!("应当是 fill"),
        }
        // 改区域：整页 vs 具体矩形。
        match parse(&args(&["fill", "D:/a.pdf", "--modify", "--set-full-page"])).unwrap() {
            Invocation::Fill(f) => {
                assert!(matches!(f.op, FillOp::Modify { change: FillChange { full_page: Some(true), .. }, .. }))
            }
            _ => panic!("应当是 fill"),
        }
        match parse(&args(&["fill", "D:/a.pdf", "--modify", "--set-rect", "1,2,3,4"])).unwrap() {
            Invocation::Fill(f) => assert!(matches!(
                f.op,
                FillOp::Modify { change: FillChange { rect: Some([1.0, 2.0, 3.0, 4.0]), .. }, .. }
            )),
            _ => panic!("应当是 fill"),
        }
    }

    #[test]
    fn mode_and_flag_conflicts_are_rejected() {
        // 删与改互斥。
        assert!(parse(&args(&["fill", "D:/a.pdf", "--erase", "--modify", "--set-color", "#000"])).is_err());
        // 删/改不能带新增专用开关。
        assert!(parse(&args(&["fill", "D:/a.pdf", "--erase", "--color", "#000"])).is_err());
        assert!(parse(&args(&["fill", "D:/a.pdf", "--modify", "--set-color", "#000", "--replace"])).is_err());
        // 新增不能带匹配/修改开关。
        assert!(parse(&args(&["fill", "D:/a.pdf", "--match-color", "#000"])).is_err());
        assert!(parse(&args(&["fill", "D:/a.pdf", "--set-color", "#000"])).is_err());
        // 修改必须给至少一个 --set-*。
        assert!(parse(&args(&["fill", "D:/a.pdf", "--modify"])).is_err());
        assert!(parse(&args(&["fill", "D:/a.pdf", "--modify", "--match-full-page"])).is_err());
        // 三种区域匹配粒度互斥。
        assert!(parse(&args(&["fill", "D:/a.pdf", "--erase", "--match-full-page", "--match-size", "1,2"])).is_err());
        assert!(parse(&args(&["fill", "D:/a.pdf", "--modify", "--set-rect", "1,2,3,4", "--set-full-page"])).is_err());
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
                let (style, _) = add_parts(&f.op);
                assert_eq!(style.color, [1.0, 1.0, 1.0]);
                assert_eq!(style.opacity, 1.0);
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
            Invocation::Fill(f) => assert_eq!(add_parts(&f.op).0.opacity, 1.0),
            _ => panic!("应当是 fill"),
        }
        match parse(&args(&["fill", "D:/a.pdf", "--opacity", "-3"])).unwrap() {
            Invocation::Fill(f) => assert_eq!(add_parts(&f.op).0.opacity, 0.0),
            _ => panic!("应当是 fill"),
        }
    }
}