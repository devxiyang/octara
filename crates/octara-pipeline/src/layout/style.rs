//! Converts Octara's resolved layout declarations into native engine input.

use octara_model::style::{
    AlignItems, Display, FlexDirection, FlexWrap, GridPos, GridRange, JustifyContent, LayoutLength, LayoutStyle, Overflow, Position,
};

use octara_model::geometry::{Edges, Size};

pub(super) fn compile(layout: &LayoutStyle) -> taffy::Style {
    use taffy as native;

    native::Style {
        display: match layout.display {
            Display::None => native::Display::None,
            Display::Flex => native::Display::Flex,
            Display::Grid => native::Display::Grid,
        },
        position: match layout.position {
            Position::Static => native::Position::Static,
            Position::Relative => native::Position::Relative,
            Position::Absolute => native::Position::Absolute,
            Position::Fixed => native::Position::Fixed,
            Position::Sticky => native::Position::Sticky,
        },
        size: size(layout.size, dimension),
        min_size: size(layout.min_size, length_or_auto),
        max_size: size(layout.max_size, length_or_auto),
        aspect_ratio: layout.aspect_ratio,
        padding: edges(layout.padding, length),
        margin: edges(layout.margin, length_or_auto),
        inset: edges(layout.inset, length_or_auto),
        border: edges(layout.border, length),
        gap: size(layout.gap, length),
        align_items: align_items(layout.align_items),
        align_self: layout.align_self.map(align_items),
        justify_content: match layout.justify_content {
            JustifyContent::Normal => native::JustifyContent::NORMAL,
            JustifyContent::Start => native::JustifyContent::START,
            JustifyContent::End => native::JustifyContent::END,
            JustifyContent::FlexStart => native::JustifyContent::FLEX_START,
            JustifyContent::FlexEnd => native::JustifyContent::FLEX_END,
            JustifyContent::Center => native::JustifyContent::CENTER,
            JustifyContent::Stretch => native::JustifyContent::STRETCH,
            JustifyContent::SpaceBetween => native::JustifyContent::SPACE_BETWEEN,
            JustifyContent::SpaceEvenly => native::JustifyContent::SPACE_EVENLY,
            JustifyContent::SpaceAround => native::JustifyContent::SPACE_AROUND,
        },
        flex_direction: match layout.flex_direction {
            FlexDirection::Row => native::FlexDirection::Row,
            FlexDirection::RowReverse => native::FlexDirection::RowReverse,
            FlexDirection::Column => native::FlexDirection::Column,
            FlexDirection::ColumnReverse => native::FlexDirection::ColumnReverse,
        },
        flex_wrap: match layout.flex_wrap {
            FlexWrap::NoWrap => native::FlexWrap::NoWrap,
            FlexWrap::Wrap => native::FlexWrap::Wrap,
            FlexWrap::WrapReverse => native::FlexWrap::WrapReverse,
        },
        flex_grow: layout.flex_grow,
        flex_shrink: layout.flex_shrink,
        flex_basis: dimension(layout.flex_basis),
        overflow: native::Point { x: overflow(layout.overflow_x), y: overflow(layout.overflow_y) },
        grid_template_columns: equal_tracks(layout.grid_columns),
        grid_template_rows: equal_tracks(layout.grid_rows),
        grid_column: grid_range(layout.grid_column),
        grid_row: grid_range(layout.grid_row),
        ..Default::default()
    }
}

fn size<T, U>(value: Size<T>, map: impl Fn(T) -> U) -> taffy::Size<U> {
    taffy::Size { width: map(value.width), height: map(value.height) }
}

fn edges<T, U>(value: Edges<T>, map: impl Fn(T) -> U) -> taffy::Rect<U> {
    taffy::Rect { top: map(value.top), right: map(value.right), bottom: map(value.bottom), left: map(value.left) }
}

fn length(value: LayoutLength) -> taffy::LengthPercentage {
    match value {
        LayoutLength::Px(px) => taffy::LengthPercentage::length(px),
        LayoutLength::Pct(fraction) => taffy::LengthPercentage::percent(fraction),
    }
}

fn length_or_auto(value: Option<LayoutLength>) -> taffy::LengthPercentageAuto {
    value.map_or(taffy::LengthPercentageAuto::auto(), |value| length(value).into())
}

fn dimension(value: Option<LayoutLength>) -> taffy::Dimension {
    length_or_auto(value).into()
}

fn align_items(value: AlignItems) -> taffy::AlignItems {
    use taffy::AlignItems as Native;
    match value {
        AlignItems::Normal => Native::NORMAL,
        AlignItems::Start => Native::START,
        AlignItems::End => Native::END,
        AlignItems::FlexStart => Native::FLEX_START,
        AlignItems::FlexEnd => Native::FLEX_END,
        AlignItems::SelfStart => Native::SELF_START,
        AlignItems::SelfEnd => Native::SELF_END,
        AlignItems::Center => Native::CENTER,
        AlignItems::Baseline => Native::BASELINE,
        AlignItems::Stretch => Native::STRETCH,
    }
}

fn overflow(value: Overflow) -> taffy::Overflow {
    match value {
        Overflow::Visible => taffy::Overflow::Visible,
        Overflow::Clip => taffy::Overflow::Clip,
        Overflow::Hidden => taffy::Overflow::Hidden,
        Overflow::Auto | Overflow::Scroll => taffy::Overflow::Scroll,
    }
}

fn grid_range(range: GridRange) -> taffy::Line<taffy::GridPlacement> {
    let placement = |value| match value {
        GridPos::Auto => taffy::GridPlacement::Auto,
        GridPos::Line(line) => taffy::style_helpers::line(line),
        GridPos::Span(span) => taffy::style_helpers::span(span),
    };
    taffy::Line { start: placement(range.start), end: placement(range.end) }
}

fn equal_tracks(count: u16) -> Vec<taffy::GridTemplateComponent<String>> {
    use taffy::style_helpers::{fr, length, minmax, repeat};
    if count == 0 { Vec::new() } else { vec![repeat(count, vec![minmax(length(0.0), fr(1.0))])] }
}
