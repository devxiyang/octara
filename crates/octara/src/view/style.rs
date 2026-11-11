//! Style methods, named as in GPUI (which takes its vocabulary from Tailwind): `p_4`, `gap_2`,
//! `items_center`, `rounded_md`, `text_sm`, `flex_1`, `flex_shrink_0`, `whitespace_nowrap`,
//! `line_clamp`, `overflow_y_scroll`. Half steps and fractions as in GPUI: `p_0p5`, `w_1_2`.
//! Semantics are native, as in Rune: every element is a flex container, boxes do not shrink by
//! default, lengths are logical pixels (points / dp).
//!
//! Any value is written directly: a number is logical pixels (`.p(13.0)`, `.w(320)`),
//! [`relative`] a fraction of the parent, [`rems`] multiples of 16, [`sp`] theme spacing steps.
//!
//! Scales (spacing, radius, text, containers, size classes) and colors come from the [`Theme`]
//! and are resolved by the kernel, so a theme switch (dark mode, density, brand) reruns nothing.
//! The same methods build elements and the overrides given to `hover`, `when_medium`, `style_when` ….
//!
//! [`Theme`]: crate::kernel::Theme

use crate::kernel::BoxShadow;
pub use crate::kernel::Length;
use crate::kernel::text::Wrap;
use crate::kernel::{
    AlignItems, ContainerStep, Display, Family, Fill, FlexDirection, FlexWrap, FontSize, GridPos, JustifyContent, LineHeight, Overflow,
    Paint, Position, Radius, RadiusStep, ShadowStep, Shadows, StyleProp, TextStep,
};

/// `n` logical pixels (a plain number means the same: `.p(px(13.0))` = `.p(13.0)`).
pub const fn px(n: f32) -> Length {
    Length::Px(n)
}

/// A fraction of the parent (GPUI `relative`): `relative(0.5)` is half.
pub const fn relative(f: f32) -> Length {
    Length::Pct(f)
}

/// Multiples of the theme's `rem` (16 px), as in GPUI.
pub const fn rems(n: f32) -> Length {
    Length::Rems(n)
}

/// `n` steps of the theme's spacing unit (`p_4` = `p(sp(4.0))`), for steps without a method.
pub const fn sp(n: f32) -> Length {
    Length::Spacing(n)
}

/// The step encoded in a method name's suffix: `p_4` → 4, `p_0p5` → 0.5, `px_12` → 12.
const fn scale_step(name: &str) -> f32 {
    let b = name.as_bytes();
    let mut i = b.len();
    // The number is the suffix after the last '_'.
    while b[i - 1] != b'_' {
        i -= 1;
    }
    let mut whole = 0u32;
    while i < b.len() && b[i] != b'p' {
        whole = whole * 10 + (b[i] - b'0') as u32;
        i += 1;
    }
    let half = if i < b.len() { 0.5 } else { 0.0 };
    whole as f32 + half
}

/// `$name()` = `$base(Length::Spacing(step))` for each name, the step taken from the name.
macro_rules! steps {
    ($($base:ident => [$($name:ident),* $(,)?]),* $(,)?) => {
        $($(
            #[doc = concat!("`", stringify!($base), "` in theme spacing steps (`", stringify!($name), "`).")]
            fn $name(self) -> Self {
                self.$base(Length::Spacing(const { scale_step(stringify!($name)) }))
            }
        )*)*
    };
}

/// `$name()` = `$base(relative(num / den))`.
macro_rules! fractions {
    ($($base:ident => [$($name:ident = $num:literal / $den:literal),* $(,)?]),* $(,)?) => {
        $($(
            #[doc = concat!("`", stringify!($base), "` of ", stringify!($num), "/", stringify!($den), " of the parent.")]
            fn $name(self) -> Self {
                self.$base(Length::Pct($num as f32 / $den as f32))
            }
        )*)*
    };
}

/// `$name()` = `$base(Length::Container(step))`.
macro_rules! containers {
    ($($base:ident => [$($name:ident = $step:ident),* $(,)?]),* $(,)?) => {
        $($(
            #[doc = concat!("`", stringify!($base), "` of the theme's `", stringify!($step), "` container width.")]
            fn $name(self) -> Self {
                self.$base(Length::Container(ContainerStep::$step))
            }
        )*)*
    };
}

/// Style methods shared by elements and overrides.
pub trait Styled: Sized {
    /// Adds one property; it replaces an earlier one of the same kind.
    fn style(self, p: StyleProp) -> Self;

    /// Applies a function (GPUI's `FluentBuilder::map`), e.g. a reusable style:
    /// `el().map(card)` with `fn card<S: Styled>(s: S) -> S`.
    fn map(self, f: impl FnOnce(Self) -> Self) -> Self {
        f(self)
    }
    /// Applies `f` when `cond` holds (GPUI `when`).
    fn when(self, cond: bool, f: impl FnOnce(Self) -> Self) -> Self {
        if cond { f(self) } else { self }
    }
    /// Applies `f` with the value when there is one (GPUI `when_some`).
    fn when_some<T>(self, v: Option<T>, f: impl FnOnce(Self, T) -> Self) -> Self {
        match v {
            Some(v) => f(self, v),
            None => self,
        }
    }

    // ---- display and flex ----

    /// Not laid out or shown; undo with `flex()` (e.g. `.hidden().when_medium(|p| p.flex())`).
    #[doc(alias = "display_none")]
    fn hidden(self) -> Self {
        self.style(StyleProp::Display(Display::None))
    }
    /// A flex container (the default for every element).
    fn flex(self) -> Self {
        self.style(StyleProp::Display(Display::Flex))
    }
    fn flex_row(self) -> Self {
        self.style(StyleProp::Direction(FlexDirection::Row))
    }
    fn flex_row_reverse(self) -> Self {
        self.style(StyleProp::Direction(FlexDirection::RowReverse))
    }
    #[doc(alias = "column")]
    fn flex_col(self) -> Self {
        self.style(StyleProp::Direction(FlexDirection::Column))
    }
    fn flex_col_reverse(self) -> Self {
        self.style(StyleProp::Direction(FlexDirection::ColumnReverse))
    }
    fn flex_wrap(self) -> Self {
        self.style(StyleProp::FlexWrap(FlexWrap::Wrap))
    }
    fn flex_nowrap(self) -> Self {
        self.style(StyleProp::FlexWrap(FlexWrap::NoWrap))
    }
    /// Grow and shrink from a zero basis: share the free space equally with other `flex_1`
    /// siblings. The usual way to fill the rest of a column, e.g. a scroll area.
    fn flex_1(self) -> Self {
        self.flex_grow().flex_shrink().flex_basis(relative(0.0))
    }
    /// Grow and shrink from the content size.
    fn flex_auto(self) -> Self {
        self.flex_grow().flex_shrink().flex_basis(Length::Auto)
    }
    /// Keep the content size.
    fn flex_none(self) -> Self {
        self.style(StyleProp::Grow(0.0)).flex_shrink_0().flex_basis(Length::Auto)
    }
    /// Take the free space along the parent's main axis.
    fn flex_grow(self) -> Self {
        self.style(StyleProp::Grow(1.0))
    }
    /// Give way when the parent is too small, down to nothing (as Rune's `shrink`: also sets
    /// the minimum size to 0). Boxes do not shrink by default; text does.
    fn flex_shrink(self) -> Self {
        self.style(StyleProp::Shrink(1.0)).min_w_0().min_h_0()
    }
    /// Never shrink below the content size (the default for boxes; useful on text).
    fn flex_shrink_0(self) -> Self {
        self.style(StyleProp::Shrink(0.0))
    }
    fn flex_basis(self, l: impl Into<Length>) -> Self {
        self.style(StyleProp::Basis(l.into()))
    }
    fn items_start(self) -> Self {
        self.style(StyleProp::AlignItems(AlignItems::FLEX_START))
    }
    fn items_center(self) -> Self {
        self.style(StyleProp::AlignItems(AlignItems::CENTER))
    }
    fn items_end(self) -> Self {
        self.style(StyleProp::AlignItems(AlignItems::FLEX_END))
    }
    fn items_baseline(self) -> Self {
        self.style(StyleProp::AlignItems(AlignItems::BASELINE))
    }
    fn items_stretch(self) -> Self {
        self.style(StyleProp::AlignItems(AlignItems::STRETCH))
    }
    fn self_start(self) -> Self {
        self.style(StyleProp::AlignSelf(Some(AlignItems::FLEX_START)))
    }
    fn self_center(self) -> Self {
        self.style(StyleProp::AlignSelf(Some(AlignItems::CENTER)))
    }
    fn self_end(self) -> Self {
        self.style(StyleProp::AlignSelf(Some(AlignItems::FLEX_END)))
    }
    fn self_stretch(self) -> Self {
        self.style(StyleProp::AlignSelf(Some(AlignItems::STRETCH)))
    }
    fn self_auto(self) -> Self {
        self.style(StyleProp::AlignSelf(None))
    }
    fn justify_start(self) -> Self {
        self.style(StyleProp::Justify(JustifyContent::FLEX_START))
    }
    fn justify_center(self) -> Self {
        self.style(StyleProp::Justify(JustifyContent::CENTER))
    }
    fn justify_end(self) -> Self {
        self.style(StyleProp::Justify(JustifyContent::FLEX_END))
    }
    fn justify_between(self) -> Self {
        self.style(StyleProp::Justify(JustifyContent::SPACE_BETWEEN))
    }
    fn justify_around(self) -> Self {
        self.style(StyleProp::Justify(JustifyContent::SPACE_AROUND))
    }
    fn justify_evenly(self) -> Self {
        self.style(StyleProp::Justify(JustifyContent::SPACE_EVENLY))
    }

    // ---- grid (names and semantics as in GPUI) ----

    /// A grid container; place children with `grid_cols` / `grid_rows` and `col_span` ….
    fn grid(self) -> Self {
        self.style(StyleProp::Display(Display::Grid))
    }
    /// `n` equal columns: `repeat(n, minmax(0, 1fr))`.
    fn grid_cols(self, n: u16) -> Self {
        self.style(StyleProp::GridCols(n))
    }
    /// `n` equal rows: `repeat(n, minmax(0, 1fr))`.
    fn grid_rows(self, n: u16) -> Self {
        self.style(StyleProp::GridRows(n))
    }
    /// Span `n` columns.
    fn col_span(self, n: u16) -> Self {
        self.style(StyleProp::ColStart(GridPos::Span(n))).style(StyleProp::ColEnd(GridPos::Span(n)))
    }
    /// Span every column (from the first line to the last).
    fn col_span_full(self) -> Self {
        self.style(StyleProp::ColStart(GridPos::Line(1))).style(StyleProp::ColEnd(GridPos::Line(-1)))
    }
    /// Start at column line `n` (1-based; negative counts from the end).
    fn col_start(self, n: i16) -> Self {
        self.style(StyleProp::ColStart(GridPos::Line(n)))
    }
    fn col_start_auto(self) -> Self {
        self.style(StyleProp::ColStart(GridPos::Auto))
    }
    fn col_end(self, n: i16) -> Self {
        self.style(StyleProp::ColEnd(GridPos::Line(n)))
    }
    fn col_end_auto(self) -> Self {
        self.style(StyleProp::ColEnd(GridPos::Auto))
    }
    /// Span `n` rows.
    fn row_span(self, n: u16) -> Self {
        self.style(StyleProp::RowStart(GridPos::Span(n))).style(StyleProp::RowEnd(GridPos::Span(n)))
    }
    fn row_span_full(self) -> Self {
        self.style(StyleProp::RowStart(GridPos::Line(1))).style(StyleProp::RowEnd(GridPos::Line(-1)))
    }
    fn row_start(self, n: i16) -> Self {
        self.style(StyleProp::RowStart(GridPos::Line(n)))
    }
    fn row_start_auto(self) -> Self {
        self.style(StyleProp::RowStart(GridPos::Auto))
    }
    fn row_end(self, n: i16) -> Self {
        self.style(StyleProp::RowEnd(GridPos::Line(n)))
    }
    fn row_end_auto(self) -> Self {
        self.style(StyleProp::RowEnd(GridPos::Auto))
    }

    // ---- spacing: padding, margin, gap ----

    #[doc(alias = "padding")]
    fn p(self, l: impl Into<Length>) -> Self {
        let l = l.into();
        self.pt(l).pr(l).pb(l).pl(l)
    }
    /// Horizontal padding (`px-*`).
    fn px(self, l: impl Into<Length>) -> Self {
        let l = l.into();
        self.pl(l).pr(l)
    }
    /// Vertical padding (`py-*`).
    fn py(self, l: impl Into<Length>) -> Self {
        let l = l.into();
        self.pt(l).pb(l)
    }
    fn pt(self, l: impl Into<Length>) -> Self {
        self.style(StyleProp::PaddingTop(l.into()))
    }
    fn pr(self, l: impl Into<Length>) -> Self {
        self.style(StyleProp::PaddingRight(l.into()))
    }
    fn pb(self, l: impl Into<Length>) -> Self {
        self.style(StyleProp::PaddingBottom(l.into()))
    }
    fn pl(self, l: impl Into<Length>) -> Self {
        self.style(StyleProp::PaddingLeft(l.into()))
    }
    #[doc(alias = "margin")]
    fn m(self, l: impl Into<Length>) -> Self {
        let l = l.into();
        self.mt(l).mr(l).mb(l).ml(l)
    }
    fn mx(self, l: impl Into<Length>) -> Self {
        let l = l.into();
        self.ml(l).mr(l)
    }
    fn my(self, l: impl Into<Length>) -> Self {
        let l = l.into();
        self.mt(l).mb(l)
    }
    fn mt(self, l: impl Into<Length>) -> Self {
        self.style(StyleProp::MarginTop(l.into()))
    }
    fn mr(self, l: impl Into<Length>) -> Self {
        self.style(StyleProp::MarginRight(l.into()))
    }
    fn mb(self, l: impl Into<Length>) -> Self {
        self.style(StyleProp::MarginBottom(l.into()))
    }
    fn ml(self, l: impl Into<Length>) -> Self {
        self.style(StyleProp::MarginLeft(l.into()))
    }
    /// `margin: auto` on all sides.
    fn m_auto(self) -> Self {
        self.m(Length::Auto)
    }
    /// Centers horizontally in free space (`mx-auto`).
    fn mx_auto(self) -> Self {
        self.mx(Length::Auto)
    }
    fn my_auto(self) -> Self {
        self.my(Length::Auto)
    }
    fn ml_auto(self) -> Self {
        self.ml(Length::Auto)
    }
    fn mr_auto(self) -> Self {
        self.mr(Length::Auto)
    }
    fn mt_auto(self) -> Self {
        self.mt(Length::Auto)
    }
    fn mb_auto(self) -> Self {
        self.mb(Length::Auto)
    }
    fn gap(self, l: impl Into<Length>) -> Self {
        self.style(StyleProp::Gap(l.into()))
    }
    /// Gap between columns.
    fn gap_x(self, l: impl Into<Length>) -> Self {
        self.style(StyleProp::GapX(l.into()))
    }
    /// Gap between rows.
    fn gap_y(self, l: impl Into<Length>) -> Self {
        self.style(StyleProp::GapY(l.into()))
    }

    steps! {
        p => [p_0, p_0p5, p_1, p_1p5, p_2, p_2p5, p_3, p_3p5, p_4, p_5, p_6, p_7, p_8, p_9, p_10, p_11, p_12, p_14, p_16, p_20, p_24, p_28, p_32, p_36, p_40, p_44, p_48, p_52, p_56, p_60, p_64, p_72, p_80, p_96],
        px => [px_0, px_0p5, px_1, px_1p5, px_2, px_2p5, px_3, px_3p5, px_4, px_5, px_6, px_7, px_8, px_9, px_10, px_11, px_12, px_14, px_16, px_20, px_24, px_28, px_32, px_36, px_40, px_44, px_48, px_52, px_56, px_60, px_64, px_72, px_80, px_96],
        py => [py_0, py_0p5, py_1, py_1p5, py_2, py_2p5, py_3, py_3p5, py_4, py_5, py_6, py_7, py_8, py_9, py_10, py_11, py_12, py_14, py_16, py_20, py_24, py_28, py_32, py_36, py_40, py_44, py_48, py_52, py_56, py_60, py_64, py_72, py_80, py_96],
        pt => [pt_0, pt_0p5, pt_1, pt_1p5, pt_2, pt_2p5, pt_3, pt_3p5, pt_4, pt_5, pt_6, pt_7, pt_8, pt_9, pt_10, pt_11, pt_12, pt_14, pt_16, pt_20, pt_24, pt_28, pt_32, pt_36, pt_40, pt_44, pt_48, pt_52, pt_56, pt_60, pt_64, pt_72, pt_80, pt_96],
        pr => [pr_0, pr_0p5, pr_1, pr_1p5, pr_2, pr_2p5, pr_3, pr_3p5, pr_4, pr_5, pr_6, pr_7, pr_8, pr_9, pr_10, pr_11, pr_12, pr_14, pr_16, pr_20, pr_24, pr_28, pr_32, pr_36, pr_40, pr_44, pr_48, pr_52, pr_56, pr_60, pr_64, pr_72, pr_80, pr_96],
        pb => [pb_0, pb_0p5, pb_1, pb_1p5, pb_2, pb_2p5, pb_3, pb_3p5, pb_4, pb_5, pb_6, pb_7, pb_8, pb_9, pb_10, pb_11, pb_12, pb_14, pb_16, pb_20, pb_24, pb_28, pb_32, pb_36, pb_40, pb_44, pb_48, pb_52, pb_56, pb_60, pb_64, pb_72, pb_80, pb_96],
        pl => [pl_0, pl_0p5, pl_1, pl_1p5, pl_2, pl_2p5, pl_3, pl_3p5, pl_4, pl_5, pl_6, pl_7, pl_8, pl_9, pl_10, pl_11, pl_12, pl_14, pl_16, pl_20, pl_24, pl_28, pl_32, pl_36, pl_40, pl_44, pl_48, pl_52, pl_56, pl_60, pl_64, pl_72, pl_80, pl_96],
        m => [m_0, m_0p5, m_1, m_1p5, m_2, m_2p5, m_3, m_3p5, m_4, m_5, m_6, m_7, m_8, m_9, m_10, m_11, m_12, m_14, m_16, m_20, m_24, m_28, m_32, m_36, m_40, m_44, m_48, m_52, m_56, m_60, m_64, m_72, m_80, m_96],
        mx => [mx_0, mx_0p5, mx_1, mx_1p5, mx_2, mx_2p5, mx_3, mx_3p5, mx_4, mx_5, mx_6, mx_7, mx_8, mx_9, mx_10, mx_11, mx_12, mx_14, mx_16, mx_20, mx_24, mx_28, mx_32, mx_36, mx_40, mx_44, mx_48, mx_52, mx_56, mx_60, mx_64, mx_72, mx_80, mx_96],
        my => [my_0, my_0p5, my_1, my_1p5, my_2, my_2p5, my_3, my_3p5, my_4, my_5, my_6, my_7, my_8, my_9, my_10, my_11, my_12, my_14, my_16, my_20, my_24, my_28, my_32, my_36, my_40, my_44, my_48, my_52, my_56, my_60, my_64, my_72, my_80, my_96],
        mt => [mt_0, mt_0p5, mt_1, mt_1p5, mt_2, mt_2p5, mt_3, mt_3p5, mt_4, mt_5, mt_6, mt_7, mt_8, mt_9, mt_10, mt_11, mt_12, mt_14, mt_16, mt_20, mt_24, mt_28, mt_32, mt_36, mt_40, mt_44, mt_48, mt_52, mt_56, mt_60, mt_64, mt_72, mt_80, mt_96],
        mr => [mr_0, mr_0p5, mr_1, mr_1p5, mr_2, mr_2p5, mr_3, mr_3p5, mr_4, mr_5, mr_6, mr_7, mr_8, mr_9, mr_10, mr_11, mr_12, mr_14, mr_16, mr_20, mr_24, mr_28, mr_32, mr_36, mr_40, mr_44, mr_48, mr_52, mr_56, mr_60, mr_64, mr_72, mr_80, mr_96],
        mb => [mb_0, mb_0p5, mb_1, mb_1p5, mb_2, mb_2p5, mb_3, mb_3p5, mb_4, mb_5, mb_6, mb_7, mb_8, mb_9, mb_10, mb_11, mb_12, mb_14, mb_16, mb_20, mb_24, mb_28, mb_32, mb_36, mb_40, mb_44, mb_48, mb_52, mb_56, mb_60, mb_64, mb_72, mb_80, mb_96],
        ml => [ml_0, ml_0p5, ml_1, ml_1p5, ml_2, ml_2p5, ml_3, ml_3p5, ml_4, ml_5, ml_6, ml_7, ml_8, ml_9, ml_10, ml_11, ml_12, ml_14, ml_16, ml_20, ml_24, ml_28, ml_32, ml_36, ml_40, ml_44, ml_48, ml_52, ml_56, ml_60, ml_64, ml_72, ml_80, ml_96],
        gap => [gap_0, gap_0p5, gap_1, gap_1p5, gap_2, gap_2p5, gap_3, gap_3p5, gap_4, gap_5, gap_6, gap_7, gap_8, gap_9, gap_10, gap_11, gap_12, gap_14, gap_16, gap_20, gap_24],
        gap_x => [gap_x_0, gap_x_0p5, gap_x_1, gap_x_1p5, gap_x_2, gap_x_2p5, gap_x_3, gap_x_3p5, gap_x_4, gap_x_5, gap_x_6, gap_x_7, gap_x_8, gap_x_9, gap_x_10, gap_x_11, gap_x_12, gap_x_14, gap_x_16, gap_x_20, gap_x_24],
        gap_y => [gap_y_0, gap_y_0p5, gap_y_1, gap_y_1p5, gap_y_2, gap_y_2p5, gap_y_3, gap_y_3p5, gap_y_4, gap_y_5, gap_y_6, gap_y_7, gap_y_8, gap_y_9, gap_y_10, gap_y_11, gap_y_12, gap_y_14, gap_y_16, gap_y_20, gap_y_24],
    }

    // ---- size ----

    #[doc(alias = "width")]
    fn w(self, l: impl Into<Length>) -> Self {
        self.style(StyleProp::Width(l.into()))
    }
    #[doc(alias = "height")]
    fn h(self, l: impl Into<Length>) -> Self {
        self.style(StyleProp::Height(l.into()))
    }
    /// Width and height.
    fn size(self, l: impl Into<Length>) -> Self {
        let l = l.into();
        self.w(l).h(l)
    }
    fn min_w(self, l: impl Into<Length>) -> Self {
        self.style(StyleProp::MinWidth(l.into()))
    }
    fn min_h(self, l: impl Into<Length>) -> Self {
        self.style(StyleProp::MinHeight(l.into()))
    }
    fn max_w(self, l: impl Into<Length>) -> Self {
        self.style(StyleProp::MaxWidth(l.into()))
    }
    fn max_h(self, l: impl Into<Length>) -> Self {
        self.style(StyleProp::MaxHeight(l.into()))
    }
    fn w_full(self) -> Self {
        self.w(relative(1.0))
    }
    fn h_full(self) -> Self {
        self.h(relative(1.0))
    }
    fn size_full(self) -> Self {
        self.size(relative(1.0))
    }
    fn w_auto(self) -> Self {
        self.w(Length::Auto)
    }
    fn h_auto(self) -> Self {
        self.h(Length::Auto)
    }
    fn min_w_0(self) -> Self {
        self.min_w(0.0)
    }
    fn min_h_0(self) -> Self {
        self.min_h(0.0)
    }
    fn min_w_full(self) -> Self {
        self.min_w(relative(1.0))
    }
    fn max_w_full(self) -> Self {
        self.max_w(relative(1.0))
    }
    fn max_h_full(self) -> Self {
        self.max_h(relative(1.0))
    }
    /// Width / height ratio (Tailwind `aspect-[4/3]` is `aspect(4.0 / 3.0)`).
    fn aspect(self, ratio: f32) -> Self {
        self.style(StyleProp::AspectRatio(Some(ratio)))
    }
    fn aspect_square(self) -> Self {
        self.aspect(1.0)
    }
    fn aspect_video(self) -> Self {
        self.aspect(16.0 / 9.0)
    }

    steps! {
        w => [w_0, w_0p5, w_1, w_1p5, w_2, w_2p5, w_3, w_3p5, w_4, w_5, w_6, w_7, w_8, w_9, w_10, w_11, w_12, w_14, w_16, w_20, w_24, w_28, w_32, w_36, w_40, w_44, w_48, w_52, w_56, w_60, w_64, w_72, w_80, w_96],
        h => [h_0, h_0p5, h_1, h_1p5, h_2, h_2p5, h_3, h_3p5, h_4, h_5, h_6, h_7, h_8, h_9, h_10, h_11, h_12, h_14, h_16, h_20, h_24, h_28, h_32, h_36, h_40, h_44, h_48, h_52, h_56, h_60, h_64, h_72, h_80, h_96],
        size => [size_0, size_0p5, size_1, size_1p5, size_2, size_2p5, size_3, size_3p5, size_4, size_5, size_6, size_7, size_8, size_9, size_10, size_11, size_12, size_14, size_16, size_20, size_24, size_28, size_32, size_36, size_40, size_44, size_48, size_52, size_56, size_60, size_64, size_72, size_80, size_96],
        min_w => [min_w_1, min_w_2, min_w_4, min_w_8, min_w_12, min_w_16, min_w_20, min_w_24, min_w_32, min_w_40, min_w_48, min_w_64, min_w_80, min_w_96],
        min_h => [min_h_1, min_h_2, min_h_4, min_h_8, min_h_12, min_h_16, min_h_20, min_h_24, min_h_32, min_h_40, min_h_48, min_h_64, min_h_80, min_h_96],
        max_h => [max_h_1, max_h_2, max_h_4, max_h_8, max_h_12, max_h_16, max_h_20, max_h_24, max_h_32, max_h_40, max_h_48, max_h_64, max_h_72, max_h_80, max_h_96],
    }

    fractions! {
        w => [w_1_2 = 1 / 2, w_1_3 = 1 / 3, w_2_3 = 2 / 3, w_1_4 = 1 / 4, w_3_4 = 3 / 4, w_1_5 = 1 / 5, w_2_5 = 2 / 5, w_3_5 = 3 / 5, w_4_5 = 4 / 5, w_1_6 = 1 / 6, w_5_6 = 5 / 6],
        h => [h_1_2 = 1 / 2, h_1_3 = 1 / 3, h_2_3 = 2 / 3, h_1_4 = 1 / 4, h_3_4 = 3 / 4],
    }

    containers! {
        max_w => [
            max_w_3xs = Xs3, max_w_2xs = Xs2, max_w_xs = Xs, max_w_sm = Sm, max_w_md = Md, max_w_lg = Lg, max_w_xl = Xl,
            max_w_2xl = Xl2, max_w_3xl = Xl3, max_w_4xl = Xl4, max_w_5xl = Xl5, max_w_6xl = Xl6, max_w_7xl = Xl7,
        ],
        w => [w_3xs = Xs3, w_2xs = Xs2, w_xs = Xs, w_sm = Sm, w_md = Md, w_lg = Lg, w_xl = Xl, w_2xl = Xl2, w_3xl = Xl3],
    }

    // ---- position ----

    /// Taken out of the flow and placed by `top` / `right` / `bottom` / `left` within the parent.
    /// For content that must escape its parent (menus, popovers), use `El::overlay`.
    fn absolute(self) -> Self {
        self.style(StyleProp::Position(Position::Absolute))
    }
    fn relative(self) -> Self {
        self.style(StyleProp::Position(Position::Relative))
    }
    fn top(self, l: impl Into<Length>) -> Self {
        self.style(StyleProp::Top(l.into()))
    }
    fn right(self, l: impl Into<Length>) -> Self {
        self.style(StyleProp::Right(l.into()))
    }
    fn bottom(self, l: impl Into<Length>) -> Self {
        self.style(StyleProp::Bottom(l.into()))
    }
    fn left(self, l: impl Into<Length>) -> Self {
        self.style(StyleProp::Left(l.into()))
    }
    fn inset(self, l: impl Into<Length>) -> Self {
        let l = l.into();
        self.top(l).right(l).bottom(l).left(l)
    }
    fn inset_x(self, l: impl Into<Length>) -> Self {
        let l = l.into();
        self.left(l).right(l)
    }
    fn inset_y(self, l: impl Into<Length>) -> Self {
        let l = l.into();
        self.top(l).bottom(l)
    }

    steps! {
        inset => [inset_0, inset_0p5, inset_1, inset_2, inset_3, inset_4, inset_6, inset_8, inset_12, inset_16],
        top => [top_0, top_0p5, top_1, top_2, top_3, top_4, top_6, top_8, top_12, top_16],
        right => [right_0, right_0p5, right_1, right_2, right_3, right_4, right_6, right_8, right_12, right_16],
        bottom => [bottom_0, bottom_0p5, bottom_1, bottom_2, bottom_3, bottom_4, bottom_6, bottom_8, bottom_12, bottom_16],
        left => [left_0, left_0p5, left_1, left_2, left_3, left_4, left_6, left_8, left_12, left_16],
    }

    fractions! {
        top => [top_1_2 = 1 / 2, top_full = 1 / 1],
        right => [right_1_2 = 1 / 2, right_full = 1 / 1],
        bottom => [bottom_1_2 = 1 / 2, bottom_full = 1 / 1],
        left => [left_1_2 = 1 / 2, left_full = 1 / 1],
    }

    // ---- overflow (CSS / Tailwind `overflow-*`) ----

    /// Scrolls when the content is larger (`overflow-auto`); clips it otherwise. A scroll
    /// container may shrink below its content, so `flex_1().overflow_y_auto()` fills the room
    /// left and scrolls the rest.
    fn overflow_auto(self) -> Self {
        self.overflow_x_auto().overflow_y_auto()
    }
    fn overflow_x_auto(self) -> Self {
        self.style(StyleProp::OverflowX(Overflow::Scroll))
    }
    fn overflow_y_auto(self) -> Self {
        self.style(StyleProp::OverflowY(Overflow::Scroll))
    }
    /// Same as `overflow_auto` (scrollbars overlay the content).
    fn overflow_scroll(self) -> Self {
        self.overflow_auto()
    }
    fn overflow_x_scroll(self) -> Self {
        self.overflow_x_auto()
    }
    fn overflow_y_scroll(self) -> Self {
        self.overflow_y_auto()
    }
    /// Clips the content; it can still be scrolled programmatically.
    fn overflow_hidden(self) -> Self {
        self.overflow_x_hidden().overflow_y_hidden()
    }
    fn overflow_x_hidden(self) -> Self {
        self.style(StyleProp::OverflowX(Overflow::Hidden))
    }
    fn overflow_y_hidden(self) -> Self {
        self.style(StyleProp::OverflowY(Overflow::Hidden))
    }
    /// Clips the content; never scrolls.
    fn overflow_clip(self) -> Self {
        self.style(StyleProp::OverflowX(Overflow::Clip)).style(StyleProp::OverflowY(Overflow::Clip))
    }
    /// The default: content may spill out.
    fn overflow_visible(self) -> Self {
        self.style(StyleProp::OverflowX(Overflow::Visible)).style(StyleProp::OverflowY(Overflow::Visible))
    }

    // ---- border and radius ----

    /// A 1 px border (`border`).
    fn border(self) -> Self {
        self.border_w(1.0)
    }
    fn border_0(self) -> Self {
        self.border_w(0.0)
    }
    fn border_2(self) -> Self {
        self.border_w(2.0)
    }
    fn border_4(self) -> Self {
        self.border_w(4.0)
    }
    fn border_8(self) -> Self {
        self.border_w(8.0)
    }
    /// Border width (`border-[3px]` is `border_w(3.0)`).
    fn border_w(self, l: impl Into<Length>) -> Self {
        self.style(StyleProp::BorderWidth(l.into()))
    }
    /// Changes here and below move on `curve` (a [`Spring`](crate::motion::Spring) such as
    /// `Spring::SNAP`): colors, opacity, shadows, positions and sizes; also when a state
    /// (hover, pressed, focus) or the theme changes them. Inherited; nothing moves without it.
    fn motion(self, curve: impl Into<crate::motion::Curve>) -> Self {
        self.style(StyleProp::Motion(crate::kernel::Motion::all(curve.into())))
    }
    /// Changes here and below land at once (undoes an inherited `motion`).
    fn motion_none(self) -> Self {
        self.style(StyleProp::Motion(crate::kernel::Motion::NONE))
    }
    /// How colors (background, text, border) and shadows move here and below.
    fn color_motion(self, curve: impl Into<crate::motion::Curve>) -> Self {
        self.style(StyleProp::ColorMotion(curve.into()))
    }
    /// How opacity moves here and below.
    fn opacity_motion(self, curve: impl Into<crate::motion::Curve>) -> Self {
        self.style(StyleProp::OpacityMotion(curve.into()))
    }
    /// How positions and sizes move here and below.
    fn layout_motion(self, curve: impl Into<crate::motion::Curve>) -> Self {
        self.style(StyleProp::LayoutMotion(curve.into()))
    }
    /// Shadows of the element's own, first on top (GPUI `shadow(vec![..])`; up to four).
    fn shadow(self, shadows: impl IntoIterator<Item = BoxShadow>) -> Self {
        let mut list = [None; 4];
        for (slot, s) in list.iter_mut().zip(shadows) {
            *slot = Some(s);
        }
        self.style(StyleProp::Shadow(Shadows::Custom(list)))
    }
    fn shadow_none(self) -> Self {
        self.style(StyleProp::Shadow(Shadows::None))
    }
    fn shadow_2xs(self) -> Self {
        self.style(StyleProp::Shadow(Shadows::Step(ShadowStep::Xs2)))
    }
    fn shadow_xs(self) -> Self {
        self.style(StyleProp::Shadow(Shadows::Step(ShadowStep::Xs)))
    }
    fn shadow_sm(self) -> Self {
        self.style(StyleProp::Shadow(Shadows::Step(ShadowStep::Sm)))
    }
    fn shadow_md(self) -> Self {
        self.style(StyleProp::Shadow(Shadows::Step(ShadowStep::Md)))
    }
    fn shadow_lg(self) -> Self {
        self.style(StyleProp::Shadow(Shadows::Step(ShadowStep::Lg)))
    }
    fn shadow_xl(self) -> Self {
        self.style(StyleProp::Shadow(Shadows::Step(ShadowStep::Xl)))
    }
    fn shadow_2xl(self) -> Self {
        self.style(StyleProp::Shadow(Shadows::Step(ShadowStep::Xl2)))
    }
    /// Corner radius in pixels (`rounded-[10px]` is `rounded(10.0)`).
    fn rounded(self, px: f32) -> Self {
        self.style(StyleProp::Radius(Radius::Px(px)))
    }
    fn rounded_none(self) -> Self {
        self.rounded(0.0)
    }
    fn rounded_xs(self) -> Self {
        self.style(StyleProp::Radius(Radius::Step(RadiusStep::Xs)))
    }
    fn rounded_sm(self) -> Self {
        self.style(StyleProp::Radius(Radius::Step(RadiusStep::Sm)))
    }
    fn rounded_md(self) -> Self {
        self.style(StyleProp::Radius(Radius::Step(RadiusStep::Md)))
    }
    fn rounded_lg(self) -> Self {
        self.style(StyleProp::Radius(Radius::Step(RadiusStep::Lg)))
    }
    fn rounded_xl(self) -> Self {
        self.style(StyleProp::Radius(Radius::Step(RadiusStep::Xl)))
    }
    fn rounded_2xl(self) -> Self {
        self.style(StyleProp::Radius(Radius::Step(RadiusStep::Xl2)))
    }
    fn rounded_3xl(self) -> Self {
        self.style(StyleProp::Radius(Radius::Step(RadiusStep::Xl3)))
    }
    fn rounded_4xl(self) -> Self {
        self.style(StyleProp::Radius(Radius::Step(RadiusStep::Xl4)))
    }
    fn rounded_full(self) -> Self {
        self.style(StyleProp::Radius(Radius::Full))
    }

    // ---- type ----

    fn text_step(self, s: TextStep) -> Self {
        self.style(StyleProp::FontSize(FontSize::Step(s)))
    }
    fn text_xs(self) -> Self {
        self.text_step(TextStep::Xs)
    }
    fn text_sm(self) -> Self {
        self.text_step(TextStep::Sm)
    }
    /// The default size.
    fn text_base(self) -> Self {
        self.text_step(TextStep::Base)
    }
    fn text_lg(self) -> Self {
        self.text_step(TextStep::Lg)
    }
    fn text_xl(self) -> Self {
        self.text_step(TextStep::Xl)
    }
    fn text_2xl(self) -> Self {
        self.text_step(TextStep::Xl2)
    }
    fn text_3xl(self) -> Self {
        self.text_step(TextStep::Xl3)
    }
    fn text_4xl(self) -> Self {
        self.text_step(TextStep::Xl4)
    }
    fn text_5xl(self) -> Self {
        self.text_step(TextStep::Xl5)
    }
    fn text_6xl(self) -> Self {
        self.text_step(TextStep::Xl6)
    }
    fn text_7xl(self) -> Self {
        self.text_step(TextStep::Xl7)
    }
    fn text_8xl(self) -> Self {
        self.text_step(TextStep::Xl8)
    }
    fn text_9xl(self) -> Self {
        self.text_step(TextStep::Xl9)
    }
    /// Font size: `text_size(px(15.0))` or `text_size(rems(0.875))` (GPUI `text_size`).
    fn text_size(self, l: impl Into<Length>) -> Self {
        self.style(StyleProp::FontSize(FontSize::Len(l.into())))
    }
    /// Line height: `line_height(px(22.0))`, or `line_height(relative(1.5))` for a multiple of
    /// the font size (GPUI `line_height`). Without one, the theme's `line_height` (1.25) × size.
    fn line_height(self, l: impl Into<Length>) -> Self {
        let h = match l.into() {
            Length::Pct(r) => LineHeight::Relative(r),
            l => LineHeight::Len(l),
        };
        self.style(StyleProp::LineHeight(h))
    }
    /// Line height equal to the font size.
    fn leading_none(self) -> Self {
        self.line_height(relative(1.0))
    }
    fn leading_tight(self) -> Self {
        self.line_height(relative(1.25))
    }
    fn leading_snug(self) -> Self {
        self.line_height(relative(1.375))
    }
    fn leading_normal(self) -> Self {
        self.line_height(relative(1.5))
    }
    fn leading_relaxed(self) -> Self {
        self.line_height(relative(1.625))
    }
    fn leading_loose(self) -> Self {
        self.line_height(relative(2.0))
    }
    fn font_weight(self, w: u16) -> Self {
        self.style(StyleProp::FontWeight(w))
    }
    fn font_thin(self) -> Self {
        self.font_weight(100)
    }
    fn font_extralight(self) -> Self {
        self.font_weight(200)
    }
    fn font_light(self) -> Self {
        self.font_weight(300)
    }
    fn font_normal(self) -> Self {
        self.font_weight(400)
    }
    fn font_medium(self) -> Self {
        self.font_weight(500)
    }
    fn font_semibold(self) -> Self {
        self.font_weight(600)
    }
    fn font_bold(self) -> Self {
        self.font_weight(700)
    }
    fn font_extrabold(self) -> Self {
        self.font_weight(800)
    }
    fn font_black(self) -> Self {
        self.font_weight(900)
    }
    /// The theme's sans-serif family (the default).
    fn font_sans(self) -> Self {
        self.style(StyleProp::FontFamily(Family::Sans))
    }
    /// The theme's monospace family.
    fn font_mono(self) -> Self {
        self.style(StyleProp::FontFamily(Family::Mono))
    }
    /// A registered family by name (`font-['Geist']`).
    fn font_family(self, name: &'static str) -> Self {
        self.style(StyleProp::FontFamily(Family::Named(name)))
    }

    // ---- wrapping ----

    /// Keeps each paragraph on one line (GPUI `whitespace_nowrap`).
    fn whitespace_nowrap(self) -> Self {
        self.style(StyleProp::TextWrap(Wrap::None))
    }
    /// Breaks between words to fit (the default; GPUI `whitespace_normal`).
    fn whitespace_normal(self) -> Self {
        self.style(StyleProp::TextWrap(Wrap::Words))
    }
    /// One line, cut with an ellipsis where it runs out of room, and allowed to shrink below its
    /// content (GPUI / Rune `truncate`).
    fn truncate(self) -> Self {
        self.text_ellipsis().flex_shrink()
    }
    /// One line, cut with an ellipsis where it runs out of room (GPUI `text_ellipsis`).
    fn text_ellipsis(self) -> Self {
        self.style(StyleProp::TextWrap(Wrap::Truncate))
    }
    /// Shows at most `n` lines, the last ending in an ellipsis (GPUI `line_clamp`).
    fn line_clamp(self, n: u16) -> Self {
        self.style(StyleProp::MaxLines(Some(n)))
    }

    // ---- color and opacity (fixed values; elements also accept closures, see `El`) ----

    fn bg_paint(self, p: impl Into<Fill>) -> Self {
        self.style(StyleProp::Background(Some(p.into())))
    }
    fn text_paint(self, p: impl Into<Paint>) -> Self {
        self.style(StyleProp::TextColor(p.into()))
    }
    fn border_paint(self, p: impl Into<Paint>) -> Self {
        self.style(StyleProp::BorderColor(p.into()))
    }
    fn opacity_value(self, o: f32) -> Self {
        self.style(StyleProp::Opacity(o))
    }
    fn opacity_0(self) -> Self {
        self.opacity_value(0.0)
    }
    fn opacity_25(self) -> Self {
        self.opacity_value(0.25)
    }
    fn opacity_50(self) -> Self {
        self.opacity_value(0.5)
    }
    fn opacity_75(self) -> Self {
        self.opacity_value(0.75)
    }
    fn opacity_100(self) -> Self {
        self.opacity_value(1.0)
    }
}

/// Properties that apply while a condition holds: built with the same methods as elements.
#[derive(Default, Clone, Debug)]
pub struct Patch(pub(crate) Vec<StyleProp>);

impl Styled for Patch {
    fn style(mut self, p: StyleProp) -> Self {
        self.0.retain(|q| !q.same_slot(&p));
        self.0.push(p);
        self
    }
}

impl Patch {
    /// A color, a token, or a gradient (`linear_gradient(..)`).
    pub fn bg(self, p: impl Into<Fill>) -> Self {
        self.bg_paint(p)
    }
    pub fn text_color(self, p: impl Into<Paint>) -> Self {
        self.text_paint(p)
    }
    pub fn border_color(self, p: impl Into<Paint>) -> Self {
        self.border_paint(p)
    }
    pub fn opacity(self, o: f32) -> Self {
        self.opacity_value(o)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steps_and_values_come_from_names_and_numbers() {
        assert_eq!(scale_step("p_4"), 4.0);
        assert_eq!(scale_step("gap_0p5"), 0.5);
        assert_eq!(scale_step("gap_x_1p5"), 1.5);
        assert_eq!(scale_step("px_12"), 12.0);
        let p = Patch::default().p_4().gap_0p5().mx(13.0).w(320).max_w_md().w_1_2();
        assert!(p.0.contains(&StyleProp::PaddingTop(Length::Spacing(4.0))));
        assert!(p.0.contains(&StyleProp::Gap(Length::Spacing(0.5))));
        assert!(p.0.contains(&StyleProp::MarginLeft(Length::Px(13.0))));
        assert!(p.0.contains(&StyleProp::MaxWidth(Length::Container(ContainerStep::Md))));
        assert!(p.0.contains(&StyleProp::Width(Length::Pct(0.5))), "the later width wins: {p:?}");
        assert!(!p.0.contains(&StyleProp::Width(Length::Px(320.0))));
    }
}
