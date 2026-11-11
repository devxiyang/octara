//! Style properties as written (lengths in theme steps or pixels, colors as tokens) and their
//! resolution against a [`Theme`] into Taffy layout and a [`Visual`].

use super::text::{TextStyle, Wrap};
use super::theme::{Breakpoint, Color, ContainerStep, Family, Fill, Paint, RadiusStep, TextStep, Theme, Token};
use bitflags::bitflags;
use taffy::{
    AlignItems, Dimension, Display, FlexDirection, FlexWrap, JustifyContent, LengthPercentage, LengthPercentageAuto, Overflow, Position,
};

bitflags! {
    /// Invalidation categories.
    #[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
    pub struct Dirty: u8 {
        const LAYOUT = 1;
        const PAINT = 2;
        const TEXT = 4;
        const SEMANTICS = 8;
    }
}

bitflags! {
    /// States that select style overrides. Overrides apply in this order: later states win.
    /// `CHECKED` and `EXPANDED` follow the `checked` / `expanded` semantics (like Tailwind's
    /// `aria-checked:`), so a switch can style its thumb with `style_when(State::CHECKED, ..)`.
    #[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
    pub struct State: u8 {
        const HOVER = 1;
        /// Has keyboard focus, however it got there.
        const FOCUSED = 2;
        /// Has focus and the last input was the keyboard: the focus ring shows.
        const FOCUS_VISIBLE = 4;
        const PRESSED = 8;
        const SELECTED = 16;
        const CHECKED = 32;
        const EXPANDED = 64;
        const DISABLED = 128;
    }
}

/// When an override applies: from a breakpoint up, or while a state is on. Screen overrides
/// apply first (smallest first), then states, so interaction feedback shows at every size.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum When {
    Screen(Breakpoint),
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

/// A length as written: pixels, a fraction of the parent, automatic, or theme steps.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Length {
    Px(f32),
    /// A fraction of the parent: `Pct(0.5)` is half.
    Pct(f32),
    Auto,
    /// Steps of the theme's spacing unit: `p_4` is `Spacing(4.0)`.
    Spacing(f32),
    /// A container width from the theme (`max_w_md`).
    Container(ContainerStep),
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
    fn lp(self, t: &Theme) -> LengthPercentage {
        match self {
            Length::Pct(f) => LengthPercentage::percent(f),
            l => LengthPercentage::length(l.px(t).unwrap_or(0.0)),
        }
    }
    fn lpa(self, t: &Theme) -> LengthPercentageAuto {
        match self {
            Length::Pct(f) => LengthPercentageAuto::percent(f),
            Length::Auto => LengthPercentageAuto::auto(),
            l => LengthPercentageAuto::length(l.px(t).unwrap_or(0.0)),
        }
    }
    fn dim(self, t: &Theme) -> Dimension {
        match self {
            Length::Pct(f) => Dimension::percent(f),
            Length::Auto => Dimension::auto(),
            l => Dimension::length(l.px(t).unwrap_or(0.0)),
        }
    }
}

/// Where a grid item starts or ends on an axis (CSS `grid-column-start` …).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum GridPos {
    Auto,
    /// A line, 1-based; negative counts from the end (`-1` is the last line).
    Line(i16),
    /// Span this many tracks.
    Span(u16),
}

impl GridPos {
    fn placement(self) -> taffy::GridPlacement {
        match self {
            GridPos::Auto => taffy::GridPlacement::Auto,
            GridPos::Line(i) => taffy::style_helpers::line(i),
            GridPos::Span(n) => taffy::style_helpers::span(n),
        }
    }
}

/// `repeat(n, minmax(0, 1fr))`: `n` equal tracks (Tailwind / GPUI `grid-cols-n`).
fn equal_tracks(n: u16) -> Vec<taffy::GridTemplateComponent<String>> {
    use taffy::style_helpers::{fr, length, minmax, repeat};
    if n == 0 { Vec::new() } else { vec![repeat(n, vec![minmax(length(0.0), fr(1.0))])] }
}

/// A corner radius as written.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Radius {
    Px(f32),
    Step(RadiusStep),
    /// Pill / circle (`rounded-full`).
    Full,
}

/// A font size as written: a theme step or a length.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum FontSize {
    Len(Length),
    Step(TextStep),
}

/// A line height as written. Without one, a text uses the theme's `line_height` × its size.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum LineHeight {
    /// A length (pixels, rems, spacing steps).
    Len(Length),
    /// A multiple of the font size (`line_height(relative(1.5))`).
    Relative(f32),
}

/// A drop shadow (GPUI's `BoxShadow`). As in CSS (and Rune), `blur_radius` is twice the
/// Gaussian's standard deviation; `spread_radius` grows (or, negative, shrinks) the shape.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct BoxShadow {
    pub color: Color,
    pub offset: (f32, f32),
    pub blur_radius: f32,
    pub spread_radius: f32,
}

impl BoxShadow {
    /// A sharp shadow offset by `(x, y)`.
    pub fn new(x: f32, y: f32, color: Color) -> Self {
        BoxShadow { color, offset: (x, y), blur_radius: 0.0, spread_radius: 0.0 }
    }
    pub fn blur_radius(self, r: f32) -> Self {
        BoxShadow { blur_radius: r.max(0.0), ..self }
    }
    pub fn spread_radius(self, r: f32) -> Self {
        BoxShadow { spread_radius: r, ..self }
    }
}

/// Shadows as written: none, a theme step, or up to four of the element's own.
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub enum Shadows {
    #[default]
    None,
    Step(crate::kernel::ShadowStep),
    Custom([Option<BoxShadow>; 4]),
}

impl Shadows {
    /// The shadows, first on top (as CSS lists them).
    pub fn resolve(&self, theme: &Theme) -> Vec<BoxShadow> {
        match self {
            Shadows::None => Vec::new(),
            Shadows::Step(s) => theme.shadow_of(*s).to_vec(),
            Shadows::Custom(list) => list.iter().flatten().copied().collect(),
        }
    }
}

/// How a subtree's changes move (`motion`): one curve for positions and sizes, one for colors
/// (and shadows), one for opacity. Inherited, like text styles.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Motion {
    pub layout: crate::motion::Curve,
    pub color: crate::motion::Curve,
    pub opacity: crate::motion::Curve,
}

impl Motion {
    /// Nothing moves: changes land at once.
    pub const NONE: Motion = Motion { layout: Curve::Instant, color: Curve::Instant, opacity: Curve::Instant };
    /// Everything moves on `curve`.
    pub const fn all(curve: crate::motion::Curve) -> Motion {
        Motion { layout: curve, color: curve, opacity: curve }
    }
    pub fn is_none(&self) -> bool {
        *self == Motion::NONE
    }
}

use crate::motion::Curve;

/// One style property. Layout properties edit the node's `taffy::Style`; the rest its [`Visual`].
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum StyleProp {
    Display(Display),
    Direction(FlexDirection),
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
    MarginTop(Length),
    MarginRight(Length),
    MarginBottom(Length),
    MarginLeft(Length),
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
    /// CSS `overflow-x`; `auto` and `scroll` both scroll (overlay scrollbars take no room).
    OverflowX(Overflow),
    OverflowY(Overflow),
    Top(Length),
    Right(Length),
    Bottom(Length),
    Left(Length),
    Grow(f32),
    Shrink(f32),
    BorderWidth(Length),
    Background(Option<Fill>),
    TextColor(Paint),
    BorderColor(Paint),
    Radius(Radius),
    FontSize(FontSize),
    LineHeight(LineHeight),
    FontWeight(u16),
    FontFamily(Family),
    TextWrap(Wrap),
    MaxLines(Option<u16>),
    Opacity(f32),
    Shadow(Shadows),
    /// How changes move here and below (`motion`).
    Motion(Motion),
    ColorMotion(Curve),
    OpacityMotion(Curve),
    LayoutMotion(Curve),
}

impl StyleProp {
    pub(crate) fn same_slot(&self, other: &StyleProp) -> bool {
        std::mem::discriminant(self) == std::mem::discriminant(other)
    }

    pub(crate) fn apply(&self, l: &mut taffy::Style, v: &mut Unresolved, t: &Theme) {
        match *self {
            StyleProp::Display(d) => l.display = d,
            StyleProp::Direction(d) => l.flex_direction = d,
            StyleProp::FlexWrap(w) => l.flex_wrap = w,
            StyleProp::AlignItems(a) => l.align_items = a,
            StyleProp::AlignSelf(a) => l.align_self = a,
            StyleProp::Justify(j) => l.justify_content = j,
            StyleProp::Gap(g) => l.gap = taffy::Size { width: g.lp(t), height: g.lp(t) },
            StyleProp::GapX(g) => l.gap.width = g.lp(t),
            StyleProp::GapY(g) => l.gap.height = g.lp(t),
            StyleProp::PaddingTop(p) => l.padding.top = p.lp(t),
            StyleProp::PaddingRight(p) => l.padding.right = p.lp(t),
            StyleProp::PaddingBottom(p) => l.padding.bottom = p.lp(t),
            StyleProp::PaddingLeft(p) => l.padding.left = p.lp(t),
            StyleProp::MarginTop(m) => l.margin.top = m.lpa(t),
            StyleProp::MarginRight(m) => l.margin.right = m.lpa(t),
            StyleProp::MarginBottom(m) => l.margin.bottom = m.lpa(t),
            StyleProp::MarginLeft(m) => l.margin.left = m.lpa(t),
            StyleProp::Width(d) => l.size.width = d.dim(t),
            StyleProp::Height(d) => l.size.height = d.dim(t),
            StyleProp::MinWidth(d) => l.min_size.width = d.lpa(t),
            StyleProp::MinHeight(d) => l.min_size.height = d.lpa(t),
            StyleProp::MaxWidth(d) => l.max_size.width = d.lpa(t),
            StyleProp::MaxHeight(d) => l.max_size.height = d.lpa(t),
            StyleProp::Basis(d) => l.flex_basis = d.dim(t),
            StyleProp::AspectRatio(r) => l.aspect_ratio = r,
            StyleProp::Position(p) => l.position = p,
            StyleProp::GridCols(n) => l.grid_template_columns = equal_tracks(n),
            StyleProp::GridRows(n) => l.grid_template_rows = equal_tracks(n),
            StyleProp::ColStart(p) => l.grid_column.start = p.placement(),
            StyleProp::ColEnd(p) => l.grid_column.end = p.placement(),
            StyleProp::RowStart(p) => l.grid_row.start = p.placement(),
            StyleProp::RowEnd(p) => l.grid_row.end = p.placement(),
            StyleProp::OverflowX(o) => l.overflow.x = o,
            StyleProp::OverflowY(o) => l.overflow.y = o,
            StyleProp::Top(d) => l.inset.top = d.lpa(t),
            StyleProp::Right(d) => l.inset.right = d.lpa(t),
            StyleProp::Bottom(d) => l.inset.bottom = d.lpa(t),
            StyleProp::Left(d) => l.inset.left = d.lpa(t),
            StyleProp::Grow(g) => l.flex_grow = g,
            StyleProp::Shrink(s) => l.flex_shrink = s,
            StyleProp::BorderWidth(w) => {
                let w = w.lp(t);
                l.border = taffy::Rect { left: w, right: w, top: w, bottom: w };
            }
            StyleProp::Background(p) => v.background = p,
            StyleProp::TextColor(p) => v.text = p,
            StyleProp::BorderColor(p) => v.border = p,
            StyleProp::Radius(r) => {
                v.radius = match r {
                    Radius::Px(n) => n,
                    Radius::Step(s) => t.radius_of(s),
                    Radius::Full => 9999.0,
                }
            }
            StyleProp::FontSize(FontSize::Len(n)) => v.font_size = n.px(t).unwrap_or(v.font_size),
            StyleProp::FontSize(FontSize::Step(s)) => v.font_size = t.text_of(s),
            StyleProp::LineHeight(h) => v.line_height = Some(h),
            StyleProp::FontWeight(w) => v.font_weight = w,
            StyleProp::FontFamily(f) => v.family = t.family(f),
            StyleProp::TextWrap(w) => v.wrap = w,
            StyleProp::MaxLines(n) => v.max_lines = n,
            StyleProp::Opacity(o) => v.opacity = o,
            StyleProp::Shadow(s) => v.shadow = s,
            StyleProp::Motion(m) => v.motion = m,
            StyleProp::ColorMotion(c) => v.motion.color = c,
            StyleProp::OpacityMotion(c) => v.motion.opacity = c,
            StyleProp::LayoutMotion(c) => v.motion.layout = c,
        }
    }
}

/// Resolved visual properties.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Visual {
    pub background: Option<Color>,
    pub text: Color,
    pub border: Color,
    pub radius: f32,
    pub font_size: f32,
    pub line_height: f32,
    pub font_weight: u16,
    pub font_family: &'static str,
    pub wrap: Wrap,
    pub max_lines: Option<u16>,
    pub opacity: f32,
    pub shadow: Shadows,
    /// A gradient background (then `background` is `None`).
    pub gradient: Option<crate::paint::Gradient>,
    /// How this node's changes move.
    pub motion: Motion,
}

impl Visual {
    pub fn text_style(&self) -> TextStyle {
        TextStyle {
            font_size: self.font_size,
            line_height: self.line_height,
            weight: self.font_weight,
            family: self.font_family,
            wrap: self.wrap,
            max_lines: self.max_lines,
        }
    }
}

/// Text properties a node passes down to its descendants (as GPUI's text style, and CSS):
/// `row().text_sm().text_color(Token::TextMuted).child("note")` styles the text inside.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) struct Inherited {
    text: Paint,
    font_size: f32,
    line_height: Option<LineHeight>,
    font_weight: u16,
    family: &'static str,
    wrap: Wrap,
    max_lines: Option<u16>,
    motion: Motion,
}

impl Inherited {
    /// What the window passes down: the theme's defaults.
    pub(crate) fn root(theme: &Theme) -> Self {
        Inherited {
            text: Paint::Token(Token::Text),
            font_size: theme.text_of(TextStep::Base),
            line_height: None,
            font_weight: 400,
            family: theme.font_sans,
            wrap: Wrap::Words,
            max_lines: None,
            motion: Motion::NONE,
        }
    }
}

/// Visual properties with colors still as tokens (lengths are already in pixels).
#[derive(Clone, Copy)]
pub(crate) struct Unresolved {
    background: Option<Fill>,
    text: Paint,
    border: Paint,
    radius: f32,
    font_size: f32,
    line_height: Option<LineHeight>,
    default_line_height: f32,
    font_weight: u16,
    family: &'static str,
    wrap: Wrap,
    max_lines: Option<u16>,
    opacity: f32,
    shadow: Shadows,
    motion: Motion,
}

impl Unresolved {
    /// Starts from what the parent passes down.
    pub(crate) fn new(theme: &Theme, from: &Inherited) -> Self {
        Unresolved {
            background: None,
            text: from.text,
            border: Paint::Token(Token::Border),
            radius: 0.0,
            font_size: from.font_size,
            line_height: from.line_height,
            default_line_height: theme.line_height,
            font_weight: from.font_weight,
            family: from.family,
            wrap: from.wrap,
            max_lines: from.max_lines,
            opacity: 1.0,
            shadow: Shadows::None,
            motion: from.motion,
        }
    }

    /// What this node passes down to its children.
    pub(crate) fn inherited(&self) -> Inherited {
        Inherited {
            text: self.text,
            font_size: self.font_size,
            line_height: self.line_height,
            font_weight: self.font_weight,
            family: self.family,
            wrap: self.wrap,
            max_lines: self.max_lines,
            motion: self.motion,
        }
    }

    /// The visual, and custom tokens the theme lacks (shown as `MISSING`).
    pub(crate) fn resolve(&self, theme: &Theme, missing: &mut Vec<&'static str>) -> Visual {
        let mut paint = |p: Paint| {
            let (c, m) = p.resolve(theme);
            missing.extend(m);
            c
        };
        let line_height = match self.line_height {
            Some(LineHeight::Len(l)) => l.px(theme).unwrap_or(self.font_size * self.default_line_height),
            Some(LineHeight::Relative(r)) => r * self.font_size,
            None => self.font_size * self.default_line_height,
        };
        let (background, gradient) = match self.background {
            None => (None, None),
            Some(Fill::Solid(p)) => (Some(paint(p)), None),
            Some(Fill::Linear(g)) => {
                let from = (paint(g.from.color), g.from.percentage);
                let to = (paint(g.to.color), g.to.percentage);
                (None, Some(crate::paint::Gradient { angle: g.angle, from, to }))
            }
        };
        Visual {
            background,
            gradient,
            text: paint(self.text),
            border: paint(self.border),
            radius: self.radius,
            font_size: self.font_size,
            line_height,
            font_weight: self.font_weight,
            font_family: self.family,
            wrap: self.wrap,
            max_lines: self.max_lines,
            opacity: self.opacity,
            shadow: self.shadow,
            motion: self.motion,
        }
    }
}
