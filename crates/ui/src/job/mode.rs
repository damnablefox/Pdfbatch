//! 界面动作枚举：模式、输出位置，以及填充层的动作与粒度选项。

/// 三种模式。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// 给页面盖纯色填充层。
    Fill,
    /// 调整页面尺寸（按目标尺寸缩放，或按扫描件图像适配）。
    PageSize,
    /// 从文本导入书签。
    Bookmarks,
}

impl Mode {
    pub const ALL: [Mode; 3] = [Mode::Fill, Mode::PageSize, Mode::Bookmarks];

    pub fn label(self) -> &'static str {
        match self {
            Mode::Fill => "填充区域",
            Mode::PageSize => "页面尺寸",
            Mode::Bookmarks => "书签",
        }
    }

    /// 同目录另存时的默认文件名后缀。
    pub fn suffix(self) -> &'static str {
        match self {
            Mode::Fill => "_filled",
            Mode::PageSize => "_resized",
            Mode::Bookmarks => "_bookmarked",
        }
    }
}

/// 输出位置的选择。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutputChoice {
    /// 与原文件同目录，文件名加后缀。
    SameFolder,
    /// 写到指定目录（保留目录输入的子路径）。
    Dir,
    /// 覆盖原文件。
    InPlace,
}

/// 填充区域的操作：新增，或对已有填充层做删改。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FillAction {
    Add,
    Edit,
}

impl FillAction {
    pub const ALL: [FillAction; 2] = [FillAction::Add, FillAction::Edit];

    pub fn label(self) -> &'static str {
        match self {
            FillAction::Add => "新增",
            FillAction::Edit => "删改",
        }
    }
}

/// 「删改」下再分：删除命中的层，或把命中的层改成新属性。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FillEditMode {
    Erase,
    Modify,
}

impl FillEditMode {
    pub const ALL: [FillEditMode; 2] = [FillEditMode::Erase, FillEditMode::Modify];

    pub fn label(self) -> &'static str {
        match self {
            FillEditMode::Erase => "删除",
            FillEditMode::Modify => "修改",
        }
    }
}

/// 查找时按哪种粒度比对填充区域。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MatchRect {
    /// 不限区域。
    Any,
    /// 只匹配整页填充。
    FullPage,
    /// 只比宽高（不看左下起点）。
    Size,
    /// 完整匹配左下起点与宽高。
    Rect,
}

impl MatchRect {
    pub const ALL: [MatchRect; 4] =
        [MatchRect::Any, MatchRect::FullPage, MatchRect::Size, MatchRect::Rect];

    pub fn label(self) -> &'static str {
        match self {
            MatchRect::Any => "不限",
            MatchRect::FullPage => "整页",
            MatchRect::Size => "按宽高",
            MatchRect::Rect => "按起点+宽高",
        }
    }
}

/// 要把命中的填充区域改成什么样。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChangeRect {
    /// 保持原样。
    Keep,
    /// 改成整页。
    FullPage,
    /// 改成具体矩形。
    Rect,
}

impl ChangeRect {
    pub const ALL: [ChangeRect; 3] = [ChangeRect::Keep, ChangeRect::FullPage, ChangeRect::Rect];

    pub fn label(self) -> &'static str {
        match self {
            ChangeRect::Keep => "保持",
            ChangeRect::FullPage => "改成整页",
            ChangeRect::Rect => "改成矩形",
        }
    }
}
