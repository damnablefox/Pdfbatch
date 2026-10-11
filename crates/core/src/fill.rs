//! 给页面的指定位置盖一层纯色填充，并控制混合模式与层级。
//!
//! 每页做三件事：
//!
//! 1. 在该页自己的 `Resources` 里注册一个图形状态（含 `/BM` 混合模式与 `/CA`、`/ca` 不透明度）；
//! 2. 生成一段内容流：`q /<状态> gs R G B rg x y w h re f Q`；
//! 3. 按层级写进 `/Contents`——`Bottom` 插到最前，`Top` 追加到最后（先把原有内容用 q/Q 包住）。
//!
//! 添加的内容流会打上 `/PBMark` 标记（写在流字典里，识别时无需解压页面内容），
//! `--replace` 就是靠它把上一次加的东西先摘掉。流字典里还写了 `/PBInfo`
//! （矩形、颜色、透明度、混合模式、层级），因此可以**按条件查找并删除/修改**已有的填充层；
//! 旧版本留下的、没有 `/PBInfo` 的层，会回退去解析内容流取回属性。

use pdfcraft_cos::{Dict, Document, Object, Stream};
use pdfcraft_model::{Page, pages};

use crate::blend::Blend;
use crate::error::{BatchError, Result};

/// 标记键与两个标记值：填充层、包裹原有内容的 q/Q 对。
const MARK_KEY: &[u8] = b"PBMark";
const MARK_FILL: &str = "PBFill";
const MARK_WRAP: &str = "PBWrap";
/// 属性字典的键（旧版本没有它）。
const INFO_KEY: &[u8] = b"PBInfo";

/// 填充相对于页面其它内容的层级。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layer {
    /// 覆盖在原有内容之上（追加到内容流末尾）。
    Top,
    /// 垫在原有内容之下（插到内容流开头）。
    Bottom,
}

impl Layer {
    pub fn parse(s: &str) -> std::result::Result<Self, String> {
        match s.trim().to_ascii_lowercase().as_str() {
            "top" | "above" | "上" | "上层" => Ok(Layer::Top),
            "bottom" | "below" | "under" | "下" | "底层" => Ok(Layer::Bottom),
            _ => Err(format!("未知层级 `{s}`；可用值：top、bottom")),
        }
    }

    pub fn canonical(self) -> &'static str {
        match self {
            Layer::Top => "top",
            Layer::Bottom => "bottom",
        }
    }
}

/// 一次填充请求的参数。
#[derive(Clone, Copy, Debug)]
pub struct FillStyle {
    /// 用户空间下的矩形 `[x, y, 宽, 高]`；`None` 表示整页（用页面 CropBox）。
    pub rect: Option<[f64; 4]>,
    pub color: [f64; 3],
    pub opacity: f64,
    pub blend: Blend,
    pub layer: Layer,
}

/// 一条填充层上记录的属性（写入时记进流字典，便于以后精确查找）。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FillInfo {
    pub rect: [f64; 4],
    /// 是不是「整页填充」（`FillStyle::rect == None`）。
    pub full_page: bool,
    pub color: [f64; 3],
    pub opacity: f64,
    pub blend: Blend,
    pub layer: Layer,
}

impl FillInfo {
    fn to_dict(self) -> Dict {
        let mut dict = Dict::new();
        dict.set(b"Rect".to_vec(), numbers(&self.rect));
        dict.set(b"FullPage".to_vec(), Object::Bool(self.full_page));
        dict.set(b"Color".to_vec(), numbers(&self.color));
        dict.set(b"Opacity".to_vec(), Object::Real(self.opacity));
        dict.set(b"Blend".to_vec(), Object::name(self.blend.canonical()));
        dict.set(b"Layer".to_vec(), Object::name(self.layer.canonical()));
        dict
    }

    fn from_dict(dict: &Dict) -> Option<Self> {
        Some(FillInfo {
            rect: read_numbers::<4>(dict, b"Rect")?,
            full_page: matches!(dict.get(b"FullPage"), Some(Object::Bool(true))),
            color: read_numbers::<3>(dict, b"Color")?,
            opacity: dict.get(b"Opacity").and_then(Object::as_f64)?,
            blend: read_name(dict, b"Blend").and_then(|n| Blend::parse(&n).ok())?,
            layer: read_name(dict, b"Layer").and_then(|n| Layer::parse(&n).ok())?,
        })
    }
}

/// 矩形匹配的三种粒度。
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum RectMatch {
    /// 只要「整页填充」。
    FullPage,
    /// 只比宽高（不看位置）。
    Size([f64; 2]),
    /// 比完整的左下起点与宽高。
    Rect([f64; 4]),
}

/// 查找填充层的条件；`None` 表示该维度不限制。`tolerance` 用于数值比较。
#[derive(Clone, Copy, Debug, Default)]
pub struct Matcher {
    pub rect: Option<RectMatch>,
    pub color: Option<[f64; 3]>,
    pub opacity: Option<f64>,
    pub blend: Option<Blend>,
    pub layer: Option<Layer>,
    /// 数值容差（矩形、颜色、透明度都用它）。0 表示精确相等。
    pub tolerance: f64,
}

impl Matcher {
    /// 是否没有任何条件（等价于「全部命中」）。
    pub fn is_empty(&self) -> bool {
        self.rect.is_none()
            && self.color.is_none()
            && self.opacity.is_none()
            && self.blend.is_none()
            && self.layer.is_none()
    }

    fn matches(&self, info: &FillInfo) -> bool {
        let near = |a: f64, b: f64| (a - b).abs() <= self.tolerance;
        if let Some(rect) = self.rect {
            let ok = match rect {
                RectMatch::FullPage => info.full_page,
                RectMatch::Size([w, h]) => near(info.rect[2], w) && near(info.rect[3], h),
                RectMatch::Rect([x, y, w, h]) => {
                    near(info.rect[0], x)
                        && near(info.rect[1], y)
                        && near(info.rect[2], w)
                        && near(info.rect[3], h)
                }
            };
            if !ok {
                return false;
            }
        }
        if let Some(color) = self.color {
            let same = near(info.color[0], color[0])
                && near(info.color[1], color[1])
                && near(info.color[2], color[2]);
            if !same {
                return false;
            }
        }
        if let Some(opacity) = self.opacity {
            if !near(info.opacity, opacity) {
                return false;
            }
        }
        if let Some(blend) = self.blend {
            if info.blend != blend {
                return false;
            }
        }
        if let Some(layer) = self.layer {
            if info.layer != layer {
                return false;
            }
        }
        true
    }
}

/// 修改命中填充层时要写入的新属性；`None` 表示保持原值。
#[derive(Clone, Copy, Debug, Default)]
pub struct FillChange {
    /// 改成具体矩形（同时取消「整页」标记）。
    pub rect: Option<[f64; 4]>,
    /// 改成整页（`true`，矩形取页面 CropBox）或取消整页标记（`false`）。
    pub full_page: Option<bool>,
    pub color: Option<[f64; 3]>,
    pub opacity: Option<f64>,
    pub blend: Option<Blend>,
    pub layer: Option<Layer>,
}

impl FillChange {
    pub fn is_empty(&self) -> bool {
        self.rect.is_none()
            && self.full_page.is_none()
            && self.color.is_none()
            && self.opacity.is_none()
            && self.blend.is_none()
            && self.layer.is_none()
    }

    fn apply_to(&self, info: &FillInfo) -> FillInfo {
        let mut out = *info;
        if let Some(full) = self.full_page {
            out.full_page = full;
        }
        if let Some(rect) = self.rect {
            out.rect = rect;
            out.full_page = false;
        }
        if let Some(color) = self.color {
            out.color = color;
        }
        if let Some(opacity) = self.opacity {
            out.opacity = opacity;
        }
        if let Some(blend) = self.blend {
            out.blend = blend;
        }
        if let Some(layer) = self.layer {
            out.layer = layer;
        }
        out
    }
}

/// 在给定页面上盖填充层，返回实际处理过的页数。
///
/// `replace` 为真时先摘掉本工具此前加在该页的**全部**填充层。
pub fn apply(doc: &mut Document, targets: &[usize], style: &FillStyle, replace: bool) -> Result<usize> {
    let all = pages(doc);
    if all.is_empty() {
        return Err(BatchError::Arg("文档没有任何页面".to_string()));
    }
    let mut painted = 0;
    for &index in targets {
        let page = page_at(&all, index)?;
        if replace {
            strip(doc, &page)?;
        }
        paint(doc, &page, style)?;
        painted += 1;
    }
    Ok(painted)
}

/// 统计命中的填充层数量（只看不改）。
pub fn count(doc: &mut Document, targets: &[usize], matcher: &Matcher) -> Result<usize> {
    let all = pages(doc);
    let mut total = 0;
    for &index in targets {
        let page = page_at(&all, index)?;
        total += fill_layers(doc, &page)?.into_iter().filter(|l| matcher.matches(&l.info)).count();
    }
    Ok(total)
}

/// 删除命中的填充层，返回删除数。
pub fn remove(doc: &mut Document, targets: &[usize], matcher: &Matcher) -> Result<usize> {
    let all = pages(doc);
    let mut removed = 0;
    for &index in targets {
        let page = page_at(&all, index)?;
        removed += remove_on_page(doc, &page, matcher)?;
    }
    Ok(removed)
}

/// 修改命中的填充层，返回修改数。
pub fn modify(
    doc: &mut Document,
    targets: &[usize],
    matcher: &Matcher,
    change: &FillChange,
) -> Result<usize> {
    let all = pages(doc);
    let mut changed = 0;
    for &index in targets {
        let page = page_at(&all, index)?;
        changed += modify_on_page(doc, &page, matcher, change)?;
    }
    Ok(changed)
}

fn page_at(all: &[Page], index: usize) -> Result<Page> {
    all.get(index)
        .cloned()
        .ok_or_else(|| BatchError::Arg(format!("文档没有第 {} 页", index + 1)))
}

/// 在单页上完成"注册图形状态 → 生成内容流 → 按层级写入"。
fn paint(doc: &mut Document, page: &Page, style: &FillStyle) -> Result<()> {
    let crop = page.crop(doc);
    let full_page = style.rect.is_none();
    let rect = style.rect.unwrap_or([crop[0], crop[1], crop[2] - crop[0], crop[3] - crop[1]]);
    if !(rect[2] > 0.0 && rect[3] > 0.0) {
        return Err(BatchError::Arg(format!(
            "填充矩形宽高必须为正，当前是 {}×{}",
            rect[2], rect[3]
        )));
    }

    let info = FillInfo {
        rect,
        full_page,
        color: style.color,
        opacity: style.opacity,
        blend: style.blend,
        layer: style.layer,
    };
    let state_name = add_gstate(doc, page, style.blend, style.opacity)?;
    let content = fill_content(&state_name, info.color, rect);
    place(doc, page, content, &info)
}

/// 在页面自己的资源字典里注册图形状态，返回它在内容流里的名字。
fn add_gstate(doc: &mut Document, page: &Page, blend: Blend, opacity: f64) -> Result<String> {
    // 名字带上参数，内容流里一眼能看出用的是什么混合模式与不透明度。
    let name = format!("PBFill{}{}", blend.canonical(), (opacity * 100.0).round() as i64);

    let mut resources = page_owned_dict(doc, page, b"Resources");
    let mut states = resources
        .get(b"ExtGState")
        .map(|g| doc.resolve(g))
        .and_then(|g| g.as_dict().cloned())
        .unwrap_or_default();

    let mut state = Dict::new();
    state.set(b"Type".to_vec(), Object::name("ExtGState"));
    if let Some(blend) = blend.pdf_name() {
        state.set(b"BM".to_vec(), Object::name(blend));
    }
    state.set(b"CA".to_vec(), Object::Real(opacity));
    state.set(b"ca".to_vec(), Object::Real(opacity));
    states.set(name.clone().into_bytes(), Object::Dict(state));
    resources.set(b"ExtGState".to_vec(), Object::Dict(states));

    // 页面拿到自己的一份资源字典，共享的字典不被改动。
    doc.update_dict(page.obj, |d| d.set(b"Resources".to_vec(), Object::Dict(resources)))?;
    Ok(name)
}

/// 取页面某个键上的字典（没有就新建），并把它复制成页面自己的一份。
fn page_owned_dict(doc: &Document, page: &Page, key: &[u8]) -> Dict {
    page.dict
        .get(key)
        .map(|r| doc.resolve(r))
        .and_then(|r| r.as_dict().cloned())
        .unwrap_or_default()
}

/// 把一段内容写进页面的 `/Contents`。
///
/// - `Bottom`：插到最前；
/// - `Top`：先把页面原有内容用 q/Q 包住（隔离图形状态，避免颜色与状态"渗出"），再追加到末尾。
fn place(doc: &mut Document, page: &Page, content: Vec<u8>, info: &FillInfo) -> Result<()> {
    let mut list = contents(doc, page)?;
    let mark = Object::Ref(doc.add(Object::Stream(Stream::flate(tagged_fill(info), &content))));

    if info.layer == Layer::Bottom {
        list.insert(0, mark);
    } else {
        let wrapped = list.iter().any(|o| stream_tag(doc, o).as_deref() == Some(MARK_WRAP));
        let has_original = list.iter().any(|o| stream_tag(doc, o).is_none());
        if !wrapped && has_original {
            let open = Object::Ref(
                doc.add(Object::Stream(Stream::from_raw(tagged(MARK_WRAP), b"q % pdfbatch\n".to_vec()))),
            );
            let close = Object::Ref(
                doc.add(Object::Stream(Stream::from_raw(tagged(MARK_WRAP), b"Q % pdfbatch\n".to_vec()))),
            );
            // 包裹只圈住页面原有内容；先前垫在底层的填充留在包裹外。
            let split = list
                .iter()
                .position(|o| stream_tag(doc, o).is_none())
                .unwrap_or(list.len());
            list.insert(split, open);
            list.push(close);
        }
        list.push(mark);
    }

    doc.update_dict(page.obj, |d| d.set(b"Contents".to_vec(), Object::Array(list)))?;
    Ok(())
}

/// 摘掉本工具此前加在该页的填充层，返回移除的层数。
fn strip(doc: &mut Document, page: &Page) -> Result<usize> {
    remove_on_page(doc, page, &Matcher::default())
}

fn remove_on_page(doc: &mut Document, page: &Page, matcher: &Matcher) -> Result<usize> {
    let list = contents(doc, page)?;
    let layers = describe(doc, page, &list)?;
    let doomed: Vec<usize> = layers
        .iter()
        .filter(|l| matcher.matches(&l.info))
        .map(|l| l.index)
        .collect();
    if doomed.is_empty() {
        return Ok(0);
    }
    let mut kept: Vec<Object> = Vec::with_capacity(list.len());
    for (index, item) in list.into_iter().enumerate() {
        if !doomed.contains(&index) {
            kept.push(item);
        }
    }
    // 一页上如果已经没有别的填充层，包裹用的 q/Q 也一并去掉。
    if !kept.iter().any(|o| stream_tag(doc, o).as_deref() == Some(MARK_FILL)) {
        kept.retain(|o| stream_tag(doc, o).as_deref() != Some(MARK_WRAP));
    }
    doc.update_dict(page.obj, |d| d.set(b"Contents".to_vec(), Object::Array(kept)))?;
    Ok(doomed.len())
}

fn modify_on_page(
    doc: &mut Document,
    page: &Page,
    matcher: &Matcher,
    change: &FillChange,
) -> Result<usize> {
    let list = contents(doc, page)?;
    let hit: Vec<LayerEntry> = describe(doc, page, &list)?
        .into_iter()
        .filter(|l| matcher.matches(&l.info))
        .collect();
    if hit.is_empty() {
        return Ok(0);
    }
    let crop = page.crop(doc);
    let crop_rect = [crop[0], crop[1], crop[2] - crop[0], crop[3] - crop[1]];
    let mut layer_changed = false;
    for entry in &hit {
        let mut next = change.apply_to(&entry.info);
        // 整页层：矩形始终跟随页面 CropBox。
        if next.full_page {
            next.rect = crop_rect;
        }
        if !(next.rect[2] > 0.0 && next.rect[3] > 0.0) {
            return Err(BatchError::Arg(format!(
                "填充矩形宽高必须为正，当前是 {}×{}",
                next.rect[2], next.rect[3]
            )));
        }
        let state = add_gstate(doc, page, next.blend, next.opacity)?;
        let content = fill_content(&state, next.color, next.rect);
        let reference = entry
            .object
            .as_ref()
            .ok_or_else(|| BatchError::Arg("填充层不是间接对象，无法修改".to_string()))?;
        doc.set(reference, Object::Stream(Stream::flate(tagged_fill(&next), &content)));
        if next.layer != entry.info.layer {
            layer_changed = true;
        }
    }
    if layer_changed {
        reorder(doc, page)?;
    }
    Ok(hit.len())
}

/// 按每层记录的层级重排 `/Contents`：底层在前、原文居中、顶层在后。
fn reorder(doc: &mut Document, page: &Page) -> Result<()> {
    let list = contents(doc, page)?;
    let entries = describe(doc, page, &list)?;
    let mut out: Vec<Object> = Vec::with_capacity(list.len());
    // 底层填充放在最前。
    for entry in entries.iter().filter(|e| e.info.layer == Layer::Bottom) {
        out.push(entry.object.clone());
    }
    // 其余（原文与包裹）保持原顺序。
    for (index, item) in list.iter().enumerate() {
        if !entries.iter().any(|e| e.index == index) {
            out.push(item.clone());
        }
    }
    // 顶层填充放在最后。
    for entry in entries.iter().filter(|e| e.info.layer == Layer::Top) {
        out.push(entry.object.clone());
    }
    doc.update_dict(page.obj, |d| d.set(b"Contents".to_vec(), Object::Array(out)))?;
    Ok(())
}

/// 页面上的一条填充层：在 `/Contents` 里的下标、对象引用、属性。
struct LayerEntry {
    index: usize,
    object: Object,
    info: FillInfo,
}

/// 列出页面上所有填充层。属性优先取 `/PBInfo`，旧文件回退解析内容流；
/// 层序按它在 `/Contents` 里的位置判定（原文之前的算底层）。
fn fill_layers(doc: &mut Document, page: &Page) -> Result<Vec<LayerEntry>> {
    let list = contents(doc, page)?;
    describe(doc, page, &list)
}

fn describe(doc: &Document, page: &Page, list: &[Object]) -> Result<Vec<LayerEntry>> {
    let crop_page = page.crop(doc);
    let crop = [crop_page[0], crop_page[1], crop_page[2] - crop_page[0], crop_page[3] - crop_page[1]];
    // 原文（无标记的流）从哪里开始：在它之前的填充属于「底层」。
    let boundary = list
        .iter()
        .position(|o| stream_tag(doc, o).is_none())
        .unwrap_or(list.len());
    let mut out = Vec::new();
    for (index, item) in list.iter().enumerate() {
        if stream_tag(doc, item).as_deref() != Some(MARK_FILL) {
            continue;
        }
        let fallback_layer = if index < boundary { Layer::Bottom } else { Layer::Top };
        if let Some(info) = read_info(doc, item, crop, fallback_layer) {
            out.push(LayerEntry { index, object: item.clone(), info });
        }
    }
    Ok(out)
}

/// 读一条填充层的属性：优先自家 `/PBInfo`；旧文件回退解析内容流。
fn read_info(doc: &Document, object: &Object, crop: [f64; 4], fallback_layer: Layer) -> Option<FillInfo> {
    if let Some(dict) = doc.resolve(object).as_dict() {
        if let Some(parsed) = dict.get(INFO_KEY).and_then(|o| o.as_dict()).and_then(FillInfo::from_dict) {
            return Some(parsed);
        }
    }
    // 回退：解析自己生成的内容流（`q /GS gs R G B rg x y w h re f Q`）。
    let resolved = doc.resolve(object);
    let Object::Stream(stream) = resolved.as_ref() else {
        return None;
    };
    let bytes = stream.decoded().ok()?;
    let (rect, color, blend, opacity) = parse_fill_content(&bytes)?;
    Some(FillInfo {
        rect,
        full_page: rect == crop,
        color,
        opacity,
        blend,
        layer: fallback_layer,
    })
}

/// 从自家生成的内容流里取回矩形、颜色、混合模式与透明度（旧文件的兜底）。
fn parse_fill_content(bytes: &[u8]) -> Option<([f64; 4], [f64; 3], Blend, f64)> {
    let text = String::from_utf8_lossy(bytes);
    let mut rect = None;
    let mut color = None;
    let mut state = None;
    for line in text.lines() {
        let tokens: Vec<&str> = line.split_whitespace().collect();
        if tokens.len() == 4 && tokens[3] == "rg" {
            color = Some([
                tokens[0].parse().ok()?,
                tokens[1].parse().ok()?,
                tokens[2].parse().ok()?,
            ]);
        }
        if tokens.len() == 6 && tokens[4] == "re" && tokens[5] == "f" {
            rect = Some([
                tokens[0].parse().ok()?,
                tokens[1].parse().ok()?,
                tokens[2].parse().ok()?,
                tokens[3].parse().ok()?,
            ]);
        }
        if tokens.len() == 2 && tokens[1] == "gs" {
            state = parse_state_name(tokens[0]);
        }
    }
    let (blend, opacity) = state?;
    Some((rect?, color?, blend, opacity))
}

/// 从图形状态名（`PBFill<Blend><不透明度×100>`）反推混合模式与透明度。
fn parse_state_name(name: &str) -> Option<(Blend, f64)> {
    let rest = name.trim_start_matches('/').strip_prefix("PBFill")?;
    let split = rest.find(|c: char| c.is_ascii_digit())?;
    let (blend_text, opacity_text) = rest.split_at(split);
    let opacity: f64 = opacity_text.parse().ok()?;
    let blend = if blend_text.is_empty() {
        Blend::Normal
    } else {
        Blend::parse(blend_text).ok()?
    };
    Some((blend, opacity / 100.0))
}

/// 页面 `/Contents` 的流引用列表。
///
/// 四种形态都要处理：缺失、间接引用指向数组、数组、单个流（提升为间接对象）。
fn contents(doc: &mut Document, page: &Page) -> Result<Vec<Object>> {
    let dict = doc.get(page.obj).as_dict().cloned().unwrap_or_default();
    Ok(match dict.get(b"Contents").cloned() {
        None => Vec::new(),
        Some(Object::Ref(r)) => match doc.get(r).as_array() {
            Some(array) => array.clone(),
            None => vec![Object::Ref(r)],
        },
        Some(Object::Array(array)) => array,
        Some(other) => vec![Object::Ref(doc.add(other))],
    })
}

/// 带标记的流字典。
fn tagged(tag: &str) -> Dict {
    let mut dict = Dict::new();
    dict.set(MARK_KEY.to_vec(), Object::name(tag));
    dict
}

/// 带标记与属性字典的填充流字典。
fn tagged_fill(info: &FillInfo) -> Dict {
    let mut dict = tagged(MARK_FILL);
    dict.set(INFO_KEY.to_vec(), Object::Dict(info.to_dict()));
    dict
}

/// 读一个对象（流）字典里的标记，不解压内容。
fn stream_tag(doc: &Document, object: &Object) -> Option<String> {
    doc.resolve(object)
        .as_dict()?
        .name(MARK_KEY)
        .map(|name| String::from_utf8_lossy(name).into_owned())
}

/// 生成填充用的内容流。首字节是换行，避免与上一段流末尾的记号粘连。
fn fill_content(state_name: &str, color: [f64; 3], rect: [f64; 4]) -> Vec<u8> {
    let mut out = String::new();
    out.push('\n');
    out.push_str("% pdfbatch fill\n");
    out.push_str("q\n");
    out.push_str(&format!("/{state_name} gs\n"));
    out.push_str(&format!("{} {} {} rg\n", number(color[0]), number(color[1]), number(color[2])));
    out.push_str(&format!(
        "{} {} {} {} re f\n",
        number(rect[0]),
        number(rect[1]),
        number(rect[2]),
        number(rect[3])
    ));
    out.push_str("Q\n");
    out.into_bytes()
}

/// PDF 里数字的紧凑写法：最多四位小数，去掉多余的零。
fn number(value: f64) -> String {
    let text = format!("{value:.4}");
    let trimmed = text.trim_end_matches('0').trim_end_matches('.');
    if trimmed.is_empty() || trimmed == "-" {
        "0".to_string()
    } else {
        trimmed.to_string()
    }
}

/// 把一串数字包成 PDF 数组。
fn numbers(values: &[f64]) -> Object {
    Object::Array(values.iter().map(|v| Object::Real(*v)).collect())
}

/// 读一个定长数字数组。
fn read_numbers<const N: usize>(dict: &Dict, key: &[u8]) -> Option<[f64; N]> {
    let array = dict.get(key)?.as_array()?;
    if array.len() != N {
        return None;
    }
    let mut out = [0.0; N];
    for (slot, item) in out.iter_mut().zip(array) {
        *slot = item.as_f64()?;
    }
    Some(out)
}

/// 读一个名字键。
fn read_name(dict: &Dict, key: &[u8]) -> Option<String> {
    let bytes = dict.get(key)?.as_name()?;
    Some(String::from_utf8_lossy(bytes).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_are_compact() {
        assert_eq!(number(0.0), "0");
        assert_eq!(number(1.0), "1");
        assert_eq!(number(0.25), "0.25");
        assert_eq!(number(-12.5), "-12.5");
        assert_eq!(number(0.123456), "0.1235");
    }

    #[test]
    fn content_stream_is_well_formed() {
        let content = String::from_utf8(fill_content("PBFillMultiply25", [1.0, 0.95, 0.69], [10.0, 20.0, 100.0, 50.0]))
            .expect("内容流是 UTF-8");
        assert!(content.starts_with('\n'), "首字节必须是换行");
        assert!(content.contains("/PBFillMultiply25 gs"), "{content}");
        assert!(content.contains("1 0.95 0.69 rg"), "{content}");
        assert!(content.contains("10 20 100 50 re f"), "{content}");
        let lines: Vec<&str> = content.lines().collect();
        assert_eq!(lines[1], "% pdfbatch fill");
        assert_eq!(*lines.last().expect("有最后一行"), "Q");
    }

    #[test]
    fn layer_names_round_trip() {
        assert_eq!(Layer::parse("top").unwrap(), Layer::Top);
        assert_eq!(Layer::parse("BOTTOM").unwrap(), Layer::Bottom);
        assert_eq!(Layer::parse("上层").unwrap(), Layer::Top);
        assert!(Layer::parse("middle").is_err());
        for layer in [Layer::Top, Layer::Bottom] {
            assert_eq!(Layer::parse(layer.canonical()).unwrap(), layer);
        }
    }

    fn sample_info() -> FillInfo {
        FillInfo {
            rect: [10.0, 20.0, 100.0, 50.0],
            full_page: false,
            color: [1.0, 0.95, 0.69],
            opacity: 0.25,
            blend: Blend::Multiply,
            layer: Layer::Top,
        }
    }

    #[test]
    fn info_survives_a_dict_round_trip() {
        let info = sample_info();
        let dict = info.to_dict();
        assert_eq!(FillInfo::from_dict(&dict), Some(info));
    }

    #[test]
    fn fallback_parses_our_own_content_stream() {
        let content = fill_content("PBFillMultiply25", [1.0, 0.95, 0.69], [10.0, 20.0, 100.0, 50.0]);
        let (rect, color, blend, opacity) = parse_fill_content(&content).expect("能解析自家内容流");
        assert_eq!(rect, [10.0, 20.0, 100.0, 50.0]);
        assert_eq!(color, [1.0, 0.95, 0.69]);
        assert_eq!(blend, Blend::Multiply);
        assert_eq!(opacity, 0.25);
        // 普通（Normal）层没有混合模式名，名字里只剩不透明度。
        assert_eq!(parse_state_name("/PBFill100"), Some((Blend::Normal, 1.0)));
    }

    #[test]
    fn matcher_checks_only_requested_dimensions() {
        let info = sample_info();
        assert!(Matcher::default().matches(&info), "无条件等于全命中");
        assert!(Matcher::default().is_empty());

        let by_color = Matcher { color: Some([1.0, 0.95, 0.69]), ..Matcher::default() };
        assert!(by_color.matches(&info));
        let wrong_color = Matcher { color: Some([0.0, 0.0, 0.0]), ..Matcher::default() };
        assert!(!wrong_color.matches(&info));

        let by_blend = Matcher { blend: Some(Blend::Multiply), ..Matcher::default() };
        assert!(by_blend.matches(&info));
        assert!(!Matcher { blend: Some(Blend::Screen), ..Matcher::default() }.matches(&info));

        let by_size = Matcher { rect: Some(RectMatch::Size([100.0, 50.0])), ..Matcher::default() };
        assert!(by_size.matches(&info), "只比宽高时忽略位置");
        let by_rect = Matcher { rect: Some(RectMatch::Rect([0.0, 20.0, 100.0, 50.0])), ..Matcher::default() };
        assert!(!by_rect.matches(&info), "起点不同就不算命中");
        assert!(!Matcher { rect: Some(RectMatch::FullPage), ..Matcher::default() }.matches(&sample_info()));
        assert!(Matcher { rect: Some(RectMatch::FullPage), ..Matcher::default() }
            .matches(&FillInfo { full_page: true, ..info }));
    }

    #[test]
    fn tolerance_widens_numeric_matches() {
        let info = sample_info();
        let near = Matcher { opacity: Some(0.2504), tolerance: 0.001, ..Matcher::default() };
        assert!(near.matches(&info));
        let strict = Matcher { opacity: Some(0.2504), tolerance: 0.0, ..Matcher::default() };
        assert!(!strict.matches(&info));
    }

    #[test]
    fn change_only_replaces_what_it_is_given() {
        let info = sample_info();
        let change = FillChange { color: Some([0.0, 0.0, 0.0]), ..FillChange::default() };
        let next = change.apply_to(&info);
        assert_eq!(next.color, [0.0, 0.0, 0.0]);
        assert_eq!(next.rect, info.rect, "没给矩形就保持原值");
        assert_eq!(next.blend, info.blend);
        assert_eq!(next.opacity, info.opacity);
        assert!(FillChange::default().is_empty(), "空修改等于什么都不做");
        assert!(!change.is_empty());
    }

    #[test]
    fn change_can_promote_a_layer_to_full_page() {
        let info = sample_info();
        let change = FillChange { full_page: Some(true), ..FillChange::default() };
        let next = change.apply_to(&info);
        assert!(next.full_page);
        // 具体矩形由调用方拿到页面 CropBox 后再覆盖。
        assert_eq!(next.rect, info.rect);
        assert!(!change.is_empty());
    }
}