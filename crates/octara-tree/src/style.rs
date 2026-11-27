//! Style resolution for authored declarations, inherited values, and viewport state.

mod intern;
pub(crate) mod sensitivity;
mod tree;
#[doc(hidden)]
pub use intern::ResolvedStyle as ComputedStyle;
pub(crate) use intern::StyleInterner;

#[cfg(test)]
mod kernel_tests;

use octara_model::{
    geometry::Size,
    input::CursorStyle,
    motion::Motion,
    style::{
        Direction, FontSize, Inherited, LayoutStyle, Length, LineHeight, Radius, Ring, Shadows, State, StyleProp, UserSelect, Visual, When,
    },
    text::{TextAlign, Wrap},
    theme::{
        Breakpoint, ColorScheme, ColorToken, ColorValue, Family, Fill, FontSizeToken, Gradient, ResponsiveBreakpoint, TextScale, Theme,
    },
};
use smallvec::SmallVec;

fn apply(prop: StyleProp, layout: &mut LayoutStyle, visual: &mut Unresolved, inline: &mut SmallVec<[StyleProp; 8]>, theme: &Theme) {
    match prop {
        StyleProp::Display(d) => layout.display = d,
        StyleProp::Direction(d) => layout.flex_direction = d,
        StyleProp::TextDirection(d) => visual.direction = d,
        StyleProp::FlexWrap(w) => layout.flex_wrap = w,
        StyleProp::AlignItems(a) => layout.align_items = a,
        StyleProp::AlignSelf(a) => layout.align_self = a,
        StyleProp::Justify(j) => layout.justify_content = j,
        StyleProp::Gap(g) => {
            let gap = g.resolved(theme).unwrap_or_default();
            layout.gap = Size { width: gap, height: gap };
        }
        StyleProp::GapX(g) => layout.gap.width = g.resolved(theme).unwrap_or_default(),
        StyleProp::GapY(g) => layout.gap.height = g.resolved(theme).unwrap_or_default(),
        StyleProp::PaddingTop(p) => layout.padding.top = p.resolved(theme).unwrap_or_default(),
        StyleProp::PaddingBottom(p) => layout.padding.bottom = p.resolved(theme).unwrap_or_default(),
        StyleProp::MarginTop(m) => layout.margin.top = m.resolved(theme),
        StyleProp::MarginBottom(m) => layout.margin.bottom = m.resolved(theme),
        StyleProp::Width(d) => layout.size.width = d.resolved(theme),
        StyleProp::Height(d) => layout.size.height = d.resolved(theme),
        StyleProp::MinWidth(d) => layout.min_size.width = d.resolved(theme),
        StyleProp::MinHeight(d) => layout.min_size.height = d.resolved(theme),
        StyleProp::MaxWidth(d) => layout.max_size.width = d.resolved(theme),
        StyleProp::MaxHeight(d) => layout.max_size.height = d.resolved(theme),
        StyleProp::Basis(d) => layout.flex_basis = d.resolved(theme),
        StyleProp::AspectRatio(r) => layout.aspect_ratio = r,
        StyleProp::Position(p) => layout.position = p,
        StyleProp::GridCols(n) => layout.grid_columns = n,
        StyleProp::GridRows(n) => layout.grid_rows = n,
        StyleProp::ColStart(p) => layout.grid_column.start = p,
        StyleProp::ColEnd(p) => layout.grid_column.end = p,
        StyleProp::RowStart(p) => layout.grid_row.start = p,
        StyleProp::RowEnd(p) => layout.grid_row.end = p,
        StyleProp::OverflowX(o) => layout.overflow_x = o,
        StyleProp::OverflowY(o) => layout.overflow_y = o,
        StyleProp::Scrollbar(visibility) => visual.scrollbar = visibility,
        StyleProp::Top(d) => layout.inset.top = d.resolved(theme),
        StyleProp::Bottom(d) => layout.inset.bottom = d.resolved(theme),
        StyleProp::Grow(g) => layout.flex_grow = g,
        StyleProp::Shrink(s) => layout.flex_shrink = s,
        StyleProp::BorderTop(w) => layout.border.top = w.resolved(theme).unwrap_or_default(),
        StyleProp::BorderBottom(w) => layout.border.bottom = w.resolved(theme).unwrap_or_default(),
        StyleProp::PaddingLeft(_)
        | StyleProp::PaddingRight(_)
        | StyleProp::PaddingStart(_)
        | StyleProp::PaddingEnd(_)
        | StyleProp::MarginLeft(_)
        | StyleProp::MarginRight(_)
        | StyleProp::MarginStart(_)
        | StyleProp::MarginEnd(_)
        | StyleProp::Left(_)
        | StyleProp::Right(_)
        | StyleProp::InsetStart(_)
        | StyleProp::InsetEnd(_)
        | StyleProp::BorderLeft(_)
        | StyleProp::BorderRight(_)
        | StyleProp::BorderStart(_)
        | StyleProp::BorderEnd(_) => inline.push(prop),
        StyleProp::Background(p) => visual.background = p,
        StyleProp::TextColor(p) => visual.text = p,
        StyleProp::BorderColor(p) => visual.border = p,
        StyleProp::RingWidth(w) => visual.ring_width = nonnegative_px(w, theme),
        StyleProp::RingOffset(o) => visual.ring_offset = nonnegative_px(o, theme),
        StyleProp::RingColor(p) => visual.ring_color = Some(p),
        StyleProp::RadiusTopLeft(r) => visual.radius[0] = radius(r, theme),
        StyleProp::RadiusTopRight(r) => visual.radius[1] = radius(r, theme),
        StyleProp::RadiusBottomRight(r) => visual.radius[2] = radius(r, theme),
        StyleProp::RadiusBottomLeft(r) => visual.radius[3] = radius(r, theme),
        StyleProp::ZIndex(index) => visual.z_index = index,
        StyleProp::FontSize(FontSize::Len(n)) if n.px(theme).is_none() => {}
        StyleProp::FontSize(size) => visual.font_size = size,
        StyleProp::LineHeight(h) => visual.line_height = Some(h),
        StyleProp::FontWeight(w) => visual.font_weight = w,
        StyleProp::FontFamily(f) => visual.family = f,
        StyleProp::Italic(italic) => visual.italic = italic,
        StyleProp::Underline(underline) => visual.underline = underline,
        StyleProp::Strike(strike) => visual.strike = strike,
        StyleProp::LetterSpacing(px) => visual.letter_spacing = LetterSpacing::Px(px),
        StyleProp::LetterSpacingEm(em) => visual.letter_spacing = LetterSpacing::Em(em),
        StyleProp::TabularNums(tabular_nums) => visual.tabular_nums = tabular_nums,
        StyleProp::TextWrap(w) => visual.wrap = w,
        StyleProp::MaxLines(n) => visual.max_lines = n,
        StyleProp::TextAlign(a) => visual.align = a,
        StyleProp::Opacity(o) => visual.opacity = o,
        StyleProp::Shadow(s) => visual.shadow = s,
        StyleProp::PointerEvents(enabled) => visual.pointer_events = Some(enabled),
        StyleProp::UserSelect(selection) => visual.user_select = Some(selection),
        StyleProp::Cursor(cursor) => visual.cursor = Some(cursor),
        StyleProp::Motion(m) => visual.motion = m,
        StyleProp::ColorMotion(c) => visual.motion.color = c,
        StyleProp::OpacityMotion(c) => visual.motion.opacity = c,
        StyleProp::LayoutMotion(c) => visual.motion.layout = c,
    }
}

// Logical and physical declarations share edge slots once the cascade has chosen a direction.
fn apply_inline(prop: StyleProp, rtl: bool, layout: &mut LayoutStyle, theme: &Theme) {
    use StyleProp::*;
    match (prop, rtl) {
        (PaddingLeft(p), _) | (PaddingStart(p), false) | (PaddingEnd(p), true) => {
            layout.padding.left = p.resolved(theme).unwrap_or_default();
        }
        (PaddingRight(p), _) | (PaddingEnd(p), false) | (PaddingStart(p), true) => {
            layout.padding.right = p.resolved(theme).unwrap_or_default();
        }
        (MarginLeft(m), _) | (MarginStart(m), false) | (MarginEnd(m), true) => layout.margin.left = m.resolved(theme),
        (MarginRight(m), _) | (MarginEnd(m), false) | (MarginStart(m), true) => layout.margin.right = m.resolved(theme),
        (Left(d), _) | (InsetStart(d), false) | (InsetEnd(d), true) => layout.inset.left = d.resolved(theme),
        (Right(d), _) | (InsetEnd(d), false) | (InsetStart(d), true) => layout.inset.right = d.resolved(theme),
        (BorderLeft(w), _) | (BorderStart(w), false) | (BorderEnd(w), true) => {
            layout.border.left = w.resolved(theme).unwrap_or_default();
        }
        (BorderRight(w), _) | (BorderEnd(w), false) | (BorderStart(w), true) => {
            layout.border.right = w.resolved(theme).unwrap_or_default();
        }
        _ => unreachable!("only inline edges are deferred"),
    }
}

fn nonnegative_px(length: Length, theme: &Theme) -> f32 {
    length.px(theme).filter(|value| value.is_finite()).unwrap_or_default().max(0.0)
}

#[derive(Clone, Copy)]
enum LetterSpacing {
    Px(f32),
    Em(f32),
}

impl LetterSpacing {
    fn resolve(self, font_size: f32) -> f32 {
        let spacing = match self {
            Self::Px(px) => px,
            Self::Em(em) => em * font_size,
        };
        if spacing.is_finite() { spacing } else { 0.0 }
    }
}

fn radius(radius: Radius, theme: &Theme) -> f32 {
    match radius {
        Radius::Px(pixels) => pixels.max(0.0),
        Radius::Token(token) => theme.radius_of(token).max(0.0),
        Radius::Full => f32::INFINITY,
    }
}

/// Visual properties with colors still as tokens (lengths are already in pixels).
#[derive(Clone, Copy)]
struct Unresolved {
    background: Option<Fill>,
    text: ColorValue,
    border: ColorValue,
    ring_width: f32,
    ring_offset: f32,
    ring_color: Option<ColorValue>,
    radius: [f32; 4],
    z_index: i32,
    direction: Direction,
    text_scale: f32,
    font_size: FontSize,
    line_height: Option<LineHeight>,
    font_weight: u16,
    family: Family,
    italic: bool,
    underline: bool,
    strike: bool,
    letter_spacing: LetterSpacing,
    tabular_nums: bool,
    wrap: Wrap,
    align: TextAlign,
    max_lines: Option<u16>,
    opacity: f32,
    shadow: Shadows,
    pointer_events: Option<bool>,
    scrollbar: octara_model::style::ScrollbarVisibility,
    user_select: Option<UserSelect>,
    cursor: Option<CursorStyle>,
    motion: Motion,
}

impl Unresolved {
    /// Starts from what the parent passes down.
    fn new(from: &Inherited) -> Self {
        Unresolved {
            background: None,
            text: from.text,
            border: ColorValue::Token(ColorToken::Border),
            ring_width: 0.0,
            ring_offset: 0.0,
            ring_color: None,
            radius: [0.0; 4],
            z_index: 0,
            direction: from.direction,
            text_scale: from.text_scale,
            font_size: from.font_size,
            line_height: from.line_height,
            font_weight: from.font_weight,
            family: from.family,
            italic: from.italic,
            underline: from.underline,
            strike: from.strike,
            letter_spacing: LetterSpacing::Px(from.letter_spacing),
            tabular_nums: from.tabular_nums,
            wrap: from.wrap,
            align: from.align,
            max_lines: from.max_lines,
            opacity: 1.0,
            shadow: Shadows::None,
            pointer_events: None,
            scrollbar: Default::default(),
            user_select: None,
            cursor: None,
            motion: from.motion,
        }
    }

    /// What this node passes down to its children.
    fn inherited(&self, letter_spacing: f32) -> Inherited {
        Inherited {
            direction: self.direction,
            text_scale: self.text_scale,
            text: self.text,
            font_size: self.font_size,
            line_height: self.line_height,
            font_weight: self.font_weight,
            family: self.family,
            italic: self.italic,
            underline: self.underline,
            strike: self.strike,
            letter_spacing,
            tabular_nums: self.tabular_nums,
            wrap: self.wrap,
            align: self.align,
            max_lines: self.max_lines,
            motion: self.motion,
        }
    }

    /// The visual, and custom tokens the theme lacks (shown as `MISSING`).
    fn resolve(&self, theme: &Theme, missing: &mut Vec<&'static str>) -> (Visual, Inherited) {
        let mut paint = |p: ColorValue| {
            let (c, m) = p.resolve(theme);
            missing.extend(m);
            c
        };
        let scale = match self.font_size {
            FontSize::Token(token) => theme.text_scale_of(token),
            FontSize::Len(length) => length
                .px(theme)
                .map_or_else(|| theme.text_scale_of(FontSizeToken::Base), |size| TextScale::new(size, size * theme.line_height)),
        };
        let line_height = match self.line_height {
            Some(LineHeight::Len(length)) => length.px(theme).unwrap_or(scale.line_height),
            Some(LineHeight::Relative(ratio)) => ratio * scale.size,
            None => scale.line_height,
        };
        let (background, gradient) = match self.background {
            None => (None, None),
            Some(Fill::Solid(p)) => (Some(paint(p)), None),
            Some(Fill::Linear(g)) => {
                let from = (paint(g.from.color), g.from.percentage);
                let to = (paint(g.to.color), g.to.percentage);
                (None, Some(Gradient { angle: g.angle, from, to }))
            }
        };
        let text = paint(self.text);
        let ring = (self.ring_width > 0.0 && (self.ring_width + self.ring_offset).is_finite()).then(|| Ring {
            width: self.ring_width,
            offset: self.ring_offset,
            color: self.ring_color.map_or(text, &mut paint),
        });
        let letter_spacing = self.letter_spacing.resolve(scale.size);
        let visual = Visual {
            background,
            gradient,
            text,
            border: paint(self.border),
            ring,
            radius: self.radius,
            z_index: self.z_index,
            direction: self.direction,
            text_scale: self.text_scale,
            font_size: scale.size * self.text_scale,
            line_height: line_height * self.text_scale,
            font_weight: self.font_weight,
            font_family: theme.family(self.family),
            italic: self.italic,
            underline: self.underline,
            strike: self.strike,
            letter_spacing: letter_spacing * self.text_scale,
            tabular_nums: self.tabular_nums,
            wrap: self.wrap,
            align: self.align,
            max_lines: self.max_lines,
            opacity: self.opacity,
            shadow: self.shadow,
            pointer_events: self.pointer_events,
            scrollbar: self.scrollbar,
            user_select: self.user_select,
            cursor: self.cursor,
            motion: self.motion,
        };
        (visual, self.inherited(letter_spacing))
    }
}

/// Resolves declarations using a single theme and viewport context.
pub struct StyleResolver<'a> {
    theme: &'a Theme,
    breakpoint: Breakpoint,
    responsive_breakpoint: Option<ResponsiveBreakpoint>,
    touch: bool,
}

pub struct ResolvedStyle {
    pub layout: LayoutStyle,
    pub visual: Visual,
    pub inherited: Inherited,
    pub opacity_motion_authored: bool,
    pub missing_tokens: Vec<&'static str>,
}

impl<'a> StyleResolver<'a> {
    pub fn new(theme: &'a Theme, breakpoint: Breakpoint, responsive_breakpoint: Option<ResponsiveBreakpoint>, touch: bool) -> Self {
        Self { theme, breakpoint, responsive_breakpoint, touch }
    }

    pub fn initial(theme: &Theme) -> (Inherited, Visual) {
        let inherited = Inherited::root();
        (inherited, Self::initial_visual(theme, &inherited))
    }

    fn initial_visual(theme: &Theme, inherited: &Inherited) -> Visual {
        Unresolved::new(inherited).resolve(theme, &mut Vec::new()).0
    }

    /// Repeated declarations apply in authored order within each precedence level.
    pub fn resolve(
        &self,
        mut layout: LayoutStyle,
        from: &Inherited,
        base: &[StyleProp],
        overrides: &[(When, Vec<StyleProp>)],
        state: State,
        group_hovered: impl Fn(&str) -> bool,
    ) -> ResolvedStyle {
        let mut visual = Unresolved::new(from);
        let mut inline = SmallVec::new();
        let mut opacity_motion_authored = false;
        let mut apply_props = |props: &[StyleProp]| {
            for &prop in props {
                opacity_motion_authored |= matches!(prop, StyleProp::Motion(_) | StyleProp::OpacityMotion(_));
                apply(prop, &mut layout, &mut visual, &mut inline, self.theme);
            }
        };
        apply_props(base);
        let mut apply_when = |when| {
            for (_, props) in overrides.iter().filter(|(condition, _)| *condition == when) {
                apply_props(props);
            }
        };
        for breakpoint in Breakpoint::ALL.into_iter().filter(|breakpoint| *breakpoint <= self.breakpoint) {
            apply_when(When::Screen(breakpoint));
        }
        if let Some(max) = self.responsive_breakpoint {
            for breakpoint in ResponsiveBreakpoint::ALL.into_iter().take_while(|breakpoint| *breakpoint <= max) {
                apply_when(When::Responsive(breakpoint));
            }
        }
        if self.touch {
            apply_when(When::Touch);
        }
        apply_when(match self.theme.scheme {
            ColorScheme::Light => When::Light,
            ColorScheme::Dark => When::Dark,
        });
        for flag in state.iter() {
            apply_when(When::State(flag));
        }
        for (when, props) in overrides {
            if let When::GroupHover(group) = when
                && group_hovered(group)
            {
                apply_props(props);
            }
        }
        if visual.direction == Direction::Inherit {
            visual.direction = from.direction;
        }
        for &prop in &inline {
            apply_inline(prop, visual.direction == Direction::Rtl, &mut layout, self.theme);
        }
        let mut missing_tokens = Vec::new();
        let (visual, inherited) = visual.resolve(self.theme, &mut missing_tokens);
        ResolvedStyle { layout, visual, inherited, opacity_motion_authored, missing_tokens }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use octara_model::style::LayoutLength;

    #[test]
    fn resolves_override_order_and_passes_text_style_to_descendants() {
        let theme = Theme::light();
        let root = Inherited::root();
        let resolver = StyleResolver::new(&theme, Breakpoint::Expanded, None, true);
        let base = [StyleProp::Width(20.into()), StyleProp::TextAlign(TextAlign::Center)];
        let overrides = [
            (When::GroupHover("row"), vec![StyleProp::Width(60.into())]),
            (When::State(State::PRESSED), vec![StyleProp::Width(50.into())]),
            (When::Touch, vec![StyleProp::Width(40.into())]),
            (When::Screen(Breakpoint::Medium), vec![StyleProp::Width(30.into())]),
        ];

        let parent = resolver.resolve(LayoutStyle::default(), &root, &base, &overrides, State::PRESSED, |_| false);
        assert_eq!(parent.layout.size.width, Some(LayoutLength::Px(50.0)));
        assert_eq!(parent.visual.align, TextAlign::Center);

        let child = resolver.resolve(LayoutStyle::default(), &parent.inherited, &[], &overrides, State::PRESSED, |group| group == "row");
        assert_eq!(child.layout.size.width, Some(LayoutLength::Px(60.0)));
        assert_eq!(child.visual.text_style().align, TextAlign::Center);
    }

    #[test]
    fn logical_edges_follow_the_final_direction_and_cascade_order() {
        use StyleProp::*;
        use octara_model::style::Direction::{Inherit, Ltr, Rtl};

        let theme = Theme::light();
        let resolver = StyleResolver::new(&theme, Breakpoint::Expanded, None, false);
        let base = [
            MarginLeft(1.into()),
            MarginEnd(2.into()),
            PaddingStart(3.into()),
            PaddingRight(4.into()),
            InsetStart(Length::Pct(0.2)),
            InsetEnd(5.into()),
            BorderStart(6.into()),
            BorderEnd(7.into()),
            TextDirection(Ltr),
        ];
        let overrides = [(When::State(State::HOVER), vec![MarginStart(Length::Auto), BorderRight(8.into()), TextDirection(Inherit)])];
        let root = Inherited { direction: Rtl, ..Inherited::root() };
        let resolve = |state| resolver.resolve(LayoutStyle::default(), &root, &base, &overrides, state, |_| false);
        let px = LayoutLength::Px;
        let ltr = resolve(State::empty());
        assert_eq!(ltr.visual.direction, Ltr);
        assert_eq!((ltr.layout.margin.left, ltr.layout.margin.right), (Some(px(1.0)), Some(px(2.0))));
        assert_eq!((ltr.layout.padding.left, ltr.layout.padding.right), (px(3.0), px(4.0)));
        assert_eq!((ltr.layout.inset.left, ltr.layout.inset.right), (Some(LayoutLength::Pct(0.2)), Some(px(5.0))));
        assert_eq!((ltr.layout.border.left, ltr.layout.border.right), (px(6.0), px(7.0)));

        let rtl = resolve(State::HOVER);
        assert_eq!(rtl.visual.direction, Rtl);
        assert_eq!((rtl.layout.margin.left, rtl.layout.margin.right), (Some(px(2.0)), None));
        assert_eq!((rtl.layout.padding.left, rtl.layout.padding.right), (px(0.0), px(4.0)));
        assert_eq!((rtl.layout.inset.left, rtl.layout.inset.right), (Some(px(5.0)), Some(LayoutLength::Pct(0.2))));
        assert_eq!((rtl.layout.border.left, rtl.layout.border.right), (px(7.0), px(8.0)));
    }

    #[test]
    fn resolves_ring_colors_and_metrics_in_the_effective_theme() {
        let theme = Theme::dark().spacing(6.0);
        let resolver = StyleResolver::new(&theme, Breakpoint::Expanded, None, false);
        let root = Inherited::root();
        let width = StyleProp::RingWidth(Length::Spacing(0.5));
        let color = StyleProp::RingColor(ColorToken::Focus.alpha(0.5));
        let offset = StyleProp::RingOffset(Length::Px(-2.0));
        let ring = resolver.resolve(LayoutStyle::default(), &root, &[width, color, offset], &[], State::empty(), |_| false);
        assert_eq!(ring.visual.ring, Some(Ring { width: 3.0, offset: 0.0, color: theme.get(ColorToken::Focus).with_alpha(0.5) }));
        assert_eq!(ring.layout, LayoutStyle::default());
        let absent = resolver.resolve(
            LayoutStyle::default(),
            &root,
            &[StyleProp::RingWidth(Length::Px(f32::INFINITY)), color],
            &[],
            State::empty(),
            |_| false,
        );
        assert_eq!(absent.visual.ring, None);
    }
}
