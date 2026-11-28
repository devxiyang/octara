//! Converts Octara's resolved layout declarations into native engine input.

use octara_model::style::{
    AlignItems, Display, FlexDirection, FlexWrap, GridPos, GridRange, JustifyContent, LayoutLength, LayoutStyle, Overflow, Position,
};

use octara_model::geometry::{Edges, Size};

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

#[derive(Clone, Copy)]
pub(super) struct StyleOverride {
    pub(super) size: Size<Option<LayoutLength>>,
    pub(super) min_size: Size<Option<LayoutLength>>,
    pub(super) max_size: Size<Option<LayoutLength>>,
    pub(super) inset: Edges<Option<LayoutLength>>,
    pub(super) position: Option<Position>,
    pub(super) flex_direction: FlexDirection,
    pub(super) overflow: [Overflow; 2],
}

impl StyleOverride {
    pub(super) fn for_style(layout: &LayoutStyle) -> Self {
        Self {
            size: layout.size,
            min_size: layout.min_size,
            max_size: layout.max_size,
            inset: layout.inset,
            position: None,
            flex_direction: layout.flex_direction,
            overflow: [layout.overflow_x, layout.overflow_y],
        }
    }
}

#[derive(Clone, Copy)]
pub(super) struct OctaraStyleRef<'a> {
    layout: &'a LayoutStyle,
    values: StyleOverride,
}

impl<'a> OctaraStyleRef<'a> {
    pub(super) fn new(layout: &'a LayoutStyle, values: StyleOverride) -> Self {
        Self { layout, values }
    }

    fn gap(&self) -> taffy::Size<taffy::LengthPercentage> {
        size(self.layout.gap, length)
    }

    fn alignment(&self) -> Option<taffy::AlignItems> {
        self.layout.align_self.map(align_items)
    }

    fn justification(&self) -> taffy::JustifyContent {
        use taffy::JustifyContent as Native;
        match self.layout.justify_content {
            JustifyContent::Normal => Native::NORMAL,
            JustifyContent::Start => Native::START,
            JustifyContent::End => Native::END,
            JustifyContent::FlexStart => Native::FLEX_START,
            JustifyContent::FlexEnd => Native::FLEX_END,
            JustifyContent::Center => Native::CENTER,
            JustifyContent::Stretch => Native::STRETCH,
            JustifyContent::SpaceBetween => Native::SPACE_BETWEEN,
            JustifyContent::SpaceEvenly => Native::SPACE_EVENLY,
            JustifyContent::SpaceAround => Native::SPACE_AROUND,
        }
    }
}

impl taffy::CoreStyle for OctaraStyleRef<'_> {
    type CustomIdent = String;

    fn box_generation_mode(&self) -> taffy::BoxGenerationMode {
        if self.layout.display == Display::None { taffy::BoxGenerationMode::None } else { taffy::BoxGenerationMode::Normal }
    }

    fn position(&self) -> taffy::Position {
        use taffy::Position as Native;
        match self.values.position.unwrap_or(self.layout.position) {
            Position::Static => Native::Static,
            Position::Relative => Native::Relative,
            Position::Absolute => Native::Absolute,
            Position::Fixed => Native::Fixed,
            Position::Sticky => Native::Sticky,
        }
    }

    fn is_containing_block(&self) -> taffy::ContainingBlockClaims {
        use octara_model::style::Contain;
        if self.layout.contain.intersects(Contain::LAYOUT | Contain::PAINT) {
            taffy::ContainingBlockClaims::ALL
        } else {
            taffy::ContainingBlockClaims { absolute: self.position().is_positioned(), fixed: false }
        }
    }

    fn overflow(&self) -> taffy::Point<taffy::Overflow> {
        let [x, y] = self.values.overflow.map(overflow);
        taffy::Point { x, y }
    }

    fn size(&self) -> taffy::Size<taffy::Dimension> {
        size(self.values.size, dimension)
    }

    fn min_size(&self) -> taffy::Size<taffy::LengthPercentageAuto> {
        size(self.values.min_size, length_or_auto)
    }

    fn max_size(&self) -> taffy::Size<taffy::LengthPercentageAuto> {
        size(self.values.max_size, length_or_auto)
    }

    fn inset(&self) -> taffy::Rect<taffy::LengthPercentageAuto> {
        edges(self.values.inset, length_or_auto)
    }

    fn aspect_ratio(&self) -> Option<f32> {
        self.layout.aspect_ratio
    }

    fn margin(&self) -> taffy::Rect<taffy::LengthPercentageAuto> {
        edges(self.layout.margin, length_or_auto)
    }

    fn padding(&self) -> taffy::Rect<taffy::LengthPercentage> {
        edges(self.layout.padding, length)
    }

    fn border(&self) -> taffy::Rect<taffy::LengthPercentage> {
        edges(self.layout.border, length)
    }

    fn contain(&self) -> taffy::Contain {
        use octara_model::style::Contain;
        let mut flags = taffy::Contain::NONE;
        if self.layout.contain.contains(Contain::LAYOUT) {
            flags = flags.union(taffy::Contain::LAYOUT);
        }
        if self.layout.contain.contains(Contain::PAINT) {
            flags = flags.union(taffy::Contain::PAINT);
        }
        flags
    }
}

impl taffy::FlexboxContainerStyle for OctaraStyleRef<'_> {
    fn flex_direction(&self) -> taffy::FlexDirection {
        use taffy::FlexDirection as Native;
        match self.values.flex_direction {
            FlexDirection::Row => Native::Row,
            FlexDirection::RowReverse => Native::RowReverse,
            FlexDirection::Column => Native::Column,
            FlexDirection::ColumnReverse => Native::ColumnReverse,
        }
    }

    fn flex_wrap(&self) -> taffy::FlexWrap {
        use taffy::FlexWrap as Native;
        match self.layout.flex_wrap {
            FlexWrap::NoWrap => Native::NoWrap,
            FlexWrap::Wrap => Native::Wrap,
            FlexWrap::WrapReverse => Native::WrapReverse,
        }
    }

    fn gap(&self) -> taffy::Size<taffy::LengthPercentage> {
        self.gap()
    }

    fn align_items(&self) -> taffy::AlignItems {
        align_items(self.layout.align_items)
    }

    fn justify_content(&self) -> taffy::JustifyContent {
        self.justification()
    }
}

impl taffy::FlexboxItemStyle for OctaraStyleRef<'_> {
    fn flex_basis(&self) -> taffy::Dimension {
        dimension(self.layout.flex_basis)
    }

    fn flex_grow(&self) -> f32 {
        self.layout.flex_grow
    }

    fn flex_shrink(&self) -> f32 {
        self.layout.flex_shrink
    }

    fn align_self(&self) -> Option<taffy::AlignSelf> {
        self.alignment()
    }
}

#[derive(Clone, Copy)]
pub(super) struct EqualTracks(u16);

type NoLineNames<'a> = std::iter::Map<std::slice::Iter<'a, Vec<String>>, fn(&Vec<String>) -> std::slice::Iter<'_, String>>;
pub(super) type GridTrackIter = std::iter::Once<taffy::GenericGridTemplateComponent<String, EqualTracks>>;

impl taffy::GenericRepetition for EqualTracks {
    type CustomIdent = String;
    type RepetitionTrackList<'a> = std::iter::Once<taffy::TrackSizingFunction>;
    type TemplateLineNames<'a> = NoLineNames<'a>;

    fn count(&self) -> taffy::RepetitionCount {
        taffy::RepetitionCount::Count(self.0)
    }

    fn tracks(&self) -> Self::RepetitionTrackList<'_> {
        use taffy::style_helpers::{fr, length, minmax};
        std::iter::once(minmax(length(0.0), fr(1.0)))
    }

    fn lines_names(&self) -> Self::TemplateLineNames<'_> {
        [].iter().map(|names: &Vec<String>| names.iter())
    }
}

impl taffy::GridContainerStyle for OctaraStyleRef<'_> {
    type Repetition<'a>
        = EqualTracks
    where
        Self: 'a;
    type TemplateTrackList<'a>
        = GridTrackIter
    where
        Self: 'a;
    type AutoTrackList<'a>
        = std::iter::Empty<taffy::TrackSizingFunction>
    where
        Self: 'a;
    type TemplateLineNames<'a>
        = NoLineNames<'a>
    where
        Self: 'a;
    type GridTemplateAreas<'a>
        = std::iter::Empty<taffy::GridTemplateArea<String>>
    where
        Self: 'a;

    fn grid_template_rows(&self) -> Option<GridTrackIter> {
        (self.layout.grid_rows != 0)
            .then(|| std::iter::once(taffy::GenericGridTemplateComponent::Repeat(EqualTracks(self.layout.grid_rows))))
    }

    fn grid_template_columns(&self) -> Option<GridTrackIter> {
        (self.layout.grid_columns != 0)
            .then(|| std::iter::once(taffy::GenericGridTemplateComponent::Repeat(EqualTracks(self.layout.grid_columns))))
    }

    fn grid_auto_rows(&self) -> Self::AutoTrackList<'_> {
        std::iter::empty()
    }

    fn grid_auto_columns(&self) -> Self::AutoTrackList<'_> {
        std::iter::empty()
    }

    fn grid_template_areas(&self) -> Option<Self::GridTemplateAreas<'_>> {
        None
    }

    fn grid_template_column_names(&self) -> Option<Self::TemplateLineNames<'_>> {
        None
    }

    fn grid_template_row_names(&self) -> Option<Self::TemplateLineNames<'_>> {
        None
    }

    fn gap(&self) -> taffy::Size<taffy::LengthPercentage> {
        self.gap()
    }

    fn align_items(&self) -> taffy::AlignItems {
        align_items(self.layout.align_items)
    }

    fn justify_content(&self) -> taffy::JustifyContent {
        self.justification()
    }
}

impl taffy::GridItemStyle for OctaraStyleRef<'_> {
    fn grid_row(&self) -> taffy::Line<taffy::GridPlacement<String>> {
        grid_range(self.layout.grid_row)
    }

    fn grid_column(&self) -> taffy::Line<taffy::GridPlacement<String>> {
        grid_range(self.layout.grid_column)
    }

    fn align_self(&self) -> Option<taffy::AlignSelf> {
        self.alignment()
    }
}

impl taffy::OofItemStyle for OctaraStyleRef<'_> {
    fn grid_row(&self) -> taffy::Line<taffy::GridPlacement<String>> {
        grid_range(self.layout.grid_row)
    }

    fn grid_column(&self) -> taffy::Line<taffy::GridPlacement<String>> {
        grid_range(self.layout.grid_column)
    }

    fn align_self(&self) -> Option<taffy::AlignSelf> {
        self.alignment()
    }
}
