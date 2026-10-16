//! 把书签规格写进 PDF。

use pdfcraft_cos::Document;
use pdfcraft_organize::{
    add_bookmark, delete_bookmark, set_bookmark_open, top_level_bookmarks,
};

use crate::error::Result;
use crate::spec::SpecEntry;

/// 把解析好的书签树写进文档，返回添加的书签条数（含子书签）。
///
/// `replace` 为真时，先删除文档里原有的全部顶层书签；否则追加到已有书签之后。
pub fn apply(doc: &mut Document, entries: &[SpecEntry], replace: bool) -> Result<usize> {
    if replace {
        clear_all(doc)?;
    }
    // 一律追加到末尾（`add_bookmark` 把越界索引当作追加）：替换时前面已经清空，
    // 追加的结果就正好是按文本顺序排列；不替换时则接在已有书签之后。
    let mut count = 0;
    for entry in entries {
        count += insert(doc, &[], usize::MAX, entry)?;
    }
    Ok(count)
}

/// 递归插入一条书签及其子书签，返回插入的条数。
///
/// 父子位置只相信 `add_bookmark` 返回的路径，绝不自己推算索引——逐个插入会让手算的索引失效。
fn insert(doc: &mut Document, parent_path: &[usize], index: usize, entry: &SpecEntry) -> Result<usize> {
    let path = add_bookmark(doc, parent_path, index, &entry.title, entry.page)?;
    let mut count = 1;
    for (child_index, child) in entry.children.iter().enumerate() {
        count += insert(doc, &path, child_index, child)?;
    }
    if !entry.children.is_empty() {
        // 有子书签的条目默认展开，读者一打开就能看到层级。
        set_bookmark_open(doc, &path, true)?;
    }
    Ok(count)
}

/// 删除全部顶层书签（连带各自的子书签）。
fn clear_all(doc: &mut Document) -> Result<()> {
    // 用 `top_level_bookmarks` 而不是 `bookmarks`：后者的新版语义是「列出全部层级」，
    // 拿它当顶层计数会多删（每删掉一个顶层项就可能连带把计数算进去的子项也数了一遍）。
    let top_level = top_level_bookmarks(doc).len();
    for _ in 0..top_level {
        delete_bookmark(doc, &[0])?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    // `bookmarks` 只被测试用到（生产代码改用 `top_level_bookmarks`），所以在这里显式引入。
    use pdfcraft_organize::bookmarks;
    use crate::spec::parse_spec;

    /// 建一个最小可用的 PDF 骨架：一个 /Pages 节点和两张空页。
    ///
    /// 用 `cos` 的对象 API 手工搭出文档树，测试就不依赖真实样张文件。
    fn blank_document() -> Document {
        use pdfcraft_cos::{Dict, Object};
        let mut doc = Document::new_empty();
        let pages_ref = doc.add(Dict::new());
        let page1 = doc.add(Dict::new());
        let page2 = doc.add(Dict::new());
        let root = doc.root().expect("new_empty 应当有根节点");
        doc.update_dict(root, |d| {
            d.set(b"Type".to_vec(), Object::name("Catalog"));
            d.set(b"Pages".to_vec(), Object::Ref(pages_ref));
        })
        .expect("写根字典");
        doc.update_dict(pages_ref, |d| {
            d.set(b"Type".to_vec(), Object::name("Pages"));
            d.set(b"Kids".to_vec(), Object::Array(vec![Object::Ref(page1), Object::Ref(page2)]));
            d.set(b"Count".to_vec(), Object::Int(2));
        })
        .expect("写 Pages 字典");
        for page in [page1, page2] {
            doc.update_dict(page, |d| {
                d.set(b"Type".to_vec(), Object::name("Page"));
                d.set(b"Parent".to_vec(), Object::Ref(pages_ref));
                d.set(
                    b"MediaBox".to_vec(),
                    Object::Array(vec![Object::Int(0), Object::Int(0), Object::Int(200), Object::Int(300)]),
                );
            })
            .expect("写 Page 字典");
        }
        doc
    }

    #[test]
    fn entries_become_a_nested_outline() {
        let mut doc = blank_document();
        let spec = "第一章\t1\n  1.1\t2\n第二章\t2\n";
        let parsed = parse_spec(spec, false, true, 2).unwrap();
        let added = apply(&mut doc, &parsed.entries, true).unwrap();
        assert_eq!(added, 3);

        // 书签现在是扁平列表：`top_level_bookmarks` 只给顶层，其余项带 `path` 表示层级。
        let tree = top_level_bookmarks(&doc);
        assert_eq!(tree.len(), 2);
        assert_eq!(tree[0].title, "第一章");
        let all = bookmarks(&doc);
        let child = all
            .iter()
            .find(|b| b.path.as_slice() == [0, 0])
            .expect("第一章应当有一个子书签");
        assert_eq!(child.title, "1.1");
        assert!(tree[0].open, "有子书签的条目应当展开");
        assert_eq!(tree[1].title, "第二章");
    }

    #[test]
    fn replace_clears_the_previous_outline() {
        let mut doc = blank_document();
        let first = parse_spec("旧的\t1\n", false, true, 2).unwrap();
        apply(&mut doc, &first.entries, true).unwrap();
        assert_eq!(bookmarks(&doc).len(), 1);

        let second = parse_spec("新的\t2\n", false, true, 2).unwrap();
        apply(&mut doc, &second.entries, true).unwrap();
        let tree = bookmarks(&doc);
        assert_eq!(tree.len(), 1);
        assert_eq!(tree[0].title, "新的");
    }

    #[test]
    fn without_replace_new_entries_are_appended() {
        let mut doc = blank_document();
        let first = parse_spec("已有的\t1\n", false, true, 2).unwrap();
        apply(&mut doc, &first.entries, false).unwrap();
        let second = parse_spec("追加的\t2\n", false, true, 2).unwrap();
        apply(&mut doc, &second.entries, false).unwrap();

        let tree = bookmarks(&doc);
        let titles: Vec<&str> = tree.iter().map(|b| b.title.as_str()).collect();
        assert_eq!(titles, ["已有的", "追加的"]);
    }
}