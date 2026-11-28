//! Value-polymorphic style methods shared by elements and conditional style patches.

use crate::TextOverflow;
use octara_model::motion::Curve;
use octara_model::style::{
    AlignItems, Contain, Direction, Display, FlexDirection, FlexWrap, JustifyContent, Overflow, Position, StyleProp, Transform3D,
};
pub use octara_model::style::{FontSize, Length, LineHeight, Radius, Ring, Shadows, State, When};
use octara_model::text::{FontWeight, TextAlign, Wrap};
use octara_model::theme::{ColorValue, Family, Fill};
use smallvec::SmallVec;

/// A length in logical pixels.
pub const fn px(n: f32) -> Length {
    Length::Px(n)
}

/// A fraction of the parent size.
pub const fn rel(n: f32) -> Length {
    Length::Pct(n)
}

/// A length in theme spacing steps.
pub const fn sp(n: f32) -> Length {
    Length::Spacing(n)
}

/// A reusable sequence of style properties.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StylePatch(pub(crate) SmallVec<[StyleProp; 4]>);

impl Styled for StylePatch {
    fn push_styles(mut self, properties: &[StyleProp]) -> Self {
        self.0.extend_from_slice(properties);
        self
    }
}

/// Static style declarations produced by [`tw!`](crate::tw).
///
/// Unknown or invalid classes fail at compile time.
///
/// ```compile_fail
/// use octara::prelude::*;
/// let _ = el().tw(tw!("bg-surface/120"));
/// ```
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TwStyle {
    pub(crate) base: &'static [StyleProp],
    pub(crate) overrides: &'static [(When, &'static [StyleProp])],
}

impl TwStyle {
    #[doc(hidden)]
    pub const fn new(base: &'static [StyleProp], overrides: &'static [(When, &'static [StyleProp])]) -> Self {
        Self { base, overrides }
    }
}

macro_rules! properties {
    ($($method:ident($value:ty) => $property:ident),* $(,)?) => {$ (
        fn $method(self, value: $value) -> Self {
            self.style(StyleProp::$property(value.into()))
        }
    )*};
}

/// Orthogonal style declarations, accepting logical pixels, theme units, and typed values.
pub trait Styled: Sized {
    #[doc(hidden)]
    fn push_styles(self, properties: &[StyleProp]) -> Self;

    fn style(self, property: StyleProp) -> Self {
        self.push_styles(&[property])
    }

    properties! {
        layout_motion(impl Into<Curve>) => LayoutMotion,
        display(Display) => Display,
        direction(FlexDirection) => Direction,
        text_direction(Direction) => TextDirection,
        wrap(FlexWrap) => FlexWrap,
        align(AlignItems) => AlignItems,
        justify(JustifyContent) => Justify,
        position(Position) => Position,
        contain(Contain) => Contain,
        overflow_x(Overflow) => OverflowX,
        overflow_y(Overflow) => OverflowY,
        grid_cols(u16) => GridCols,
        grid_rows(u16) => GridRows,
        grow(f32) => Grow,
        shrink(f32) => Shrink,
        basis(impl Into<Length>) => Basis,
        w(impl Into<Length>) => Width,
        h(impl Into<Length>) => Height,
        min_w(impl Into<Length>) => MinWidth,
        min_h(impl Into<Length>) => MinHeight,
        max_w(impl Into<Length>) => MaxWidth,
        max_h(impl Into<Length>) => MaxHeight,
        pt(impl Into<Length>) => PaddingTop,
        pr(impl Into<Length>) => PaddingRight,
        pb(impl Into<Length>) => PaddingBottom,
        pl(impl Into<Length>) => PaddingLeft,
        ps(impl Into<Length>) => PaddingStart,
        pe(impl Into<Length>) => PaddingEnd,
        mt(impl Into<Length>) => MarginTop,
        mr(impl Into<Length>) => MarginRight,
        mb(impl Into<Length>) => MarginBottom,
        ml(impl Into<Length>) => MarginLeft,
        ms(impl Into<Length>) => MarginStart,
        me(impl Into<Length>) => MarginEnd,
        top(impl Into<Length>) => Top,
        right(impl Into<Length>) => Right,
        bottom(impl Into<Length>) => Bottom,
        left(impl Into<Length>) => Left,
        start(impl Into<Length>) => InsetStart,
        end(impl Into<Length>) => InsetEnd,
        gap(impl Into<Length>) => Gap,
        gap_x(impl Into<Length>) => GapX,
        gap_y(impl Into<Length>) => GapY,
        border_t(impl Into<Length>) => BorderTop,
        border_r(impl Into<Length>) => BorderRight,
        border_b(impl Into<Length>) => BorderBottom,
        border_l(impl Into<Length>) => BorderLeft,
        border_s(impl Into<Length>) => BorderStart,
        border_e(impl Into<Length>) => BorderEnd,
        border_color(impl Into<ColorValue>) => BorderColor,
        ring(impl Into<Length>) => RingWidth,
        ring_color(impl Into<ColorValue>) => RingColor,
        ring_offset(impl Into<Length>) => RingOffset,
        rounded_tl(impl Into<Radius>) => RadiusTopLeft,
        rounded_tr(impl Into<Radius>) => RadiusTopRight,
        rounded_br(impl Into<Radius>) => RadiusBottomRight,
        rounded_bl(impl Into<Radius>) => RadiusBottomLeft,
        z(i32) => ZIndex,
        shadow(impl Into<Shadows>) => Shadow,
        text_color(impl Into<ColorValue>) => TextColor,
        text_size(impl Into<FontSize>) => FontSize,
        text_weight(FontWeight) => FontWeight,
        text_overflow(TextOverflow) => TextWrap,
        text_family(impl Into<Family>) => FontFamily,
        italic(bool) => Italic,
        underline(bool) => Underline,
        strike(bool) => Strike,
        letter_spacing(f32) => LetterSpacing,
        tabular_nums(bool) => TabularNums,
        text_align(TextAlign) => TextAlign,
        text_wrap(Wrap) => TextWrap,
        line_height(impl Into<LineHeight>) => LineHeight,
        opacity(f32) => Opacity,
    }

    /// Applies a paint transform to the element and its descendants. Pass `None` to clear it.
    fn transform_3d(self, value: impl Into<Option<Transform3D>>) -> Self {
        self.style(StyleProp::Transform3D(value.into()))
    }

    fn size(self, length: impl Into<Length>) -> Self {
        let length = length.into();
        self.push_styles(&[StyleProp::Width(length), StyleProp::Height(length)])
    }

    fn p(self, length: impl Into<Length>) -> Self {
        let length = length.into();
        self.push_styles(&[
            StyleProp::PaddingLeft(length),
            StyleProp::PaddingRight(length),
            StyleProp::PaddingTop(length),
            StyleProp::PaddingBottom(length),
        ])
    }

    fn px(self, length: impl Into<Length>) -> Self {
        let length = length.into();
        self.push_styles(&[StyleProp::PaddingLeft(length), StyleProp::PaddingRight(length)])
    }

    fn py(self, length: impl Into<Length>) -> Self {
        let length = length.into();
        self.push_styles(&[StyleProp::PaddingTop(length), StyleProp::PaddingBottom(length)])
    }

    fn m(self, length: impl Into<Length>) -> Self {
        let length = length.into();
        self.push_styles(&[
            StyleProp::MarginLeft(length),
            StyleProp::MarginRight(length),
            StyleProp::MarginTop(length),
            StyleProp::MarginBottom(length),
        ])
    }

    fn mx(self, length: impl Into<Length>) -> Self {
        let length = length.into();
        self.push_styles(&[StyleProp::MarginLeft(length), StyleProp::MarginRight(length)])
    }

    fn my(self, length: impl Into<Length>) -> Self {
        let length = length.into();
        self.push_styles(&[StyleProp::MarginTop(length), StyleProp::MarginBottom(length)])
    }

    fn inset(self, length: impl Into<Length>) -> Self {
        let length = length.into();
        self.push_styles(&[StyleProp::Top(length), StyleProp::Right(length), StyleProp::Bottom(length), StyleProp::Left(length)])
    }

    fn border(self, width: impl Into<Length>) -> Self {
        let width = width.into();
        self.push_styles(&[
            StyleProp::BorderTop(width),
            StyleProp::BorderRight(width),
            StyleProp::BorderBottom(width),
            StyleProp::BorderLeft(width),
        ])
    }

    fn rounded(self, radius: impl Into<Radius>) -> Self {
        let radius = radius.into();
        self.rounded_t(radius).rounded_b(radius)
    }

    fn rounded_t(self, radius: impl Into<Radius>) -> Self {
        let radius = radius.into();
        self.rounded_tl(radius).rounded_tr(radius)
    }

    fn rounded_b(self, radius: impl Into<Radius>) -> Self {
        let radius = radius.into();
        self.rounded_bl(radius).rounded_br(radius)
    }

    fn rounded_l(self, radius: impl Into<Radius>) -> Self {
        let radius = radius.into();
        self.rounded_tl(radius).rounded_bl(radius)
    }

    fn rounded_r(self, radius: impl Into<Radius>) -> Self {
        let radius = radius.into();
        self.rounded_tr(radius).rounded_br(radius)
    }

    fn border_x(self, width: impl Into<Length>) -> Self {
        let width = width.into();
        self.push_styles(&[StyleProp::BorderLeft(width), StyleProp::BorderRight(width)])
    }

    fn border_y(self, width: impl Into<Length>) -> Self {
        let width = width.into();
        self.push_styles(&[StyleProp::BorderTop(width), StyleProp::BorderBottom(width)])
    }

    fn align_self(self, align: AlignItems) -> Self {
        self.style(StyleProp::AlignSelf(Some(align)))
    }

    fn aspect_ratio(self, ratio: f32) -> Self {
        self.style(StyleProp::AspectRatio(Some(ratio)))
    }

    fn bg(self, fill: impl Into<Fill>) -> Self {
        self.style(StyleProp::Background(Some(fill.into())))
    }

    fn max_lines(self, count: u16) -> Self {
        self.style(StyleProp::MaxLines(Some(count)))
    }

    fn items_center(self) -> Self {
        self.align(AlignItems::Center)
    }

    fn items_start(self) -> Self {
        self.align(AlignItems::FlexStart)
    }

    fn items_end(self) -> Self {
        self.align(AlignItems::FlexEnd)
    }

    fn items_stretch(self) -> Self {
        self.align(AlignItems::Stretch)
    }

    fn justify_center(self) -> Self {
        self.justify(JustifyContent::Center)
    }

    fn justify_between(self) -> Self {
        self.justify(JustifyContent::SpaceBetween)
    }

    fn justify_start(self) -> Self {
        self.justify(JustifyContent::FlexStart)
    }

    fn justify_end(self) -> Self {
        self.justify(JustifyContent::FlexEnd)
    }

    /// Fills available flex space and allows the item to shrink below its content size.
    fn flex_1(self) -> Self {
        self.push_styles(&[
            StyleProp::Grow(1.0),
            StyleProp::Shrink(1.0),
            StyleProp::Basis(Length::Px(0.0)),
            StyleProp::MinWidth(Length::Px(0.0)),
            StyleProp::MinHeight(Length::Px(0.0)),
        ])
    }

    fn w_full(self) -> Self {
        self.w(rel(1.0))
    }

    fn h_full(self) -> Self {
        self.h(rel(1.0))
    }

    /// Isolate layout and clip painted descendants to this box.
    fn contain_content(self) -> Self {
        self.contain(Contain::CONTENT)
    }

    /// Isolate layout, intrinsic size, and paint.
    fn contain_strict(self) -> Self {
        self.contain(Contain::STRICT)
    }

    fn scroll_y(self) -> Self {
        self.push_styles(&[StyleProp::OverflowY(Overflow::Scroll), StyleProp::MinHeight(Length::Px(0.0))])
    }

    fn scroll_x(self) -> Self {
        self.push_styles(&[StyleProp::OverflowX(Overflow::Scroll), StyleProp::MinWidth(Length::Px(0.0))])
    }

    fn truncate(self) -> Self {
        self.text_wrap(Wrap::Truncate)
    }
}
