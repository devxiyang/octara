//! CSS declarations for the resolved layout and visual properties.

use octara::tree::{NodeFlags, NodeId, NodeKind, Tree, resolve_cursor};
use octara_model::geometry::{Align, Placed, Side, WindowViewport};
use octara_model::input::CursorStyle;
use octara_model::motion::{Curve, Motion};
use octara_model::paint::ObjectFit;
use octara_model::style::{
    AlignItems, Display, FlexDirection, FlexWrap, GridPos, JustifyContent, LayoutLength, Overflow, Position, ScrollbarVisibility,
    UserSelect,
};
use octara_model::text::{TextAlign, Wrap};
use octara_model::theme::{Color, ColorScheme, ColorToken};
use std::fmt::Write;

pub(crate) fn style(tree: &Tree, node: NodeId, viewport: WindowViewport, activatable: bool, reduced_motion: bool) -> String {
    let core = tree.node(node).expect("style for a retained node");
    let kind = core.kind;
    let l = tree.layout_style(node);
    let v = tree.visual(node);
    let theme = tree.theme_at(node);
    let scheme = if kind == NodeKind::Root || tree.has_theme_patch(node) {
        match theme.scheme {
            ColorScheme::Light => "light",
            ColorScheme::Dark => "dark",
        }
    } else {
        "inherit"
    };
    let positioned_content = matches!(kind, NodeKind::Canvas | NodeKind::Icon | NodeKind::Media)
        || tree.paint(node).is_some_and(|paint| paint.custom.is_some())
        || tree.editor(node).is_some_and(|editor| editor.is_rich());
    let display = match l.display {
        Display::None => "none",
        Display::Flex if kind == NodeKind::Link => "inline",
        Display::Flex if matches!(kind, NodeKind::Text | NodeKind::TextInput | NodeKind::Image | NodeKind::Media) => "block",
        Display::Flex => "flex",
        Display::Grid => "grid",
    };
    let direction = match l.flex_direction {
        FlexDirection::Row => "row",
        FlexDirection::Column => "column",
        FlexDirection::RowReverse => "row-reverse",
        FlexDirection::ColumnReverse => "column-reverse",
    };
    let wrap = match l.flex_wrap {
        FlexWrap::NoWrap => "nowrap",
        FlexWrap::Wrap => "wrap",
        FlexWrap::WrapReverse => "wrap-reverse",
    };
    let positioned_inline = kind == NodeKind::Link && v.z_index != 0;
    let position = match l.position {
        Position::Static if positioned_content || positioned_inline => "relative",
        Position::Static => "static",
        Position::Absolute => "absolute",
        Position::Relative => "relative",
        Position::Fixed => "fixed",
        Position::Sticky => "sticky",
    };
    let mut css = format!(
        "box-sizing:border-box;color-scheme:{scheme};display:{display};position:{position};flex-direction:{direction};flex-wrap:{wrap};flex-grow:{};flex-shrink:{};",
        l.flex_grow, l.flex_shrink,
    );
    let _ = write!(css, "isolation:isolate;z-index:{};", v.z_index);
    if let Some(link) = tree.link(node)
        && let Some(text) = tree.parent(node).and_then(|parent| tree.text(parent))
        && text.runs.iter().any(|(range, _)| range.contains(&link.range.start))
    {
        css.push_str("text-decoration:none;");
    }
    for (name, value) in [
        ("width", l.size.width),
        ("height", l.size.height),
        ("min-width", l.min_size.width),
        ("min-height", l.min_size.height),
        ("max-width", l.max_size.width),
        ("max-height", l.max_size.height),
        ("flex-basis", l.flex_basis),
    ] {
        if matches!(name, "max-width" | "max-height") && value.is_none() {
            let _ = write!(css, "{name}:none;");
        } else {
            let _ = write!(css, "{name}:{};", length_or_auto(value));
        }
    }
    for (name, value) in [
        ("padding-top", l.padding.top),
        ("padding-right", l.padding.right),
        ("padding-bottom", l.padding.bottom),
        ("padding-left", l.padding.left),
        ("border-top-width", l.border.top),
        ("border-right-width", l.border.right),
        ("border-bottom-width", l.border.bottom),
        ("border-left-width", l.border.left),
        ("column-gap", l.gap.width),
        ("row-gap", l.gap.height),
    ] {
        let _ = write!(css, "{name}:{};", length(value));
    }
    for (name, value) in [
        ("margin-top", l.margin.top),
        ("margin-right", l.margin.right),
        ("margin-bottom", l.margin.bottom),
        ("margin-left", l.margin.left),
        ("top", l.inset.top),
        ("right", l.inset.right),
        ("bottom", l.inset.bottom),
        ("left", l.inset.left),
    ] {
        if (positioned_content || positioned_inline)
            && l.position == Position::Static
            && matches!(name, "top" | "right" | "bottom" | "left")
        {
            continue;
        }
        let _ = write!(css, "{name}:{};", length_or_auto(value));
    }
    for (name, value) in [("align-items", Some(l.align_items)), ("align-self", l.align_self)] {
        if let Some(value) = value {
            let word = match value {
                AlignItems::Normal => "normal",
                AlignItems::Start => "start",
                AlignItems::End => "end",
                AlignItems::FlexStart => "flex-start",
                AlignItems::FlexEnd => "flex-end",
                AlignItems::SelfStart => "self-start",
                AlignItems::SelfEnd => "self-end",
                AlignItems::Center => "center",
                AlignItems::Baseline => "baseline",
                AlignItems::Stretch => "stretch",
            };
            let _ = write!(css, "{name}:{word};");
        }
    }
    {
        let word = match l.justify_content {
            JustifyContent::Normal => "normal",
            JustifyContent::Start => "start",
            JustifyContent::End => "end",
            JustifyContent::FlexStart => "flex-start",
            JustifyContent::FlexEnd => "flex-end",
            JustifyContent::Center => "center",
            JustifyContent::Stretch => "stretch",
            JustifyContent::SpaceBetween => "space-between",
            JustifyContent::SpaceEvenly => "space-evenly",
            JustifyContent::SpaceAround => "space-around",
        };
        let _ = write!(css, "justify-content:{word};");
    }
    for (name, value) in [("overflow-x", l.overflow_x), ("overflow-y", l.overflow_y)] {
        let word = match value {
            Overflow::Visible => "visible",
            Overflow::Clip => "clip",
            Overflow::Hidden => "hidden",
            Overflow::Auto => "auto",
            Overflow::Scroll => "scroll",
        };
        let _ = write!(css, "{name}:{word};");
    }
    if let Some(ratio) = l.aspect_ratio {
        let _ = write!(css, "aspect-ratio:{ratio};");
    }
    for (name, n) in [("grid-template-columns", l.grid_columns), ("grid-template-rows", l.grid_rows)] {
        if n != 0 {
            let _ = write!(css, "{name}:repeat({n},minmax(0,1fr));");
        }
    }
    for (name, placement) in [
        ("grid-column-start", l.grid_column.start),
        ("grid-column-end", l.grid_column.end),
        ("grid-row-start", l.grid_row.start),
        ("grid-row-end", l.grid_row.end),
    ] {
        match placement {
            GridPos::Line(line) => {
                let _ = write!(css, "{name}:{line};");
            }
            GridPos::Span(span) => {
                let _ = write!(css, "{name}:span {span};");
            }
            GridPos::Auto => {}
        }
    }
    let family = font_family(v.font_family);
    let _ = write!(
        css,
        "font-family:{family};font-size:{}px;line-height:{}px;font-weight:{};font-style:{};letter-spacing:{}px;font-variant-numeric:{};color:{};background:{};border-style:solid;border-color:{};opacity:{};",
        v.font_size,
        v.line_height,
        v.font_weight,
        if v.italic { "italic" } else { "normal" },
        v.letter_spacing,
        if v.tabular_nums { "tabular-nums" } else { "normal" },
        color(v.text),
        v.background.map_or_else(|| "transparent".into(), color),
        color(v.border),
        v.opacity,
    );
    css.push_str("border-radius:");
    for (corner, radius) in v.radius.into_iter().enumerate() {
        if corner != 0 {
            css.push(' ');
        }
        if radius == f32::INFINITY {
            css.push_str("calc(infinity * 1px)");
        } else {
            let _ = write!(css, "{radius}px");
        }
    }
    css.push(';');
    if let Some(gradient) = v.gradient {
        let _ = write!(
            css,
            "background-image:linear-gradient({}deg,{} {}%,{} {}%);",
            gradient.angle,
            color(gradient.from.0),
            gradient.from.1 * 100.0,
            color(gradient.to.0),
            gradient.to.1 * 100.0
        );
    }
    let shadows = v.shadow.resolve(theme);
    if !shadows.is_empty() {
        css.push_str("box-shadow:");
        for (index, shadow) in shadows.iter().enumerate() {
            if index != 0 {
                css.push(',');
            }
            let _ = write!(
                css,
                "{}px {}px {}px {}px {}",
                shadow.offset.0,
                shadow.offset.1,
                shadow.blur_radius,
                shadow.spread_radius,
                color(shadow.color)
            );
        }
        css.push(';');
    }
    if let Some(ring) = v.ring {
        let _ = write!(css, "outline:{}px solid {};outline-offset:{}px;", ring.width, color(ring.color), ring.offset);
    }
    if !reduced_motion {
        transitions(&mut css, v.motion);
    } else if !v.motion.is_none() {
        css.push_str("transition:none;");
    }
    if kind == NodeKind::Text {
        css.push_str(match l.text_block_align() {
            Align::Start => "",
            Align::Center => "align-content:safe center;",
            Align::End => "align-content:safe end;",
        });
        css.push_str(match v.wrap {
            Wrap::Words => "white-space:pre-wrap;overflow-wrap:break-word;",
            Wrap::None => "white-space:pre;",
            Wrap::Truncate => "white-space:nowrap;overflow:hidden;text-overflow:ellipsis;",
        });
        if l.min_size.width.is_none() && v.wrap != Wrap::None {
            css.push_str("min-inline-size:0;");
        }
        if display != "none"
            && v.wrap == Wrap::Words
            && let Some(lines) = v.max_lines.filter(|lines| *lines > 0)
        {
            let _ = write!(css, "display:-webkit-box;-webkit-box-orient:vertical;-webkit-line-clamp:{lines};overflow:hidden;");
        }
    }
    if let Some(image) = tree.image(node) {
        let _ = write!(css, "object-fit:{};", object_fit(image.fit));
    }
    if let Some(media) = tree.media_source(node) {
        let (width, height) = media.natural_size().unwrap_or((300, 150));
        if width > 0 && height > 0 {
            let _ = write!(css, "aspect-ratio:{width}/{height};");
        }
        if l.size.width.is_none() && l.size.height.is_none() && !(l.inset.left.is_some() && l.inset.right.is_some()) {
            let _ = write!(css, "width:{width}px;");
        }
    }
    if kind == NodeKind::Native && tree.native(node).is_some_and(|slot| slot.kind == "video") {
        css.push_str("object-fit:contain;");
    }
    if matches!(kind, NodeKind::Text | NodeKind::TextInput) || activatable {
        let align = match v.align {
            TextAlign::Start => "start",
            TextAlign::Center => "center",
            TextAlign::End => "end",
            TextAlign::Left => "left",
            TextAlign::Right => "right",
            TextAlign::Justify => "justify",
        };
        let _ = write!(css, "text-align:{align};");
    }
    if matches!(kind, NodeKind::Text | NodeKind::TextInput) {
        text_decoration(&mut css, v.underline, v.strike);
    }
    let cursor = match resolve_cursor(tree, Some(node)) {
        CursorStyle::Auto => "auto",
        CursorStyle::Arrow => "default",
        CursorStyle::IBeam => "text",
        CursorStyle::PointingHand => "pointer",
        CursorStyle::Crosshair => "crosshair",
        CursorStyle::OpenHand => "grab",
        CursorStyle::ClosedHand => "grabbing",
        CursorStyle::ResizeLeftRight => "ew-resize",
        CursorStyle::ResizeUpDown => "ns-resize",
        CursorStyle::ResizeColumn => "col-resize",
        CursorStyle::ResizeRow => "row-resize",
        CursorStyle::OperationNotAllowed => "not-allowed",
        CursorStyle::Wait => "wait",
        CursorStyle::Move => "move",
        CursorStyle::Help => "help",
        CursorStyle::None => "none",
        CursorStyle::ContextMenu => "context-menu",
        CursorStyle::Progress => "progress",
        CursorStyle::Cell => "cell",
        CursorStyle::VerticalText => "vertical-text",
        CursorStyle::Alias => "alias",
        CursorStyle::Copy => "copy",
        CursorStyle::NoDrop => "no-drop",
        CursorStyle::AllScroll => "all-scroll",
        CursorStyle::ResizeNorth => "n-resize",
        CursorStyle::ResizeEast => "e-resize",
        CursorStyle::ResizeSouth => "s-resize",
        CursorStyle::ResizeWest => "w-resize",
        CursorStyle::ResizeNorthEast => "ne-resize",
        CursorStyle::ResizeNorthWest => "nw-resize",
        CursorStyle::ResizeSouthEast => "se-resize",
        CursorStyle::ResizeSouthWest => "sw-resize",
        CursorStyle::ResizeNorthEastSouthWest => "nesw-resize",
        CursorStyle::ResizeNorthWestSouthEast => "nwse-resize",
        CursorStyle::ZoomIn => "zoom-in",
        CursorStyle::ZoomOut => "zoom-out",
    };
    let _ = write!(css, "cursor:{cursor};");
    if kind == NodeKind::TextInput {
        css.push_str("appearance:none;resize:none;");
        if let Some(editor) = tree.editor(node).filter(|editor| editor.is_rich()) {
            if editor.multiline {
                css.push_str("white-space:pre-wrap;overflow-wrap:break-word;");
                if l.min_size.height.is_none() {
                    let height = v.line_height * f32::from(editor.rows.0.max(1));
                    let _ = write!(css, "min-block-size:{height}px;");
                }
            } else {
                css.push_str("white-space:pre;overflow-x:auto;overflow-y:hidden;");
            }
        }
    }
    let drag_source = tree.drag_drop(node).is_some_and(|slot| slot.source.is_some());
    if drag_source {
        css.push_str("touch-action:none;");
    }
    let selection = v.user_select.or_else(|| {
        if drag_source {
            Some(UserSelect::None)
        } else if kind != NodeKind::TextInput {
            tree.user_select(node)
        } else {
            None
        }
    });
    match selection {
        Some(UserSelect::None) => css.push_str("user-select:none;"),
        Some(UserSelect::Text) => css.push_str("user-select:text;"),
        None => {}
    }
    if tree.scroll(node).is_some() {
        css.push_str(if tree.maintains_scroll_anchor(node) { "overflow-anchor:auto;" } else { "overflow-anchor:none;" });
        match v.scrollbar {
            ScrollbarVisibility::Auto => css.push_str("scrollbar-width:thin;"),
            ScrollbarVisibility::Always => {
                let (thumb, _) = ColorToken::TextSubtle.alpha(0.55).resolve(theme);
                let _ = write!(css, "scrollbar-width:thin;scrollbar-color:{} transparent;", color(thumb));
            }
            ScrollbarVisibility::Hidden => css.push_str("scrollbar-width:none;"),
        }
    } else if core.flags.contains(NodeFlags::HIDDEN) {
        css.push_str("overflow-anchor:none;");
    }
    if node == tree.root() {
        css.push_str("position:relative;width:100%;height:100%;");
    }
    if node == tree.overlay_root() {
        css.push_str("position:absolute;inset:0;z-index:2147483647;");
    }
    if node == tree.overlay_root() || core.flags.contains(NodeFlags::POINTER_EVENTS_NONE) {
        css.push_str("pointer-events:none;");
    } else if v.pointer_events == Some(true)
        || tree.parent(node).is_some_and(|parent| {
            parent == tree.overlay_root() || tree.node(parent).is_some_and(|parent| parent.flags.contains(NodeFlags::POINTER_EVENTS_NONE))
        })
    {
        css.push_str("pointer-events:auto;");
    }
    if tree.overlay(node).is_some_and(|slot| slot.target.is_some() || slot.mount.is_some()) && !tree.is_shown(node) {
        css.push_str("display:none;");
    }
    if let Some(slot) = tree.overlay(node)
        && let Some((_, placement)) = slot.target
    {
        css.push_str("position:absolute;right:auto;bottom:auto;");
        if slot.placed.is_none() {
            css.push_str("visibility:hidden;");
        }
        if placement.fit {
            let available = viewport.safe_bounds();
            let (property, extent) = match placement.side {
                Side::Top | Side::Bottom => ("max-width", available.w),
                Side::Left | Side::Right => ("max-height", available.h),
            };
            let _ = write!(css, "{property}:{}px;", (extent - 2.0 * placement.margin).max(0.0));
        }
        if let Some(Placed { x, y, side, limit, .. }) = slot.placed {
            let _ = write!(css, "left:{x}px;top:{y}px;");
            if let Some(limit) = limit {
                let axis = match side {
                    Side::Top | Side::Bottom => "height",
                    Side::Left | Side::Right => "width",
                };
                let _ = write!(css, "max-{axis}:{limit}px;overflow:auto;");
            }
        } else {
            css.push_str("left:0;top:0;visibility:hidden;");
        }
    }
    css
}

fn transitions(css: &mut String, motion: Motion) {
    let groups: [(Curve, &[&str]); 3] = [
        (
            motion.layout,
            &[
                "width",
                "height",
                "min-width",
                "min-height",
                "max-width",
                "max-height",
                "top",
                "right",
                "bottom",
                "left",
                "padding",
                "margin",
                "gap",
                "border-width",
                "border-radius",
                "flex-basis",
                "flex-grow",
                "flex-shrink",
                "outline-width",
                "outline-offset",
            ],
        ),
        (motion.color, &["color", "background-color", "border-color", "box-shadow", "outline-color"]),
        (motion.opacity, &["opacity"]),
    ];
    let mut separator = "transition:";
    for (curve, names) in groups {
        if let Curve::Fade(duration) = curve
            && duration.is_finite()
            && duration > 0.0
        {
            for name in names {
                let _ = write!(css, "{separator}{name} {duration}s cubic-bezier(0.4,0,0.2,1)");
                separator = ",";
            }
        }
    }
    if separator == "," {
        css.push(';');
    }
}

fn length(value: LayoutLength) -> String {
    match value {
        LayoutLength::Px(px) => format!("{px}px"),
        LayoutLength::Pct(fraction) => format!("{}%", fraction * 100.0),
    }
}

fn length_or_auto(value: Option<LayoutLength>) -> String {
    value.map_or_else(|| "auto".into(), length)
}

pub(crate) fn color(value: Color) -> String {
    let c = value.to_rgba8();
    format!("#{:02x}{:02x}{:02x}{:02x}", c.r, c.g, c.b, c.a)
}

pub(crate) fn text_decoration(css: &mut String, underline: bool, strike: bool) {
    css.push_str(match (underline, strike) {
        (true, true) => "text-decoration:underline line-through;",
        (true, false) => "text-decoration:underline;",
        (false, true) => "text-decoration:line-through;",
        (false, false) => "",
    });
}

pub(crate) fn font_family(family: &str) -> &str {
    if family.chars().all(|ch| ch.is_alphanumeric() || matches!(ch, ' ' | '-' | '_')) { family } else { "system-ui" }
}

pub(crate) fn object_fit(fit: ObjectFit) -> &'static str {
    match fit {
        ObjectFit::Fill => "fill",
        ObjectFit::Contain => "contain",
        ObjectFit::Cover => "cover",
        ObjectFit::ScaleDown => "scale-down",
        ObjectFit::None => "none",
    }
}
