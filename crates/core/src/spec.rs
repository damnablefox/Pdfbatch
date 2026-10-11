//! 书签文本规格解析。
//!
//! 格式（默认）：
//!
//! ```text
//! # 以 # 开头是注释；空行忽略
//! 第一章 绪论<TAB>1
//!   1.1 研究背景<TAB>2
//!   1.2 研究意义 | 3
//! 第二章 方法<TAB>10
//! ```
//!
//! - 缩进（一个 Tab，或每两个空格）表示层级；
//! - 标题与页码之间用**最后一个** Tab 或 ` | ` 分隔；
//! - 页码默认为 1 基（用户看到的页码），`zero_based` 为真时按 0 基解释。

use std::path::Path;

use crate::batch::Input;

/// 一条书签规格（可带子书签）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpecEntry {
    /// 书签标题。
    pub title: String,
    /// 目标页，0 基。
    pub page: usize,
    /// 子书签。
    pub children: Vec<SpecEntry>,
}

/// 书签规格的解析错误，带源文件行号。
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum SpecError {
    #[error("第 {line} 行：{message}")]
    Line { line: usize, message: String },
}

/// 解析结果：书签树 + 非致命警告（例如缩进跳级已被自动修正）。
#[derive(Debug)]
pub struct ParsedSpec {
    pub entries: Vec<SpecEntry>,
    pub warnings: Vec<String>,
}

/// 文件段头指向的目标。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SectionTarget {
    /// 完整文件名（含扩展名）；匹配时大小写不敏感、必须完全相等。
    Name(String),
    /// 文件在列表中的序号（从 1 开始）。
    Index(usize),
}

/// 文本里的一段书签：段头指定它属于哪个文件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpecSection {
    pub target: SectionTarget,
    /// 段头所在行号（1 基，报错用）。
    pub line: usize,
    /// 该段的原文（不含段头行）。
    pub text: String,
}

/// 整份文本的切分结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpecSections {
    /// 没有段头：整份文本应用到所有文件（向后兼容）。
    Shared(String),
    /// 有段头：每段绑定一个文件。
    Sections(Vec<SpecSection>),
}

/// 分段绑定到文件列表后的结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BookmarkBinding {
    /// 与输入列表一一对应：每个文件的段原文；`None` 表示没有段覆盖（跳过）。
    pub per_file: Vec<Option<String>>,
    /// 没有段覆盖的文件下标。
    pub uncovered: Vec<usize>,
}

/// 按文件段头切分文本。没有段头时返回 [`SpecSections::Shared`]。
///
/// 段头形如 `# 文件：report.pdf`（中英文冒号都认）或 `# 文件：#2`。
/// 段头之前不允许出现书签行，否则报错——避免书签归属不明。
pub fn split_sections(text: &str) -> Result<SpecSections, SpecError> {
    // 整份文本没有任何段头：当作公共段，应用到所有文件（与旧行为一致）。
    if !text.lines().any(|line| parse_section_header(line).is_some()) {
        return Ok(SpecSections::Shared(text.to_string()));
    }

    let mut sections: Vec<SpecSection> = Vec::new();
    let mut current: Option<(SectionTarget, usize, Vec<&str>)> = None;

    for (index, raw) in text.lines().enumerate() {
        let line_no = index + 1;
        if let Some(parsed) = parse_section_header(raw) {
            let target = parsed.map_err(|message| SpecError::Line { line: line_no, message })?;
            if let Some((target, line, lines)) = current.take() {
                sections.push(SpecSection { target, line, text: lines.join("\n") });
            }
            current = Some((target, line_no, Vec::new()));
            continue;
        }
        match &mut current {
            Some((_, _, lines)) => lines.push(raw),
            None => {
                if !is_blank_or_comment(raw) {
                    return Err(SpecError::Line {
                        line: line_no,
                        message: "这条书签没有归属：请在它前面先写文件段头（如 `# 文件：文件名.pdf`）".to_string(),
                    });
                }
            }
        }
    }
    if let Some((target, line, lines)) = current.take() {
        sections.push(SpecSection { target, line, text: lines.join("\n") });
    }

    if sections.is_empty() {
        Ok(SpecSections::Shared(text.to_string()))
    } else {
        Ok(SpecSections::Sections(sections))
    }
}

/// 把分段绑定到文件列表。
///
/// 文件名按「完整文件名（含扩展名）、大小写不敏感」精确匹配，不做省略后缀或模糊匹配；
/// 序号按列表顺序（从 1 开始）。同一文件出现多段时按出现顺序合并。
pub fn bind_sections(sections: &[SpecSection], inputs: &[Input]) -> Result<BookmarkBinding, String> {
    let names: Vec<String> = inputs.iter().map(|input| file_name_of(&input.path)).collect();
    let mut per_file: Vec<Option<String>> = vec![None; inputs.len()];

    for section in sections {
        let index = match &section.target {
            SectionTarget::Index(number) => {
                let index = number - 1;
                if index >= inputs.len() {
                    return Err(format!(
                        "第 {} 行：序号 #{} 超出文件数量（当前共 {} 个文件）",
                        section.line,
                        number,
                        inputs.len()
                    ));
                }
                index
            }
            SectionTarget::Name(name) => {
                let matched: Vec<usize> = names
                    .iter()
                    .enumerate()
                    .filter(|(_, candidate)| candidate.eq_ignore_ascii_case(name))
                    .map(|(index, _)| index)
                    .collect();
                match matched.as_slice() {
                    [] => {
                        return Err(format!(
                            "第 {} 行：找不到文件「{}」（段头要写完整文件名，含扩展名）",
                            section.line, name
                        ));
                    }
                    [only] => *only,
                    _ => {
                        return Err(format!(
                            "第 {} 行：「{}」匹配到多个文件，请改用序号（`# 文件：#序号`）",
                            section.line, name
                        ));
                    }
                }
            }
        };
        match &mut per_file[index] {
            Some(existing) => {
                existing.push('\n');
                existing.push_str(&section.text);
            }
            slot @ None => *slot = Some(section.text.clone()),
        }
    }

    let uncovered = per_file
        .iter()
        .enumerate()
        .filter(|(_, text)| text.is_none())
        .map(|(index, _)| index)
        .collect();
    Ok(BookmarkBinding { per_file, uncovered })
}

/// 识别段头行：返回 `None` 表示不是段头；`Some(Err)` 表示是段头但写法有问题。
fn parse_section_header(line: &str) -> Option<Result<SectionTarget, String>> {
    let rest = line.trim_start().strip_prefix('#')?.trim_start();
    let rest = rest.strip_prefix("文件")?;
    let rest = rest.strip_prefix(['：', ':'])?;
    let target = rest.trim();
    if target.is_empty() {
        return Some(Err("段头 `# 文件：` 后面要写文件名（含扩展名）或序号 `#2`".to_string()));
    }
    if let Some(number) = target.strip_prefix('#') {
        return Some(match number.trim().parse::<usize>() {
            Ok(value) if value >= 1 => Ok(SectionTarget::Index(value)),
            _ => Err(format!("序号 `{target}` 无效，应写成 `# 文件：#2`（从 1 开始）")),
        });
    }
    Some(Ok(SectionTarget::Name(target.to_string())))
}

/// 空行或注释行（`#` 开头）。
fn is_blank_or_comment(line: &str) -> bool {
    let trimmed = line.trim();
    trimmed.is_empty() || trimmed.starts_with('#')
}

fn file_name_of(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// 解析书签规格。`page_count` 用于校验页码是否越界。
pub fn parse_spec(text: &str, zero_based: bool, strict: bool, page_count: usize) -> Result<ParsedSpec, SpecError> {
    let mut roots: Vec<SpecEntry> = Vec::new();
    // 还没确定"不再接收子书签"的条目：层级 + 内容。
    let mut stack: Vec<(usize, SpecEntry)> = Vec::new();
    let mut warnings: Vec<String> = Vec::new();

    for (index, raw) in text.lines().enumerate() {
        let line_no = index + 1;
        let line = raw.trim_end();
        if line.trim().is_empty() || line.trim_start().starts_with('#') {
            continue;
        }

        let indent = indent_level(line);
        let body = line.trim_start();
        let (title, page_text) = split_title_page(body).ok_or_else(|| SpecError::Line {
            line: line_no,
            message: "找不到页码：请用 Tab 或 ` | ` 分隔标题与页码（如 `第一章 绪论 | 1`）".to_string(),
        })?;
        let title = title.trim().to_string();
        if title.is_empty() {
            return Err(SpecError::Line {
                line: line_no,
                message: "标题为空".to_string(),
            });
        }
        let page: usize = page_text.trim().parse().map_err(|_| SpecError::Line {
            line: line_no,
            message: format!("页码 `{}` 不是整数", page_text.trim()),
        })?;
        let target = if zero_based {
            page
        } else {
            if page == 0 {
                return Err(SpecError::Line {
                    line: line_no,
                    message: "页码从 1 开始".to_string(),
                });
            }
            page - 1
        };
        if target >= page_count {
            return Err(SpecError::Line {
                line: line_no,
                message: format!("页码 {page} 超出文档页数（共 {page_count} 页）"),
            });
        }

        // 层级不能越级：顶层必须是第一级，子级最多比父级深一级。
        let mut level = indent;
        match stack.last() {
            Some((parent_level, _)) if level > parent_level + 1 => {
                if strict {
                    return Err(SpecError::Line {
                        line: line_no,
                        message: format!("缩进跳级：上一行是第 {parent_level} 级，本行是第 {level} 级"),
                    });
                }
                warnings.push(format!("第 {line_no} 行：缩进跳级，已按上一行 +1 级处理"));
                level = parent_level + 1;
            }
            None if level > 0 => {
                if strict {
                    return Err(SpecError::Line {
                        line: line_no,
                        message: format!("第一个书签有缩进（第 {level} 级），它必须是顶层"),
                    });
                }
                warnings.push(format!("第 {line_no} 行：第一条书签有缩进，已按顶层处理"));
                level = 0;
            }
            _ => {}
        }

        let entry = SpecEntry {
            title,
            page: target,
            children: Vec::new(),
        };

        // 弹出所有"层级不低于本行"的条目：它们的子书签已经收完。
        while let Some((top_level, _)) = stack.last() {
            if *top_level >= level {
                let (_, done) = stack.pop().expect("上面刚判断过非空");
                attach(done, &mut stack, &mut roots);
            } else {
                break;
            }
        }
        stack.push((level, entry));
    }

    while let Some((_, done)) = stack.pop() {
        attach(done, &mut stack, &mut roots);
    }

    Ok(ParsedSpec {
        entries: roots,
        warnings,
    })
}

/// 把一个解析完的条目挂到栈顶条目的子节点，或（栈空时）挂到根上。
fn attach(entry: SpecEntry, stack: &mut [(usize, SpecEntry)], roots: &mut Vec<SpecEntry>) {
    match stack.last_mut() {
        Some((_, parent)) => parent.children.push(entry),
        None => roots.push(entry),
    }
}

/// 前导缩进的层级：一个 Tab 记一级，每两个空格记一级（两者可混用）。
fn indent_level(line: &str) -> usize {
    let mut level = 0;
    let mut spaces = 0;
    for c in line.chars() {
        match c {
            '\t' => {
                level += 1;
                spaces = 0;
            }
            ' ' => {
                spaces += 1;
                if spaces == 2 {
                    level += 1;
                    spaces = 0;
                }
            }
            _ => break,
        }
    }
    level
}

/// 把一行正文拆成（标题, 页码文本）：优先最后一个 Tab，其次最后一个 ` | `，
/// 最后回退到"以空白分隔、末段是纯数字"的写法。
fn split_title_page(body: &str) -> Option<(String, String)> {
    if let Some(i) = body.rfind('\t') {
        return Some((body[..i].to_string(), body[i + 1..].to_string()));
    }
    if let Some(i) = body.rfind(" | ") {
        return Some((body[..i].to_string(), body[i + " | ".len()..].to_string()));
    }
    let trimmed = body.trim_end();
    let cut = trimmed.rfind(char::is_whitespace)?;
    let tail = trimmed[cut + 1..].trim();
    (!tail.is_empty() && tail.chars().all(|c| c.is_ascii_digit()))
        .then(|| (trimmed[..cut].to_string(), tail.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn titles(entries: &[SpecEntry]) -> Vec<&str> {
        entries.iter().map(|e| e.title.as_str()).collect()
    }

    #[test]
    fn indentation_builds_a_tree() {
        let text = "第一章 绪论\t1\n  1.1 背景\t2\n  1.2 意义\t3\n第二章 方法\t10\n  2.1 数据\t11\n";
        let parsed = parse_spec(text, false, true, 20).unwrap();
        assert!(parsed.warnings.is_empty());
        assert_eq!(titles(&parsed.entries), ["第一章 绪论", "第二章 方法"]);
        assert_eq!(titles(&parsed.entries[0].children), ["1.1 背景", "1.2 意义"]);
        assert_eq!(titles(&parsed.entries[1].children), ["2.1 数据"]);
        assert_eq!(parsed.entries[0].page, 0);
        assert_eq!(parsed.entries[0].children[1].page, 2);
        assert_eq!(parsed.entries[1].children[0].page, 10);
    }

    #[test]
    fn tab_and_pipe_separators_both_work() {
        let parsed = parse_spec("甲 | 1\n乙\t2\n", false, true, 5).unwrap();
        assert_eq!(parsed.entries[0].title, "甲");
        assert_eq!(parsed.entries[0].page, 0);
        assert_eq!(parsed.entries[1].title, "乙");
        assert_eq!(parsed.entries[1].page, 1);
    }

    #[test]
    fn the_last_separator_wins() {
        // 标题里可以自由出现 ` | `。
        let parsed = parse_spec("A | B | 4\n", false, true, 5).unwrap();
        assert_eq!(parsed.entries[0].title, "A | B");
        assert_eq!(parsed.entries[0].page, 3);
    }

    #[test]
    fn comments_and_blank_lines_are_skipped() {
        let parsed = parse_spec("# 注释\n\n   \n甲 | 1\n# 又一条注释\n乙 | 2\n", false, true, 5).unwrap();
        assert_eq!(titles(&parsed.entries), ["甲", "乙"]);
    }

    #[test]
    fn deep_nesting_with_tabs_and_spaces() {
        let text = "一\t1\n\t1.1\t2\n\t\t1.1.1\t3\n  1.2\t4\n";
        let parsed = parse_spec(text, false, true, 10).unwrap();
        let l1 = &parsed.entries[0];
        assert_eq!(titles(&l1.children), ["1.1", "1.2"]);
        assert_eq!(titles(&l1.children[0].children), ["1.1.1"]);
    }

    #[test]
    fn zero_based_pages_are_interpreted_as_given() {
        let parsed = parse_spec("甲 | 0\n", true, true, 5).unwrap();
        assert_eq!(parsed.entries[0].page, 0);
        // 0 基下 5 已经越界（合法范围 0..=4）。
        assert!(parse_spec("甲 | 5\n", true, true, 5).is_err());
    }

    #[test]
    fn errors_point_at_the_line() {
        let err = parse_spec("甲 | 1\n乙\n", false, true, 9).unwrap_err();
        match err {
            SpecError::Line { line, message } => {
                assert_eq!(line, 2);
                assert!(message.contains("找不到页码"), "{message}");
            }
        }
        let err = parse_spec("甲 | 99\n", false, true, 9).unwrap_err();
        match err {
            SpecError::Line { line, message } => {
                assert_eq!(line, 1);
                assert!(message.contains("超出文档页数"), "{message}");
            }
        }
        let err = parse_spec("甲 | 0\n", false, true, 9).unwrap_err();
        match err {
            SpecError::Line { line, message } => {
                assert_eq!(line, 1);
                assert!(message.contains("从 1 开始"), "{message}");
            }
        }
    }

    #[test]
    fn skipped_levels_are_clamped_or_rejected() {
        // 非 strict：钳制并给出警告。
        let parsed = parse_spec("甲 | 1\n\t\t\t乙 | 2\n", false, false, 5).unwrap();
        assert_eq!(titles(&parsed.entries[0].children), ["乙"]);
        assert_eq!(parsed.warnings.len(), 1);
        assert!(parsed.warnings[0].contains("跳级"), "{:?}", parsed.warnings);

        // strict：报错。
        assert!(parse_spec("甲 | 1\n\t\t\t乙 | 2\n", false, true, 5).is_err());
        // 首行有缩进也要报错。
        assert!(parse_spec("  甲 | 1\n", false, true, 5).is_err());
    }

    #[test]
    fn malformed_lines_are_rejected() {
        // 只有标题、没有页码。
        assert!(parse_spec("甲\t\n", false, true, 5).is_err());
        // 页码不是数字。
        assert!(parse_spec("甲 | 二\n", false, true, 5).is_err());
    }

    fn input(path: &str) -> Input {
        Input {
            path: std::path::PathBuf::from(path),
            relative: std::path::PathBuf::from(std::path::Path::new(path).file_name().unwrap()),
        }
    }

    fn sections_of(text: &str) -> Vec<SpecSection> {
        match split_sections(text).unwrap() {
            SpecSections::Sections(sections) => sections,
            other => panic!("应当是分段：{other:?}"),
        }
    }

    #[test]
    fn no_header_means_shared() {
        match split_sections("甲 | 1\n乙 | 2\n").unwrap() {
            SpecSections::Shared(text) => assert!(text.contains("甲")),
            other => panic!("应当是无段头的公共文本：{other:?}"),
        }
    }

    #[test]
    fn headers_split_the_text() {
        let sections = sections_of("# 文件：a.pdf\n甲 | 1\n# 文件：b.pdf\n乙 | 2\n");
        assert_eq!(sections.len(), 2);
        assert_eq!(sections[0].target, SectionTarget::Name("a.pdf".into()));
        assert_eq!(sections[0].text.trim(), "甲 | 1");
        assert_eq!(sections[1].target, SectionTarget::Name("b.pdf".into()));
        assert_eq!(sections[1].text.trim(), "乙 | 2");
    }

    #[test]
    fn a_bookmark_before_any_header_is_rejected() {
        match split_sections("甲 | 1\n# 文件：a.pdf\n乙 | 2\n").unwrap_err() {
            SpecError::Line { line, .. } => assert_eq!(line, 1),
        }
    }

    #[test]
    fn comments_before_the_first_header_are_fine() {
        let text = "# 说明\n\n# 文件：a.pdf\n甲 | 1\n";
        assert!(matches!(split_sections(text).unwrap(), SpecSections::Sections(_)));
    }

    #[test]
    fn index_header_is_parsed() {
        let sections = sections_of("# 文件：#2\n甲 | 1\n");
        assert_eq!(sections[0].target, SectionTarget::Index(2));
    }

    #[test]
    fn empty_header_target_is_rejected() {
        assert!(split_sections("# 文件：\n甲 | 1\n").is_err());
    }

    #[test]
    fn binding_matches_the_full_file_name() {
        let sections = sections_of("# 文件：a.pdf\n甲 | 1\n# 文件：b.pdf\n乙 | 2\n");
        let inputs = [input("D:/x/a.pdf"), input("D:/y/b.pdf")];
        let binding = bind_sections(&sections, &inputs).unwrap();
        assert_eq!(binding.per_file[0].as_deref().map(str::trim), Some("甲 | 1"));
        assert_eq!(binding.per_file[1].as_deref().map(str::trim), Some("乙 | 2"));
        assert!(binding.uncovered.is_empty());
    }

    #[test]
    fn binding_is_case_insensitive_but_needs_the_extension() {
        let inputs = [input("D:/x/Report.PDF")];
        // 大小写不同仍匹配。
        assert!(bind_sections(&sections_of("# 文件：report.pdf\n甲 | 1\n"), &inputs).is_ok());
        // 省掉扩展名应当匹配不到（避免误配）。
        assert!(bind_sections(&sections_of("# 文件：report\n甲 | 1\n"), &inputs).is_err());
    }

    #[test]
    fn binding_reports_unknown_and_ambiguous_targets() {
        let inputs = [input("D:/x/a.pdf"), input("D:/y/a.pdf")];
        assert!(bind_sections(&sections_of("# 文件：missing.pdf\n甲 | 1\n"), &inputs)
            .unwrap_err()
            .contains("找不到"));
        assert!(bind_sections(&sections_of("# 文件：a.pdf\n甲 | 1\n"), &inputs)
            .unwrap_err()
            .contains("多个"));
        assert!(bind_sections(&sections_of("# 文件：#5\n甲 | 1\n"), &inputs)
            .unwrap_err()
            .contains("超出"));
    }

    #[test]
    fn multiple_sections_for_one_file_are_merged() {
        let binding = bind_sections(&sections_of("# 文件：a.pdf\n甲 | 1\n# 文件：a.pdf\n乙 | 2\n"), &[input("D:/x/a.pdf")])
            .unwrap();
        let text = binding.per_file[0].as_deref().unwrap();
        assert!(text.contains("甲") && text.contains("乙"), "{text}");
    }

    #[test]
    fn uncovered_files_are_reported() {
        let binding = bind_sections(&sections_of("# 文件：a.pdf\n甲 | 1\n"), &[input("D:/x/a.pdf"), input("D:/y/b.pdf")])
            .unwrap();
        assert_eq!(binding.uncovered, vec![1]);
    }

    #[test]
    fn long_names_with_punctuation_match_exactly() {
        let name = "新编诸子集成. 潜夫论笺校正[M]. 〔汉〕王符 著，〔清〕汪继培 笺，彭铎 校正. 北京：中华书局,2014年04月第2版,2014年04月第5次印刷. ISBN=978-7-101-09949-2. 平装本_filled_CB818181_filled_CB818181_ME3EDCB.pdf";
        let sections = sections_of(&format!("# 文件：{name}\n甲 | 1\n"));
        assert!(bind_sections(&sections, &[input(&format!("D:/x/{name}"))]).is_ok());
    }
}