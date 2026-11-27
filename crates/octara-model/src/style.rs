//! Authored and resolved style values shared by the tree and hosts.

use crate::geometry::{Align, Edges, Size};
use crate::input::CursorStyle;
use crate::motion::{Curve, Motion};
use crate::text::{TextAlign, TextStyle, Wrap};
use crate::theme::{
    BoxShadow, Breakpoint, Color, ColorToken, ColorValue, ContainerWidthToken, Family, Fill, FontSizeToken, Gradient, LeadingToken,
    RadiusToken, ResponsiveBreakpoint, ShadowToken, Theme,
};
use bitflags::bitflags;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Display {
    None,
    #[default]
    Flex,
    Grid,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FlexDirection {
    #[default]
    Row,
    RowReverse,
    Column,
    ColumnReverse,
}

/// Inline direction for text and logical spacing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    #[default]
    Inherit,
    Ltr,
    Rtl,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FlexWrap {
    #[default]
    NoWrap,
    Wrap,
    WrapReverse,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AlignItems {
    #[default]
    Normal,
    Start,
    End,
    FlexStart,
    FlexEnd,
    SelfStart,
    SelfEnd,
    Center,
    Baseline,
    Stretch,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum JustifyContent {
    #[default]
    Normal,
    Start,
    End,
    FlexStart,
    FlexEnd,
    Center,
    Stretch,
    SpaceBetween,
    SpaceEvenly,
    SpaceAround,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Position {
    #[default]
    Static,
    Relative,
    Absolute,
    Fixed,
    Sticky,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Overflow {
    #[default]
    Visible,
    Clip,
    Hidden,
    Auto,
    Scroll,
}

impl Overflow {
    pub const fn scrolls(self) -> bool {
        matches!(self, Self::Auto | Self::Scroll)
    }
}

/// Visibility of a scrollport's overlay scrollbars.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ScrollbarVisibility {
    /// Show during scroll, near the edges, and while dragging.
    #[default]
    Auto,
    Always,
    Hidden,
}

/// Where a grid item starts or ends on an axis (CSS `grid-column-start` …).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum GridPos {
    #[default]
    Auto,
    /// A line, 1-based. Negative counts from the end (`-1` is the last line).
    Line(i16),
    /// Span this many tracks.
    Span(u16),
}

/// A length with theme units resolved. A percentage is expressed as a fraction of its basis.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum LayoutLength {
    Px(f32),
    Pct(f32),
}

impl Default for LayoutLength {
    fn default() -> Self {
        Self::Px(0.0)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GridRange {
    pub start: GridPos,
    pub end: GridPos,
}

/// The resolved layout declaration of a retained node. Optional lengths use `None` for `auto`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LayoutStyle {
    pub display: Display,
    pub position: Position,
    pub size: Size<Option<LayoutLength>>,
    pub min_size: Size<Option<LayoutLength>>,
    pub max_size: Size<Option<LayoutLength>>,
    pub aspect_ratio: Option<f32>,
    pub padding: Edges<LayoutLength>,
    pub margin: Edges<Option<LayoutLength>>,
    pub inset: Edges<Option<LayoutLength>>,
    pub border: Edges<LayoutLength>,
    pub gap: Size<LayoutLength>,
    pub align_items: AlignItems,
    pub align_self: Option<AlignItems>,
    pub justify_content: JustifyContent,
    pub flex_direction: FlexDirection,
    pub flex_wrap: FlexWrap,
    pub flex_grow: f32,
    pub flex_shrink: f32,
    pub flex_basis: Option<LayoutLength>,
    pub overflow_x: Overflow,
    pub overflow_y: Overflow,
    /// Equal-sized tracks: `repeat(n, minmax(0, 1fr))`.
    pub grid_columns: u16,
    pub grid_rows: u16,
    pub grid_column: GridRange,
    pub grid_row: GridRange,
}

impl Default for LayoutStyle {
    fn default() -> Self {
        let zero = Some(LayoutLength::Px(0.0));
        Self {
            display: Display::default(),
            position: Position::default(),
            size: Size::default(),
            min_size: Size::default(),
            max_size: Size::default(),
            aspect_ratio: None,
            padding: Edges::default(),
            margin: Edges { top: zero, right: zero, bottom: zero, left: zero },
            inset: Edges::default(),
            border: Edges::default(),
            gap: Size::default(),
            align_items: AlignItems::default(),
            align_self: None,
            justify_content: JustifyContent::default(),
            flex_direction: FlexDirection::default(),
            flex_wrap: FlexWrap::default(),
            flex_grow: 0.0,
            flex_shrink: 1.0,
            flex_basis: None,
            overflow_x: Overflow::default(),
            overflow_y: Overflow::default(),
            grid_columns: 0,
            grid_rows: 0,
            grid_column: GridRange::default(),
            grid_row: GridRange::default(),
        }
    }
}

impl LayoutStyle {
    /// How an anonymous text item in a flex leaf sits along the vertical axis.
    pub fn text_block_align(&self) -> Align {
        if self.display != Display::Flex {
            return Align::Start;
        }
        match self.flex_direction {
            FlexDirection::Row | FlexDirection::RowReverse => match self.align_items {
                AlignItems::Center => Align::Center,
                AlignItems::End | AlignItems::SelfEnd => Align::End,
                AlignItems::FlexStart if self.flex_wrap == FlexWrap::WrapReverse => Align::End,
                AlignItems::FlexEnd if self.flex_wrap != FlexWrap::WrapReverse => Align::End,
                _ => Align::Start,
            },
            FlexDirection::Column | FlexDirection::ColumnReverse => match self.justify_content {
                JustifyContent::Center | JustifyContent::SpaceAround | JustifyContent::SpaceEvenly => Align::Center,
                JustifyContent::End => Align::End,
                JustifyContent::FlexEnd if self.flex_direction == FlexDirection::Column => Align::End,
                JustifyContent::Normal | JustifyContent::FlexStart | JustifyContent::Stretch | JustifyContent::SpaceBetween
                    if self.flex_direction == FlexDirection::ColumnReverse =>
                {
                    Align::End
                }
                _ => Align::Start,
            },
        }
    }
}
bitflags! {
    /// States that select style overrides. Overrides apply in this order: later states win.
    /// `CHECKED` and `EXPANDED` follow the `checked` / `expanded` semantics (like Tailwind's
    /// `aria-checked:`), so a switch can style its thumb with `style_when(State::CHECKED, ..)`.
    #[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default, Serialize, Deserialize)]
    pub struct State: u16 {
        const HOVER = 1 << 0;
        /// This node or one of its descendants has focus.
        const FOCUS_WITHIN = 1 << 1;
        /// Has focus, however it got there.
        const FOCUSED = 1 << 2;
        /// Has focus and the last input was the keyboard: the focus ring shows.
        const FOCUS_VISIBLE = 1 << 3;
        const PRESSED = 1 << 4;
        const SELECTED = 1 << 5;
        const CHECKED = 1 << 6;
        const EXPANDED = 1 << 7;
        const DISABLED = 1 << 8;
        /// Files dragged from the system are over it, and it takes them (`on_drop`).
        const DRAG_OVER = 1 << 9;
        /// This node owns an active element drag.
        const DRAGGING = 1 << 10;
        /// This node is the current receiver for an element drag.
        const DROP_HOVER = 1 << 11;
    }
}

impl State {
    /// States set by the tree's focus reconciliation.
    pub const TREE_DERIVED: Self = Self::FOCUSED.union(Self::FOCUS_VISIBLE).union(Self::FOCUS_WITHIN);
}

/// When an override applies.
/// Screen and responsive overrides apply in ascending order, followed by touch, color scheme, node states, and group hover.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum When {
    Screen(Breakpoint),
    /// From the Tailwind viewport width up.
    Responsive(ResponsiveBreakpoint),
    /// While input is touch (the app runs on iOS or Android): controls take their touch sizes.
    Touch,
    Dark,
    Light,
    State(State),
    /// While an ancestor of this group name (`group`) is hovered (GPUI's `group_hover`).
    GroupHover(&'static str),
}

impl From<State> for When {
    fn from(s: State) -> When {
        When::State(s)
    }
}

impl From<Breakpoint> for When {
    fn from(b: Breakpoint) -> When {
        When::Screen(b)
    }
}

impl From<ResponsiveBreakpoint> for When {
    fn from(breakpoint: ResponsiveBreakpoint) -> Self {
        Self::Responsive(breakpoint)
    }
}

/// A length as written: pixels, a fraction of the parent, automatic, or theme steps.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Length {
    Px(f32),
    /// A fraction of the parent: `Pct(0.5)` is half.
    Pct(f32),
    Auto,
    /// Steps of the theme's spacing unit, for example `p(sp(4.0))`.
    Spacing(f32),
    /// A container width from the theme, for example `max_w(ContainerWidthToken::Md)`.
    Container(ContainerWidthToken),
    /// Multiples of the theme's `rem` (16 px): GPUI's `rems(1.5)`.
    Rems(f32),
}

/// A plain number is pixels: `.p(13.0)` is Tailwind's `p-[13px]`.
impl From<f32> for Length {
    fn from(n: f32) -> Length {
        Length::Px(n)
    }
}
impl From<f64> for Length {
    fn from(n: f64) -> Length {
        Length::Px(n as f32)
    }
}
impl From<i32> for Length {
    fn from(n: i32) -> Length {
        Length::Px(n as f32)
    }
}

impl From<ContainerWidthToken> for Length {
    fn from(step: ContainerWidthToken) -> Self {
        Self::Container(step)
    }
}

impl Length {
    /// Pixels, or `None` for a fraction or `Auto`.
    pub fn px(self, t: &Theme) -> Option<f32> {
        match self {
            Length::Px(n) => Some(n),
            Length::Spacing(n) => Some(n * t.spacing),
            Length::Container(c) => Some(t.container_of(c)),
            Length::Rems(n) => Some(n * t.rem),
            Length::Pct(_) | Length::Auto => None,
        }
    }
    pub fn resolved(self, theme: &Theme) -> Option<LayoutLength> {
        match self {
            Length::Pct(fraction) => Some(LayoutLength::Pct(fraction)),
            value => value.px(theme).map(LayoutLength::Px),
        }
    }
}

/// A corner radius as written.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Radius {
    Px(f32),
    Token(RadiusToken),
    /// A pill or circle sized to the element's bounds (`rounded-full`).
    Full,
}

impl From<f32> for Radius {
    fn from(px: f32) -> Self {
        Self::Px(px)
    }
}

impl From<f64> for Radius {
    fn from(px: f64) -> Self {
        Self::Px(px as f32)
    }
}

impl From<i32> for Radius {
    fn from(px: i32) -> Self {
        Self::Px(px as f32)
    }
}

impl From<RadiusToken> for Radius {
    fn from(step: RadiusToken) -> Self {
        Self::Token(step)
    }
}

/// A font size as written: a theme step or a length.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum FontSize {
    Len(Length),
    Token(FontSizeToken),
}

impl<L: Into<Length>> From<L> for FontSize {
    fn from(length: L) -> Self {
        Self::Len(length.into())
    }
}

impl From<FontSizeToken> for FontSize {
    fn from(step: FontSizeToken) -> Self {
        Self::Token(step)
    }
}

/// A line height as written. Defaults to the font size token's pair or the theme's multiplier.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum LineHeight {
    /// A length (pixels, rems, spacing steps).
    Len(Length),
    /// A multiple of the font size (`line_height(relative(1.5))`).
    Relative(f32),
}

impl From<LeadingToken> for LineHeight {
    fn from(token: LeadingToken) -> Self {
        Self::Relative(token.ratio())
    }
}

/// Shadows as written: none, a theme step, or up to four of the element's own.
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub enum Shadows {
    #[default]
    None,
    Token(ShadowToken),
    Custom([Option<BoxShadow>; 4]),
}

impl Shadows {
    /// Up to four custom shadows, first on top.
    pub fn custom(shadows: impl IntoIterator<Item = BoxShadow>) -> Self {
        let mut list = [None; 4];
        for (slot, shadow) in list.iter_mut().zip(shadows) {
            *slot = Some(shadow);
        }
        Self::Custom(list)
    }

    /// The shadows, first on top (as CSS lists them).
    pub fn resolve(&self, theme: &Theme) -> Vec<BoxShadow> {
        match self {
            Shadows::None => Vec::new(),
            Shadows::Token(s) => theme.shadow_of(*s).to_vec(),
            Shadows::Custom(list) => list.iter().flatten().copied().collect(),
        }
    }
}

impl From<ShadowToken> for Shadows {
    fn from(step: ShadowToken) -> Self {
        Self::Token(step)
    }
}

impl<const N: usize> From<[BoxShadow; N]> for Shadows {
    fn from(shadows: [BoxShadow; N]) -> Self {
        Self::custom(shadows)
    }
}

impl From<Vec<BoxShadow>> for Shadows {
    fn from(shadows: Vec<BoxShadow>) -> Self {
        Self::custom(shadows)
    }
}

/// Whether text in an element can be selected.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum UserSelect {
    None,
    Text,
}

/// One authored style property for a node.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum StyleProp {
    Display(Display),
    Direction(FlexDirection),
    TextDirection(Direction),
    FlexWrap(FlexWrap),
    AlignItems(AlignItems),
    AlignSelf(Option<AlignItems>),
    Justify(JustifyContent),
    /// Row and column gap.
    Gap(Length),
    /// Gap between columns (`gap_x`).
    GapX(Length),
    /// Gap between rows (`gap_y`).
    GapY(Length),
    PaddingTop(Length),
    PaddingRight(Length),
    PaddingBottom(Length),
    PaddingLeft(Length),
    PaddingStart(Length),
    PaddingEnd(Length),
    MarginTop(Length),
    MarginRight(Length),
    MarginBottom(Length),
    MarginLeft(Length),
    MarginStart(Length),
    MarginEnd(Length),
    Width(Length),
    Height(Length),
    MinWidth(Length),
    MinHeight(Length),
    MaxWidth(Length),
    MaxHeight(Length),
    Basis(Length),
    AspectRatio(Option<f32>),
    Position(Position),
    /// `n` equal columns (`grid_cols`).
    GridCols(u16),
    /// `n` equal rows (`grid_rows`).
    GridRows(u16),
    ColStart(GridPos),
    ColEnd(GridPos),
    RowStart(GridPos),
    RowEnd(GridPos),
    /// Horizontal overflow behavior. Scrolling uses overlay scrollbars.
    OverflowX(Overflow),
    OverflowY(Overflow),
    Scrollbar(ScrollbarVisibility),
    Top(Length),
    Right(Length),
    Bottom(Length),
    Left(Length),
    InsetStart(Length),
    InsetEnd(Length),
    Grow(f32),
    Shrink(f32),
    BorderTop(Length),
    BorderRight(Length),
    BorderBottom(Length),
    BorderLeft(Length),
    BorderStart(Length),
    BorderEnd(Length),
    Background(Option<Fill>),
    TextColor(ColorValue),
    BorderColor(ColorValue),
    RingWidth(Length),
    RingOffset(Length),
    RingColor(ColorValue),
    RadiusTopLeft(Radius),
    RadiusTopRight(Radius),
    RadiusBottomRight(Radius),
    RadiusBottomLeft(Radius),
    ZIndex(i32),
    FontSize(FontSize),
    LineHeight(LineHeight),
    FontWeight(u16),
    FontFamily(Family),
    Italic(bool),
    Underline(bool),
    Strike(bool),
    /// Additional tracking in logical pixels.
    LetterSpacing(f32),
    /// Additional tracking relative to the resolved font size.
    LetterSpacingEm(f32),
    TabularNums(bool),
    TextWrap(Wrap),
    TextAlign(TextAlign),
    MaxLines(Option<u16>),
    Opacity(f32),
    Shadow(Shadows),
    /// Whether this node can be the direct hit target. Descendants can still be hit.
    PointerEvents(bool),
    UserSelect(UserSelect),
    Cursor(CursorStyle),
    /// How changes move here and below (`motion`).
    Motion(Motion),
    ColorMotion(Curve),
    OpacityMotion(Curve),
    LayoutMotion(Curve),
}

impl StyleProp {
    #[doc(hidden)]
    pub fn same_slot(&self, other: &StyleProp) -> bool {
        std::mem::discriminant(self) == std::mem::discriminant(other)
            || matches!(
                (self, other),
                (Self::LetterSpacing(_), Self::LetterSpacingEm(_)) | (Self::LetterSpacingEm(_), Self::LetterSpacing(_))
            )
    }
}

/// An outer stroke and its gap from the border box, both in logical pixels.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Ring {
    pub width: f32,
    pub offset: f32,
    pub color: Color,
}

/// Resolved appearance and interaction properties.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Visual {
    pub background: Option<Color>,
    pub text: Color,
    pub border: Color,
    pub ring: Option<Ring>,
    /// Top-left, top-right, bottom-right, bottom-left in logical pixels.
    pub radius: [f32; 4],
    pub z_index: i32,
    pub direction: Direction,
    pub text_scale: f32,
    pub font_size: f32,
    pub line_height: f32,
    pub font_weight: u16,
    pub font_family: &'static str,
    pub italic: bool,
    pub underline: bool,
    pub strike: bool,
    pub letter_spacing: f32,
    pub tabular_nums: bool,
    pub wrap: Wrap,
    pub align: TextAlign,
    pub max_lines: Option<u16>,
    pub opacity: f32,
    pub shadow: Shadows,
    pub pointer_events: Option<bool>,
    pub scrollbar: ScrollbarVisibility,
    pub user_select: Option<UserSelect>,
    pub cursor: Option<CursorStyle>,
    /// A gradient background (then `background` is `None`).
    pub gradient: Option<Gradient>,
    /// How this node's changes move.
    pub motion: Motion,
}

impl Visual {
    pub fn text_style(&self) -> TextStyle {
        TextStyle {
            direction: self.direction,
            text_scale: self.text_scale,
            font_size: self.font_size,
            line_height: self.line_height,
            weight: self.font_weight,
            family: self.font_family,
            italic: self.italic,
            underline: self.underline,
            strike: self.strike,
            letter_spacing: self.letter_spacing,
            tabular_nums: self.tabular_nums,
            wrap: self.wrap,
            align: self.align,
            max_lines: self.max_lines,
        }
    }
}

/// Text and motion properties a node passes down to its descendants.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Inherited {
    pub direction: Direction,
    pub text_scale: f32,
    pub text: ColorValue,
    pub font_size: FontSize,
    pub line_height: Option<LineHeight>,
    pub font_weight: u16,
    pub family: Family,
    pub italic: bool,
    pub underline: bool,
    pub strike: bool,
    pub letter_spacing: f32,
    pub tabular_nums: bool,
    pub wrap: Wrap,
    pub align: TextAlign,
    pub max_lines: Option<u16>,
    pub motion: Motion,
}

impl Inherited {
    /// What the window passes down: the theme's defaults.
    pub fn root() -> Self {
        Inherited {
            direction: Direction::Ltr,
            text_scale: 1.0,
            text: ColorValue::Token(ColorToken::Text),
            font_size: FontSize::Token(FontSizeToken::Base),
            line_height: None,
            font_weight: 400,
            family: Family::Sans,
            italic: false,
            underline: false,
            strike: false,
            letter_spacing: 0.0,
            tabular_nums: false,
            wrap: Wrap::Words,
            align: TextAlign::default(),
            max_lines: None,
            motion: Motion::NONE,
        }
    }
}
