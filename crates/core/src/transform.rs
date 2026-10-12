//! 页面内容变换层与页面尺寸改写。
//!
//! 做法与填充层一致：往 `/Contents` 最前插一段 `q <矩阵> cm`、最后追加 `Q`，
//! 这样原有内容整体被这个矩阵映射，且图形状态被 `q`/`Q` 隔离。
//! 插入的流在字典里打 `/PBMark /PBScale` 标记，再次处理同一页时先把旧的摘掉，
//! 避免反复缩放**叠加**。

use pdfcraft_cos::{Document, Object, Stream};
use pdfcraft_model::Page;

use crate::content::{page_contents, set_page_contents, stream_tag, tagged};
use crate::error::{BatchError, Result};

/// 变换层的标记值。
const MARK_SCALE: &str = "PBScale";

/// 2D 仿射矩阵 `[a b c d e f]`，与 PDF 的 `cm` 一一对应。
///
/// 点变换为 `x' = a·x + c·y + e`、`y' = b·x + d·y + f`。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Matrix(pub [f64; 6]);

impl Matrix {
    pub const IDENTITY: Matrix = Matrix([1.0, 0.0, 0.0, 1.0, 0.0, 0.0]);

    pub fn translate(tx: f64, ty: f64) -> Self {
        Matrix([1.0, 0.0, 0.0, 1.0, tx, ty])
    }

    pub fn scale(sx: f64, sy: f64) -> Self {
        Matrix([sx, 0.0, 0.0, sy, 0.0, 0.0])
    }

    /// 先应用 `self`、再应用 `other`（即矩阵乘积 `other · self`）。
    pub fn then(self, other: Self) -> Self {
        let [a1, b1, c1, d1, e1, f1] = self.0;
        let [a2, b2, c2, d2, e2, f2] = other.0;
        Matrix([
            a2 * a1 + c2 * b1,
            b2 * a1 + d2 * b1,
            a2 * c1 + c2 * d1,
            b2 * c1 + d2 * d1,
            a2 * e1 + c2 * f1 + e2,
            b2 * e1 + d2 * f1 + f2,
        ])
    }

    pub fn apply(self, x: f64, y: f64) -> (f64, f64) {
        let [a, b, c, d, e, f] = self.0;
        (a * x + c * y + e, b * x + d * y + f)
    }

    /// 逆矩阵；退化（行列式为 0）时退回单位矩阵。
    pub fn inverse(self) -> Self {
        let [a, b, c, d, e, f] = self.0;
        let det = a * d - b * c;
        if det.abs() < 1e-12 {
            return Matrix::IDENTITY;
        }
        let ia = d / det;
        let ib = -b / det;
        let ic = -c / det;
        let id = a / det;
        Matrix([ia, ib, ic, id, -(ia * e + ic * f), -(ib * e + id * f)])
    }

    pub fn is_identity(self) -> bool {
        let [a, b, c, d, e, f] = self.0;
        (a - 1.0).abs() < 1e-9
            && b.abs() < 1e-9
            && c.abs() < 1e-9
            && (d - 1.0).abs() < 1e-9
            && e.abs() < 1e-9
            && f.abs() < 1e-9
    }
}

/// 给页面加一层内容变换，并把页面尺寸改成 `[0, 0, width, height]`。
///
/// `matrix` 把「页面原有内容的用户坐标」映射到新页面的用户坐标。
/// `strip_old` 为真时，先摘掉本工具此前加在该页的变换层。
///
/// MediaBox 与 CropBox 都会设成新页面（等价于整页可见），
/// 避免旧的 CropBox 残留导致「改了尺寸但可见区没变」。
pub fn apply_transform(
    doc: &mut Document,
    page: &Page,
    matrix: Matrix,
    size: (f64, f64),
    strip_old: bool,
) -> Result<()> {
    let (width, height) = size;
    if !(width.is_finite() && height.is_finite()) || width <= 0.0 || height <= 0.0 {
        return Err(BatchError::Arg(format!(
            "目标页面尺寸必须为正，当前是 {width}×{height}"
        )));
    }
    if strip_old {
        strip(doc, page)?;
    }

    if !matrix.is_identity() {
        let mut list = page_contents(doc, page)?;
        let open = Object::Ref(doc.add(Object::Stream(Stream::from_raw(
            tagged(MARK_SCALE),
            open_content(matrix),
        ))));
        let close = Object::Ref(doc.add(Object::Stream(Stream::from_raw(
            tagged(MARK_SCALE),
            b"Q % pdfbatch scale\n".to_vec(),
        ))));
        // 变换包住整页内容：`q … cm` 在最前、`Q` 在最后，配对不被拆散。
        list.insert(0, open);
        list.push(close);
        set_page_contents(doc, page, list)?;
    }

    doc.update_dict(page.obj, |d| {
        let rect = Object::Array(vec![
            Object::Real(0.0),
            Object::Real(0.0),
            Object::Real(width),
            Object::Real(height),
        ]);
        d.set(b"MediaBox".to_vec(), rect.clone());
        d.set(b"CropBox".to_vec(), rect);
    })?;
    Ok(())
}

/// 摘掉本工具此前加在该页的变换层，返回摘掉的流数量。
pub fn strip(doc: &mut Document, page: &Page) -> Result<usize> {
    let list = page_contents(doc, page)?;
    let total = list.len();
    let kept: Vec<Object> = list
        .into_iter()
        .filter(|object| stream_tag(doc, object).as_deref() != Some(MARK_SCALE))
        .collect();
    let removed = total - kept.len();
    if removed > 0 {
        set_page_contents(doc, page, kept)?;
    }
    Ok(removed)
}

/// 页面上是否已经有本工具加的变换层。
pub fn has_transform(doc: &mut Document, page: &Page) -> Result<bool> {
    let list = page_contents(doc, page)?;
    Ok(list
        .iter()
        .any(|object| stream_tag(doc, object).as_deref() == Some(MARK_SCALE)))
}

/// 变换层开头那段内容流。首字节是换行，避免与上一段流末尾的记号粘连。
fn open_content(matrix: Matrix) -> Vec<u8> {
    let [a, b, c, d, e, f] = matrix.0;
    format!(
        "\n% pdfbatch scale\nq\n{} {} {} {} {} {} cm\n",
        num(a),
        num(b),
        num(c),
        num(d),
        num(e),
        num(f)
    )
    .into_bytes()
}

/// PDF 里数字的紧凑写法：最多四位小数，去掉多余的零。
fn num(value: f64) -> String {
    let text = format!("{value:.4}");
    let trimmed = text.trim_end_matches('0').trim_end_matches('.');
    if trimmed.is_empty() || trimmed == "-" {
        "0".to_string()
    } else {
        trimmed.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pdfcraft_cos::{Dict, Object};

    #[test]
    fn translate_then_scale_reads_left_to_right() {
        // 先把 (10,20) 平移到原点，再放大 2 倍，再平移到 (5,5)。
        let m = Matrix::translate(-10.0, -20.0)
            .then(Matrix::scale(2.0, 2.0))
            .then(Matrix::translate(5.0, 5.0));
        assert_eq!(m.apply(10.0, 20.0), (5.0, 5.0), "原点应当落在平移后的位置");
        assert_eq!(m.apply(11.0, 21.0), (7.0, 7.0), "放大 2 倍");
    }

    #[test]
    fn inverse_undoes_the_matrix() {
        let m = Matrix::translate(-30.0, 40.0)
            .then(Matrix::scale(1.5, 0.5))
            .then(Matrix::translate(7.0, 9.0));
        let back = m.inverse();
        let (x, y) = m.apply(123.0, 456.0);
        let (rx, ry) = back.apply(x, y);
        assert!((rx - 123.0).abs() < 1e-9 && (ry - 456.0).abs() < 1e-9, "{rx},{ry}");
    }

    #[test]
    fn identity_is_detected_and_degenerate_inverse_is_safe() {
        assert!(Matrix::IDENTITY.is_identity());
        assert!(!Matrix::scale(2.0, 1.0).is_identity());
        // 行列式为 0 时退回单位矩阵，而不是产生 NaN。
        assert_eq!(Matrix::scale(0.0, 1.0).inverse(), Matrix::IDENTITY);
    }

    /// 一页的最小文档（页面 200×300），带一段原有内容。
    fn document_with_content() -> Document {
        let mut doc = Document::new_empty();
        let pages_ref = doc.add(Dict::new());
        let page = doc.add(Dict::new());
        let root = doc.root().expect("new_empty 有根节点");
        doc.update_dict(root, |d| {
            d.set(b"Type".to_vec(), Object::name("Catalog"));
            d.set(b"Pages".to_vec(), Object::Ref(pages_ref));
        })
        .unwrap();
        doc.update_dict(pages_ref, |d| {
            d.set(b"Type".to_vec(), Object::name("Pages"));
            d.set(b"Kids".to_vec(), Object::Array(vec![Object::Ref(page)]));
            d.set(b"Count".to_vec(), Object::Int(1));
        })
        .unwrap();
        let content = doc.add(Object::Stream(Stream::from_raw(Dict::new(), b"1 0 0 RG\n".to_vec())));
        doc.update_dict(page, |d| {
            d.set(b"Type".to_vec(), Object::name("Page"));
            d.set(b"Parent".to_vec(), Object::Ref(pages_ref));
            d.set(
                b"MediaBox".to_vec(),
                Object::Array(vec![Object::Int(0), Object::Int(0), Object::Int(200), Object::Int(300)]),
            );
            d.set(b"Contents".to_vec(), Object::Ref(content));
        })
        .unwrap();
        doc
    }

    fn first_page(doc: &Document) -> Page {
        pdfcraft_model::pages(doc)[0].clone()
    }

    #[test]
    fn transform_is_inserted_and_page_size_is_rewritten() {
        let mut doc = document_with_content();
        let page = first_page(&doc);
        let matrix = Matrix::scale(0.5, 0.5);
        apply_transform(&mut doc, &page, matrix, (100.0, 150.0), true).unwrap();

        // 尺寸改成了新页面，MediaBox 与 CropBox 一致。
        let page = first_page(&doc);
        assert_eq!(page.crop(&doc), [0.0, 0.0, 100.0, 150.0]);

        // 内容流变成 [变换头, 原内容, 变换尾]。
        let list = page_contents(&mut doc, &page).unwrap();
        assert_eq!(list.len(), 3, "{list:?}");
        assert_eq!(stream_tag(&doc, &list[0]).as_deref(), Some("PBScale"));
        assert_eq!(stream_tag(&doc, &list[1]), None, "原有内容不带标记");
        assert_eq!(stream_tag(&doc, &list[2]).as_deref(), Some("PBScale"));
        assert!(has_transform(&mut doc, &page).unwrap());
    }

    #[test]
    fn reapplying_replaces_the_old_transform_instead_of_stacking() {
        let mut doc = document_with_content();
        let page = first_page(&doc);
        apply_transform(&mut doc, &page, Matrix::scale(0.5, 0.5), (100.0, 150.0), true).unwrap();
        let page = first_page(&doc);
        apply_transform(&mut doc, &page, Matrix::scale(0.25, 0.25), (50.0, 75.0), true).unwrap();

        let page = first_page(&doc);
        let list = page_contents(&mut doc, &page).unwrap();
        assert_eq!(list.len(), 3, "应当是「摘掉旧的再加新的」，而不是叠加：{list:?}");
        assert_eq!(page.crop(&doc), [0.0, 0.0, 50.0, 75.0]);
    }

    #[test]
    fn strip_removes_only_our_own_streams() {
        let mut doc = document_with_content();
        let page = first_page(&doc);
        apply_transform(&mut doc, &page, Matrix::scale(0.5, 0.5), (100.0, 150.0), false).unwrap();
        let page = first_page(&doc);
        assert_eq!(strip(&mut doc, &page).unwrap(), 2);
        let list = page_contents(&mut doc, &page).unwrap();
        assert_eq!(list.len(), 1, "原有内容必须保留");
        assert!(!has_transform(&mut doc, &page).unwrap());
    }

    #[test]
    fn identity_matrix_only_rewrites_the_size() {
        let mut doc = document_with_content();
        let page = first_page(&doc);
        apply_transform(&mut doc, &page, Matrix::IDENTITY, (80.0, 120.0), true).unwrap();
        let page = first_page(&doc);
        assert_eq!(page.crop(&doc), [0.0, 0.0, 80.0, 120.0]);
        assert!(!has_transform(&mut doc, &page).unwrap(), "不应插入多余的变换层");
    }

    #[test]
    fn a_non_positive_size_is_rejected() {
        let mut doc = document_with_content();
        let page = first_page(&doc);
        let error = apply_transform(&mut doc, &page, Matrix::IDENTITY, (0.0, 100.0), false).unwrap_err();
        assert!(matches!(error, BatchError::Arg(_)), "{error}");
        let error = apply_transform(&mut doc, &page, Matrix::IDENTITY, (f64::NAN, 100.0), false).unwrap_err();
        assert!(matches!(error, BatchError::Arg(_)), "{error}");
    }

    #[test]
    fn the_open_stream_carries_the_matrix() {
        let text = String::from_utf8(open_content(Matrix::scale(0.5, 0.25))).unwrap();
        assert!(text.contains("0.5 0 0 0.25 0 0 cm"), "{text}");
        assert!(text.contains("q\n"), "{text}");
        assert!(text.starts_with('\n'), "首字节应当是换行");
    }
}
