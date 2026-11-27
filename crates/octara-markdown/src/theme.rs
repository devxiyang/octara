//! Semantic typography and spacing for Markdown documents.

use super::CalloutKind;
use octara::prelude::*;
use octara_icons as icons;

/// A themed Markdown stylesheet. Lengths and colors may refer to the window's theme.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MarkdownTheme {
    pub block_gap: Length,
    pub text_size: FontSize,
    /// Uses the inherited line height and theme default when unset.
    pub line_height: Option<LineHeight>,
    pub text_color: ColorValue,
    /// Styles in order from H1 to H6.
    pub headings: [HeadingStyle; 6],
    pub tight_list_gap: Length,
    pub loose_list_gap: Length,
    pub list: ListStyle,
    pub quote: QuoteStyle,
    pub rule: RuleStyle,
    pub strong_weight: FontWeight,
    pub strong_color: Option<ColorValue>,
    pub emphasis_color: Option<ColorValue>,
    pub strike_color: Option<ColorValue>,
    pub inline_code: InlineCodeStyle,
    pub link: LinkStyle,
    pub math: MathStyle,
    pub code_block: CodeBlockStyle,
    pub table: TableStyle,
}

impl Default for MarkdownTheme {
    fn default() -> Self {
        Self::document()
    }
}

impl MarkdownTheme {
    /// Comfortable spacing and a clear heading hierarchy for long documents.
    pub fn document() -> Self {
        Self {
            block_gap: sp(3.0),
            text_size: FontSizeToken::Base.into(),
            line_height: Some(LineHeight::Relative(1.55)),
            text_color: ColorToken::Text.into(),
            headings: [
                heading(FontSizeToken::Xl3, 700, 4.0, 1.0, true),
                heading(FontSizeToken::Xl2, 650, 3.0, 1.0, true),
                heading(FontSizeToken::Xl, 650, 2.0, 0.5, false),
                heading(FontSizeToken::Lg, 600, 1.5, 0.5, false),
                heading(FontSizeToken::Base, 600, 1.0, 0.0, false),
                HeadingStyle { color: Some(ColorToken::TextMuted.into()), ..heading(FontSizeToken::Base, 600, 1.0, 0.0, false) },
            ],
            tight_list_gap: sp(1.0),
            loose_list_gap: sp(2.5),
            list: ListStyle::default(),
            strong_weight: 700,
            strong_color: None,
            emphasis_color: None,
            strike_color: Some(ColorToken::TextMuted.into()),
            quote: QuoteStyle::default(),
            rule: RuleStyle::default(),
            inline_code: InlineCodeStyle::default(),
            link: LinkStyle::default(),
            math: MathStyle::default(),
            code_block: CodeBlockStyle::default(),
            table: TableStyle::default(),
        }
    }

    /// A shorter heading scale and spacing for conversational messages.
    pub fn chat() -> Self {
        let mut theme = Self::document();
        theme.block_gap = sp(2.0);
        theme.line_height = Some(LineHeight::Relative(1.45));
        theme.loose_list_gap = sp(1.5);
        theme.headings = [
            heading(FontSizeToken::Xl, 700, 1.0, 0.5, false),
            heading(FontSizeToken::Lg, 650, 1.0, 0.5, false),
            heading(FontSizeToken::Base, 650, 0.5, 0.0, false),
            HeadingStyle::default(),
            HeadingStyle::default(),
            HeadingStyle::default(),
        ];
        theme.code_block.padding = sp(2.0);
        theme
    }

    /// Reduced type and spacing for compact panels.
    pub fn compact() -> Self {
        let mut theme = Self::chat();
        theme.block_gap = sp(1.5);
        theme.text_size = FontSizeToken::Sm.into();
        theme.line_height = Some(LineHeight::Relative(1.35));
        theme.tight_list_gap = sp(0.5);
        theme.loose_list_gap = sp(1.0);
        theme.headings = [
            heading(FontSizeToken::Lg, 700, 1.0, 0.0, false),
            heading(FontSizeToken::Base, 650, 0.5, 0.0, false),
            heading(FontSizeToken::Sm, 650, 0.5, 0.0, false),
            heading(FontSizeToken::Sm, 600, 0.0, 0.0, false),
            heading(FontSizeToken::Sm, 600, 0.0, 0.0, false),
            heading(FontSizeToken::Sm, 600, 0.0, 0.0, false),
        ];
        theme.table.cell_padding = [sp(1.0), sp(2.0), sp(1.0), sp(2.0)].into();
        theme
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HeadingStyle {
    pub size: FontSize,
    pub weight: FontWeight,
    pub line_height: LineHeight,
    pub color: Option<ColorValue>,
    pub top_spacing: Length,
    pub bottom_spacing: Length,
    pub border_bottom: Option<ColorValue>,
}

impl Default for HeadingStyle {
    fn default() -> Self {
        heading(FontSizeToken::Base, 600, 0.0, 0.0, false)
    }
}

fn heading(size: FontSizeToken, weight: FontWeight, top: f32, bottom: f32, border: bool) -> HeadingStyle {
    HeadingStyle {
        size: size.into(),
        weight,
        line_height: LineHeight::Relative(1.25),
        color: None,
        top_spacing: sp(top),
        bottom_spacing: sp(bottom),
        border_bottom: border.then(|| ColorToken::BorderSubtle.into()),
    }
}

/// One alert's visual treatment. Labels come from the inherited Markdown locale.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CalloutStyle {
    pub tone: ColorToken,
    pub icon: icons::LucideIcon,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CalloutStyles {
    pub note: CalloutStyle,
    pub tip: CalloutStyle,
    pub important: CalloutStyle,
    pub warning: CalloutStyle,
    pub caution: CalloutStyle,
}

impl Default for CalloutStyles {
    fn default() -> Self {
        Self {
            note: CalloutStyle { tone: ColorToken::Info, icon: icons::INFO },
            tip: CalloutStyle { tone: ColorToken::Success, icon: icons::LIGHTBULB },
            important: CalloutStyle { tone: ColorToken::Accent, icon: icons::MESSAGE_SQUARE_EXCLAMATION_POINT },
            warning: CalloutStyle { tone: ColorToken::Warning, icon: icons::TRIANGLE_EXCLAMATION_POINT },
            caution: CalloutStyle { tone: ColorToken::Danger, icon: icons::OCTAGON_X },
        }
    }
}

impl CalloutStyles {
    pub fn get(&self, kind: CalloutKind) -> &CalloutStyle {
        match kind {
            CalloutKind::Note => &self.note,
            CalloutKind::Tip => &self.tip,
            CalloutKind::Important => &self.important,
            CalloutKind::Warning => &self.warning,
            CalloutKind::Caution => &self.caution,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ListStyle {
    pub bullet: &'static str,
    /// The suffix used after ordered markers, for example ".".
    pub ordered_suffix: &'static str,
    pub marker_color: ColorValue,
    pub marker_gap: Length,
}

impl Default for ListStyle {
    fn default() -> Self {
        Self { bullet: "•", ordered_suffix: ".", marker_color: ColorToken::TextMuted.into(), marker_gap: sp(2.0) }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MathStyle {
    pub color: Option<ColorValue>,
    pub background: ColorValue,
    pub padding: Length,
    pub radius: Radius,
}

impl Default for MathStyle {
    fn default() -> Self {
        Self { color: None, background: ColorToken::SurfacePressed.into(), padding: sp(2.0), radius: RadiusToken::Md.into() }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct QuoteStyle {
    pub bar_width: Length,
    pub callouts: CalloutStyles,
    pub bar_color: ColorValue,
    pub text_color: ColorValue,
}

impl Default for QuoteStyle {
    fn default() -> Self {
        Self {
            bar_width: px(3.0),
            bar_color: ColorToken::Border.into(),
            text_color: ColorToken::TextMuted.into(),
            callouts: CalloutStyles::default(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RuleStyle {
    pub height: Length,
    pub color: ColorValue,
}

impl Default for RuleStyle {
    fn default() -> Self {
        Self { height: px(1.0), color: ColorToken::Border.into() }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct InlineCodeStyle {
    pub background: Option<ColorValue>,
    pub text_color: Option<ColorValue>,
    pub border_color: Option<ColorValue>,
    pub radius: f32,
    pub padding_x: f32,
    pub padding_y: f32,
    /// Logical pixels. Uses the surrounding text size when unset.
    pub font_size: Option<f32>,
}

impl Default for InlineCodeStyle {
    fn default() -> Self {
        Self {
            background: Some(ColorToken::SurfacePressed.into()),
            text_color: None,
            border_color: None,
            radius: 4.0,
            padding_x: 4.0,
            padding_y: 1.0,
            font_size: None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LinkStyle {
    pub color: Option<ColorValue>,
    pub underline: bool,
}

impl Default for LinkStyle {
    fn default() -> Self {
        Self { color: None, underline: true }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CodeBlockStyle {
    pub background: ColorValue,
    pub header_background: ColorValue,
    pub border_color: ColorValue,
    pub radius: Radius,
    pub padding: Length,
    pub font_size: FontSize,
    pub wrap_lines: bool,
    /// Show the full fence info string above the code.
    pub show_header: bool,
    pub show_copy: bool,
    pub line_numbers: bool,
    /// Constrains the code body to a vertically scrolling viewport when set.
    pub max_height: Option<Length>,
}

impl Default for CodeBlockStyle {
    fn default() -> Self {
        Self {
            background: ColorToken::Surface.into(),
            header_background: ColorToken::SurfaceElevated.into(),
            border_color: ColorToken::Border.into(),
            radius: RadiusToken::Md.into(),
            padding: sp(3.0),
            font_size: FontSizeToken::Sm.into(),
            wrap_lines: false,
            show_header: true,
            show_copy: true,
            line_numbers: false,
            max_height: None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TableStyle {
    pub border_color: ColorValue,
    pub header_background: ColorValue,
    pub row_background: ColorValue,
    /// Background for every second body row. Unset for a solid body.
    pub striped_background: Option<ColorValue>,
    pub cell_padding: Edges<Length>,
    pub radius: Radius,
}

impl Default for TableStyle {
    fn default() -> Self {
        Self {
            border_color: ColorToken::Border.into(),
            header_background: ColorToken::SurfaceElevated.into(),
            row_background: ColorToken::Background.into(),
            striped_background: None,
            cell_padding: [sp(2.0), sp(3.0), sp(2.0), sp(3.0)].into(),
            radius: RadiusToken::Md.into(),
        }
    }
}

/// Labels for document controls and callouts. Provide a value in the build scope to localize a document.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MarkdownLocale {
    pub copy: &'static str,
    pub copied: &'static str,
    pub note: &'static str,
    pub tip: &'static str,
    pub important: &'static str,
    pub warning: &'static str,
    pub caution: &'static str,
    pub reveal_spoiler: &'static str,
}

impl MarkdownLocale {
    pub const fn en() -> Self {
        Self {
            copy: "Copy",
            copied: "Copied",
            note: "Note",
            tip: "Tip",
            important: "Important",
            warning: "Warning",
            caution: "Caution",
            reveal_spoiler: "Show spoiler",
        }
    }

    pub const fn zh_cn() -> Self {
        Self {
            copy: "复制",
            copied: "已复制",
            note: "备注",
            tip: "提示",
            important: "重要",
            warning: "警告",
            caution: "注意",
            reveal_spoiler: "显示剧透",
        }
    }

    pub(crate) fn get(build: &BuildCx<'_>) -> Self {
        build.context().unwrap_or_default()
    }
}

impl Default for MarkdownLocale {
    fn default() -> Self {
        Self::en()
    }
}
