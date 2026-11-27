use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use syn::LitStr;

macro_rules! prop {
    ($($tokens:tt)*) => {
        quote!(::octara::model::style::StyleProp::$($tokens)*)
    };
}

macro_rules! layout {
    ($property:ident, $kind:ident::$value:ident) => {
        prop!($property(::octara::model::style::$kind::$value))
    };
}

type ClassResult<T> = Result<T, &'static str>;
const BORDER_EDGES: &[&str] = &["BorderTop", "BorderRight", "BorderBottom", "BorderLeft"];

pub(super) fn expand(input: &LitStr) -> syn::Result<TokenStream> {
    let source = input.value();
    let mut base = Vec::new();
    let mut overrides: Vec<(&str, TokenStream, Vec<TokenStream>)> = Vec::new();
    for class in source.split_whitespace() {
        let invalid = |message| syn::Error::new(input.span(), format!("invalid `tw!` class `{class}`: {message}"));
        let Some((prefix, class)) = class.split_once(':') else {
            base.extend(utility(class).map_err(invalid)?);
            continue;
        };
        if class.contains(':') {
            return Err(invalid("only one condition per utility is supported"));
        }
        let condition = condition(prefix).map_err(invalid)?;
        let props = utility(class).map_err(invalid)?;
        // Join adjacent overrides without reordering different named groups.
        match overrides.last_mut() {
            Some((previous, _, styles)) if *previous == prefix => styles.extend(props),
            _ => overrides.push((prefix, condition, props)),
        }
    }
    let overrides = overrides.into_iter().map(|(_, condition, props)| quote!((#condition, &[#(#props),*])));
    Ok(quote!(const {
        const BASE: &[::octara::model::style::StyleProp] = &[#(#base),*];
        const OVERRIDES: &[(::octara::model::style::When, &[::octara::model::style::StyleProp])] = &[#(#overrides),*];
        ::octara::TwStyle::new(BASE, OVERRIDES)
    }))
}

fn condition(name: &str) -> ClassResult<TokenStream> {
    let when = quote!(::octara::model::style::When);
    let state = match name {
        "dark" => return Ok(quote!(#when::Dark)),
        "light" => return Ok(quote!(#when::Light)),
        "touch" => return Ok(quote!(#when::Touch)),
        "group-hover" => return Ok(quote!(#when::GroupHover("group"))),
        "hover" => "HOVER",
        "focus" => "FOCUSED",
        "focus-visible" => "FOCUS_VISIBLE",
        "focus-within" => "FOCUS_WITHIN",
        "active" | "pressed" => "PRESSED",
        "selected" | "aria-selected" => "SELECTED",
        "checked" | "aria-checked" => "CHECKED",
        "expanded" | "aria-expanded" => "EXPANDED",
        "disabled" => "DISABLED",
        "drag-over" => "DRAG_OVER",
        "dragging" => "DRAGGING",
        "drop-hover" => "DROP_HOVER",
        _ => {
            if let Some(group) = name.strip_prefix("group-hover/") {
                if group.is_empty() || !group.bytes().all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, b'-' | b'_')) {
                    return Err("expected a group name after `group-hover/`");
                }
                return Ok(quote!(#when::GroupHover(#group)));
            }
            let breakpoint = match name {
                "sm" => "Sm",
                "md" => "Md",
                "lg" => "Lg",
                "xl" => "Xl",
                "2xl" => "Xl2",
                _ => return Err("unknown condition"),
            };
            let breakpoint = format_ident!("{breakpoint}");
            return Ok(quote!(#when::Responsive(::octara::model::theme::ResponsiveBreakpoint::#breakpoint)));
        }
    };
    let state = format_ident!("{state}");
    Ok(quote!(#when::State(::octara::model::style::State::#state)))
}

fn utility(class: &str) -> ClassResult<Vec<TokenStream>> {
    let property = match class {
        "flex" => layout!(Display, Display::Flex),
        "grid" => layout!(Display, Display::Grid),
        "hidden" => layout!(Display, Display::None),
        "flex-row" => layout!(Direction, FlexDirection::Row),
        "flex-row-reverse" => layout!(Direction, FlexDirection::RowReverse),
        "flex-col" => layout!(Direction, FlexDirection::Column),
        "flex-col-reverse" => layout!(Direction, FlexDirection::ColumnReverse),
        "flex-wrap" => layout!(FlexWrap, FlexWrap::Wrap),
        "flex-wrap-reverse" => layout!(FlexWrap, FlexWrap::WrapReverse),
        "flex-nowrap" => layout!(FlexWrap, FlexWrap::NoWrap),
        "items-start" => layout!(AlignItems, AlignItems::FlexStart),
        "items-end" => layout!(AlignItems, AlignItems::FlexEnd),
        "items-center" => layout!(AlignItems, AlignItems::Center),
        "items-baseline" => layout!(AlignItems, AlignItems::Baseline),
        "items-stretch" => layout!(AlignItems, AlignItems::Stretch),
        "items-normal" => layout!(AlignItems, AlignItems::Normal),
        "self-auto" => prop!(AlignSelf(None)),
        "self-start" => prop!(AlignSelf(Some(::octara::model::style::AlignItems::FlexStart))),
        "self-end" => prop!(AlignSelf(Some(::octara::model::style::AlignItems::FlexEnd))),
        "self-center" => prop!(AlignSelf(Some(::octara::model::style::AlignItems::Center))),
        "self-baseline" => prop!(AlignSelf(Some(::octara::model::style::AlignItems::Baseline))),
        "self-stretch" => prop!(AlignSelf(Some(::octara::model::style::AlignItems::Stretch))),
        "justify-start" => layout!(Justify, JustifyContent::FlexStart),
        "justify-end" => layout!(Justify, JustifyContent::FlexEnd),
        "justify-center" => layout!(Justify, JustifyContent::Center),
        "justify-between" => layout!(Justify, JustifyContent::SpaceBetween),
        "justify-around" => layout!(Justify, JustifyContent::SpaceAround),
        "justify-evenly" => layout!(Justify, JustifyContent::SpaceEvenly),
        "justify-normal" => layout!(Justify, JustifyContent::Normal),
        "justify-stretch" => layout!(Justify, JustifyContent::Stretch),
        "flex-1" => {
            return Ok(vec![
                prop!(Grow(1.0)),
                prop!(Shrink(1.0)),
                prop!(Basis(::octara::model::style::Length::Px(0.0))),
                prop!(MinWidth(::octara::model::style::Length::Px(0.0))),
                prop!(MinHeight(::octara::model::style::Length::Px(0.0))),
            ]);
        }
        "flex-auto" => return Ok(flex_with_auto_basis(1.0, 1.0)),
        "flex-initial" => return Ok(flex_with_auto_basis(0.0, 1.0)),
        "flex-none" => return Ok(flex_with_auto_basis(0.0, 0.0)),
        "grow" => prop!(Grow(1.0)),
        "grow-0" => prop!(Grow(0.0)),
        "shrink" => prop!(Shrink(1.0)),
        "shrink-0" => prop!(Shrink(0.0)),
        "static" => layout!(Position, Position::Static),
        "relative" => layout!(Position, Position::Relative),
        "absolute" => layout!(Position, Position::Absolute),
        "fixed" => layout!(Position, Position::Fixed),
        "sticky" => layout!(Position, Position::Sticky),
        "aspect-square" => prop!(AspectRatio(Some(1.0))),
        "aspect-video" => prop!(AspectRatio(Some(16.0 / 9.0))),
        "truncate" => prop!(TextWrap(::octara::model::text::Wrap::Truncate)),
        "whitespace-nowrap" => prop!(TextWrap(::octara::model::text::Wrap::None)),
        "whitespace-normal" => prop!(TextWrap(::octara::model::text::Wrap::Words)),
        "font-sans" => prop!(FontFamily(::octara::model::theme::Family::Sans)),
        "font-mono" => prop!(FontFamily(::octara::model::theme::Family::Mono)),
        "italic" => prop!(Italic(true)),
        "not-italic" => prop!(Italic(false)),
        "underline" => prop!(Underline(true)),
        "line-through" => prop!(Strike(true)),
        "no-underline" => return Ok(vec![prop!(Underline(false)), prop!(Strike(false))]),
        "tabular-nums" => prop!(TabularNums(true)),
        "normal-nums" => prop!(TabularNums(false)),
        "border" => return Ok(border_width(BORDER_EDGES, 1.0)),
        "ring" => prop!(RingWidth(::octara::model::style::Length::Px(1.0))),
        "transition-colors" => prop!(Motion(::octara::model::motion::Motion {
            color: ::octara::model::motion::Curve::Fade(0.15),
            ..::octara::model::motion::Motion::NONE
        })),
        "transition-opacity" => prop!(Motion(::octara::model::motion::Motion {
            opacity: ::octara::model::motion::Curve::Fade(0.15),
            ..::octara::model::motion::Motion::NONE
        })),
        "transition-all" => prop!(Motion(::octara::model::motion::Motion::all(::octara::model::motion::Curve::Fade(0.15)))),
        "pointer-events-none" => prop!(PointerEvents(false)),
        "pointer-events-auto" => prop!(PointerEvents(true)),
        "select-none" => layout!(UserSelect, UserSelect::None),
        "select-text" => layout!(UserSelect, UserSelect::Text),
        "rounded" => return rounded("sm"),
        "shadow" => prop!(Shadow(::octara::model::style::Shadows::Token(::octara::model::theme::ShadowToken::Sm))),
        _ => return utility_with_value(class),
    };
    Ok(vec![property])
}

fn flex_with_auto_basis(grow: f32, shrink: f32) -> Vec<TokenStream> {
    vec![prop!(Grow(#grow)), prop!(Shrink(#shrink)), prop!(Basis(::octara::model::style::Length::Auto))]
}

fn utility_with_value(class: &str) -> ClassResult<Vec<TokenStream>> {
    let property = if let Some(value) = class.strip_prefix("text-") {
        text(value)?
    } else if let Some(value) = class.strip_prefix("tracking-") {
        tracking(value)?
    } else if let Some(value) = class.strip_prefix("border-") {
        return border(value);
    } else if let Some(value) = class.strip_prefix("ring-offset-") {
        let offset = pixel_width(value)?.ok_or("expected a nonnegative ring offset in pixels")?;
        prop!(RingOffset(::octara::model::style::Length::Px(#offset)))
    } else if let Some(value) = class.strip_prefix("ring-") {
        if let Some(width) = pixel_width(value)? {
            prop!(RingWidth(::octara::model::style::Length::Px(#width)))
        } else {
            let color = color(value, "ring")?;
            prop!(RingColor(#color))
        }
    } else if let Some(value) = class.strip_prefix("bg-") {
        let color = color(value, "bg")?;
        prop!(Background(Some(::octara::model::theme::Fill::Solid(#color))))
    } else if let Some(value) = class.strip_prefix("opacity-") {
        let opacity = opacity(value)?;
        prop!(Opacity(#opacity))
    } else if let Some(value) = class.strip_prefix("rounded-") {
        return rounded(value);
    } else if let Some(value) = class.strip_prefix("z-") {
        let index = match value {
            "0" | "10" | "20" | "30" | "40" | "50" => value.parse(),
            _ => value
                .strip_prefix('[')
                .and_then(|value| value.strip_suffix(']'))
                .ok_or("expected z-0, z-10 through z-50, or z-[N]")?
                .parse(),
        }
        .map_err(|_| "expected a signed integer z-index")?;
        let index: i32 = index;
        prop!(ZIndex(#index))
    } else if let Some(value) = class.strip_prefix("shadow-") {
        let shadow = match value {
            "none" => "None",
            "2xs" => "Xs2",
            "xs" => "Xs",
            "sm" => "Sm",
            "md" => "Md",
            "lg" => "Lg",
            "xl" => "Xl",
            "2xl" => "Xl2",
            _ => return Err("unknown shadow"),
        };
        let shadow = format_ident!("{shadow}");
        prop!(Shadow(::octara::model::style::Shadows::Token(::octara::model::theme::ShadowToken::#shadow)))
    } else if let Some(value) = class.strip_prefix("font-") {
        let weight = match value {
            "thin" => "Thin",
            "extralight" => "ExtraLight",
            "light" => "Light",
            "normal" => "Normal",
            "medium" => "Medium",
            "semibold" => "Semibold",
            "bold" => "Bold",
            "extrabold" => "ExtraBold",
            "black" => "Black",
            _ => return Err("unknown font weight or family"),
        };
        let weight = format_ident!("{weight}");
        prop!(FontWeight(::octara::model::theme::FontWeightToken::#weight.value()))
    } else if let Some(value) = class.strip_prefix("leading-") {
        let line_height = quote!(::octara::model::style::LineHeight);
        if let Some(px) = bracketed_px(value)? {
            prop!(LineHeight(#line_height::Len(::octara::model::style::Length::Px(#px))))
        } else {
            let leading = match value {
                "none" => "None",
                "tight" => "Tight",
                "snug" => "Snug",
                "normal" => "Normal",
                "relaxed" => "Relaxed",
                "loose" => "Loose",
                _ => return Err("unknown line height"),
            };
            let leading = format_ident!("{leading}");
            prop!(LineHeight(#line_height::Relative(::octara::model::theme::LeadingToken::#leading.ratio())))
        }
    } else if let Some(value) = class.strip_prefix("grid-cols-") {
        let count = positive_count(value, 12).ok_or("expected 1 to 12 grid columns")?;
        prop!(GridCols(#count))
    } else if let Some(value) = class.strip_prefix("grid-rows-") {
        let count = positive_count(value, 6).ok_or("expected 1 to 6 grid rows")?;
        prop!(GridRows(#count))
    } else if let Some(value) = class.strip_prefix("col-span-") {
        return grid_span(value, format_ident!("ColStart"), format_ident!("ColEnd"));
    } else if let Some(value) = class.strip_prefix("row-span-") {
        return grid_span(value, format_ident!("RowStart"), format_ident!("RowEnd"));
    } else if let Some(value) = class.strip_prefix("line-clamp-") {
        let count = positive_count(value, 6).ok_or("expected a line clamp from 1 to 6")?;
        return Ok(vec![prop!(TextWrap(::octara::model::text::Wrap::Words)), prop!(MaxLines(Some(#count)))]);
    } else if let Some(value) = class.strip_prefix("overflow-") {
        return overflow(value);
    } else if let Some(value) = class.strip_prefix("cursor-") {
        cursor(value)?
    } else {
        return lengths(class);
    };
    Ok(vec![property])
}

fn cursor(value: &str) -> ClassResult<TokenStream> {
    let cursor = match value {
        "auto" => "Auto",
        "default" => "Arrow",
        "pointer" => "PointingHand",
        "wait" => "Wait",
        "text" => "IBeam",
        "move" => "Move",
        "help" => "Help",
        "not-allowed" => "OperationNotAllowed",
        "none" => "None",
        "context-menu" => "ContextMenu",
        "progress" => "Progress",
        "cell" => "Cell",
        "crosshair" => "Crosshair",
        "vertical-text" => "VerticalText",
        "alias" => "Alias",
        "copy" => "Copy",
        "no-drop" => "NoDrop",
        "grab" => "OpenHand",
        "grabbing" => "ClosedHand",
        "all-scroll" => "AllScroll",
        "col-resize" => "ResizeColumn",
        "row-resize" => "ResizeRow",
        "n-resize" => "ResizeNorth",
        "e-resize" => "ResizeEast",
        "s-resize" => "ResizeSouth",
        "w-resize" => "ResizeWest",
        "ne-resize" => "ResizeNorthEast",
        "nw-resize" => "ResizeNorthWest",
        "se-resize" => "ResizeSouthEast",
        "sw-resize" => "ResizeSouthWest",
        "ew-resize" => "ResizeLeftRight",
        "ns-resize" => "ResizeUpDown",
        "nesw-resize" => "ResizeNorthEastSouthWest",
        "nwse-resize" => "ResizeNorthWestSouthEast",
        "zoom-in" => "ZoomIn",
        "zoom-out" => "ZoomOut",
        _ => return Err("unknown cursor"),
    };
    let cursor = format_ident!("{cursor}");
    Ok(prop!(Cursor(::octara::model::input::CursorStyle::#cursor)))
}

fn grid_span(value: &str, start: proc_macro2::Ident, end: proc_macro2::Ident) -> ClassResult<Vec<TokenStream>> {
    let grid_pos = quote!(::octara::model::style::GridPos);
    if value == "full" {
        return Ok(vec![prop!(#start(#grid_pos::Line(1))), prop!(#end(#grid_pos::Line(-1)))]);
    }
    let count = positive_count(value, u16::MAX).ok_or("expected a positive grid span or `full`")?;
    Ok(vec![prop!(#start(#grid_pos::Auto)), prop!(#end(#grid_pos::Span(#count)))])
}

fn border(value: &str) -> ClassResult<Vec<TokenStream>> {
    let (side, width) = match value.split_once('-') {
        Some((side, width)) => (side, Some(width)),
        None => (value, None),
    };
    let edges: &[&str] = match side {
        "t" => &["BorderTop"],
        "r" => &["BorderRight"],
        "b" => &["BorderBottom"],
        "l" => &["BorderLeft"],
        "x" => &["BorderLeft", "BorderRight"],
        "y" => &["BorderTop", "BorderBottom"],
        _ => {
            if let Some(width) = pixel_width(value)? {
                return Ok(border_width(BORDER_EDGES, width));
            }
            let color = color(value, "border")?;
            return Ok(vec![prop!(BorderColor(#color))]);
        }
    };
    let width = match width {
        Some(width) => pixel_width(width)?.ok_or("expected a nonnegative border width in pixels")?,
        None => 1.0,
    };
    Ok(border_width(edges, width))
}

fn border_width(edges: &[&str], width: f32) -> Vec<TokenStream> {
    length_properties(edges, quote!(::octara::model::style::Length::Px(#width)))
}

fn rounded(value: &str) -> ClassResult<Vec<TokenStream>> {
    let (side, size) = value.split_once('-').unwrap_or((value, "sm"));
    let (corners, size): (&[&str], _) = match side {
        "t" => (&["RadiusTopLeft", "RadiusTopRight"], size),
        "r" => (&["RadiusTopRight", "RadiusBottomRight"], size),
        "b" => (&["RadiusBottomRight", "RadiusBottomLeft"], size),
        "l" => (&["RadiusBottomLeft", "RadiusTopLeft"], size),
        "tl" => (&["RadiusTopLeft"], size),
        "tr" => (&["RadiusTopRight"], size),
        "br" => (&["RadiusBottomRight"], size),
        "bl" => (&["RadiusBottomLeft"], size),
        _ => (&["RadiusTopLeft", "RadiusTopRight", "RadiusBottomRight", "RadiusBottomLeft"], value),
    };
    let radius = quote!(::octara::model::style::Radius);
    let value = if size == "full" {
        quote!(#radius::Full)
    } else if let Some(pixels) = bracketed_px(size)? {
        quote!(#radius::Px(#pixels))
    } else {
        let token = match size {
            "none" => "None",
            "xs" => "Xs",
            "sm" => "Sm",
            "md" => "Md",
            "lg" => "Lg",
            "xl" => "Xl",
            "2xl" => "Xl2",
            "3xl" => "Xl3",
            "4xl" => "Xl4",
            _ => return Err("unknown radius"),
        };
        let token = format_ident!("{token}");
        quote!(#radius::Token(::octara::model::theme::RadiusToken::#token))
    };
    Ok(length_properties(corners, value))
}

fn text(value: &str) -> ClassResult<TokenStream> {
    if let Some(size) = bracketed_px(value)? {
        return Ok(prop!(FontSize(::octara::model::style::FontSize::Len(::octara::model::style::Length::Px(#size)))));
    }
    let size = match value {
        "xs" => "Xs",
        "sm" => "Sm",
        "base" => "Base",
        "lg" => "Lg",
        "xl" => "Xl",
        "2xl" => "Xl2",
        "3xl" => "Xl3",
        "4xl" => "Xl4",
        "5xl" => "Xl5",
        "6xl" => "Xl6",
        "7xl" => "Xl7",
        "8xl" => "Xl8",
        "9xl" => "Xl9",
        "left" => return Ok(prop!(TextAlign(::octara::model::text::TextAlign::Left))),
        "right" => return Ok(prop!(TextAlign(::octara::model::text::TextAlign::Right))),
        "start" => return Ok(prop!(TextAlign(::octara::model::text::TextAlign::Start))),
        "end" => return Ok(prop!(TextAlign(::octara::model::text::TextAlign::End))),
        "center" => return Ok(prop!(TextAlign(::octara::model::text::TextAlign::Center))),
        "justify" => return Ok(prop!(TextAlign(::octara::model::text::TextAlign::Justify))),
        _ => {
            let color = color(value, "text")?;
            return Ok(prop!(TextColor(#color)));
        }
    };
    let size = format_ident!("{size}");
    Ok(prop!(FontSize(::octara::model::style::FontSize::Token(::octara::model::theme::FontSizeToken::#size))))
}

fn color(value: &str, slot: &str) -> ClassResult<TokenStream> {
    let (name, alpha) = match value.split_once('/') {
        Some((name, alpha)) => (name, Some(opacity(alpha)?)),
        None => (value, None),
    };
    let value_type = quote!(::octara::model::theme::ColorValue);
    if let Some(color) = palette_color(name)? {
        return Ok(match alpha {
            Some(alpha) if name != "transparent" => quote!(#value_type::Color(#color.with_alpha(#alpha))),
            _ => quote!(#value_type::Color(#color)),
        });
    }
    let token = match (slot, name) {
        (_, "background") => "Background",
        (_, "surface") => "Surface",
        (_, "surface-elevated") => "SurfaceElevated",
        (_, "surface-hover") => "SurfaceHover",
        (_, "surface-pressed") => "SurfacePressed",
        (_, "surface-selected") => "SurfaceSelected",
        (_, "text") => "Text",
        ("text", "muted") | (_, "text-muted") => "TextMuted",
        ("text", "subtle") | (_, "text-subtle") => "TextSubtle",
        (_, "border") => "Border",
        ("border", "subtle") | (_, "border-subtle") => "BorderSubtle",
        ("border", "strong") | (_, "border-strong") => "BorderStrong",
        (_, "accent") => "Accent",
        (_, "accent-hover") => "AccentHover",
        (_, "accent-text") => "AccentText",
        (_, "focus") => "Focus",
        (_, "selection") => "Selection",
        (_, "info") => "Info",
        (_, "success") => "Success",
        (_, "warning") => "Warning",
        (_, "danger") => "Danger",
        _ => return Err("unknown semantic or palette color"),
    };
    let token = format_ident!("{token}");
    let base = quote!(::octara::model::theme::ColorToken::#token);
    Ok(match alpha {
        Some(alpha) => quote!(#value_type::Alpha { base: #base, alpha: #alpha }),
        None => quote!(#value_type::Token(#base)),
    })
}

fn palette_color(name: &str) -> ClassResult<Option<TokenStream>> {
    let constant = match name {
        "black" => Some("BLACK"),
        "white" => Some("WHITE"),
        "transparent" => Some("TRANSPARENT"),
        _ => None,
    };
    if let Some(constant) = constant {
        let constant = format_ident!("{constant}");
        return Ok(Some(quote!(::octara::model::theme::palette::#constant)));
    }
    let Some((family, shade)) = name.split_once('-') else {
        return Ok(None);
    };
    let family = match family {
        "slate" | "gray" | "zinc" | "neutral" | "stone" | "red" | "orange" | "amber" | "yellow" | "lime" | "green" | "emerald" | "teal"
        | "cyan" | "sky" | "blue" | "indigo" | "violet" | "purple" | "fuchsia" | "pink" | "rose" => {
            format_ident!("{}", family.to_ascii_uppercase())
        }
        _ => return Ok(None),
    };
    let shade = match shade {
        "50" | "100" | "200" | "300" | "400" | "500" | "600" | "700" | "800" | "900" | "950" => {
            format_ident!("S{shade}")
        }
        _ => return Err("expected a palette shade from 50 to 950: 50, 100, 200, …, 900, or 950"),
    };
    Ok(Some(quote!(::octara::model::theme::palette::#family.shade(::octara::model::theme::palette::Shade::#shade))))
}

fn overflow(value: &str) -> ClassResult<Vec<TokenStream>> {
    let (axes, value) = match value.split_once('-') {
        Some(("x", value)) => (&["OverflowX"][..], value),
        Some(("y", value)) => (&["OverflowY"][..], value),
        _ => (&["OverflowX", "OverflowY"][..], value),
    };
    let overflow = match value {
        "visible" => "Visible",
        "clip" => "Clip",
        "hidden" => "Hidden",
        "scroll" => "Scroll",
        "auto" => "Auto",
        _ => return Err("unknown overflow mode"),
    };
    let overflow = format_ident!("{overflow}");
    Ok(axes
        .iter()
        .map(|property| {
            let property = format_ident!("{property}");
            prop!(#property(::octara::model::style::Overflow::#overflow))
        })
        .collect())
}

fn lengths(class: &str) -> ClassResult<Vec<TokenStream>> {
    let (class, sign) = class.strip_prefix('-').map_or((class, 1.0f32), |class| (class, -1.0));
    let (kind, value) = class.rsplit_once('-').ok_or("unknown utility")?;
    let properties: &[&str] = match kind {
        "p" => &["PaddingLeft", "PaddingRight", "PaddingTop", "PaddingBottom"],
        "px" => &["PaddingLeft", "PaddingRight"],
        "py" => &["PaddingTop", "PaddingBottom"],
        "pt" => &["PaddingTop"],
        "pr" => &["PaddingRight"],
        "pb" => &["PaddingBottom"],
        "pl" => &["PaddingLeft"],
        "m" => &["MarginLeft", "MarginRight", "MarginTop", "MarginBottom"],
        "mx" => &["MarginLeft", "MarginRight"],
        "my" => &["MarginTop", "MarginBottom"],
        "mt" => &["MarginTop"],
        "mr" => &["MarginRight"],
        "mb" => &["MarginBottom"],
        "ml" => &["MarginLeft"],
        "inset" => &["Top", "Right", "Bottom", "Left"],
        "inset-x" => &["Left", "Right"],
        "inset-y" => &["Top", "Bottom"],
        "top" => &["Top"],
        "right" => &["Right"],
        "bottom" => &["Bottom"],
        "left" => &["Left"],
        "size" => &["Width", "Height"],
        "w" => &["Width"],
        "h" => &["Height"],
        "min-w" => &["MinWidth"],
        "min-h" => &["MinHeight"],
        "max-w" => &["MaxWidth"],
        "max-h" => &["MaxHeight"],
        "basis" => &["Basis"],
        "gap" => &["Gap"],
        "gap-x" => &["GapX"],
        "gap-y" => &["GapY"],
        _ => return Err("unknown utility"),
    };
    let allow_negative = matches!(
        kind,
        "inset" | "inset-x" | "inset-y" | "top" | "right" | "bottom" | "left" | "m" | "mx" | "my" | "mt" | "mr" | "mb" | "ml"
    );
    let sizing = matches!(kind, "w" | "h" | "min-w" | "min-h" | "max-w" | "max-h" | "size" | "basis");
    if sign < 0.0 && !allow_negative {
        return Err("negative values are only supported for margins and insets");
    }
    let length = quote!(::octara::model::style::Length);
    let value = match (value, bracketed_px(value)?) {
        (_, Some(px)) => {
            let px = sign * px;
            quote!(#length::Px(#px))
        }
        ("px", _) => quote!(#length::Px(#sign)),
        ("full", _) => quote!(#length::Pct(#sign)),
        ("auto", _) if sign > 0.0 && (allow_negative || sizing) => quote!(#length::Auto),
        (value, _) if sizing && let Some(container) = container(value) => {
            quote!(#length::Container(::octara::model::theme::ContainerWidthToken::#container))
        }
        (value, _) => match value.split_once('/') {
            Some((numerator, denominator)) => {
                let numerator = decimal(numerator).ok_or("expected a finite length fraction")?;
                let denominator = decimal(denominator).ok_or("expected a finite length fraction")?;
                let fraction = sign * numerator / denominator;
                if !fraction.is_finite() {
                    return Err("expected a finite length fraction");
                }
                quote!(#length::Pct(#fraction))
            }
            None => {
                let steps = sign * decimal(value).ok_or("expected a finite spacing value")?;
                quote!(#length::Spacing(#steps))
            }
        },
    };
    Ok(length_properties(properties, value))
}

fn length_properties(properties: &[&str], length: TokenStream) -> Vec<TokenStream> {
    properties
        .iter()
        .map(|property| {
            let property = format_ident!("{property}");
            prop!(#property(#length))
        })
        .collect()
}

fn container(value: &str) -> Option<proc_macro2::Ident> {
    let container = match value {
        "3xs" => "Xs3",
        "2xs" => "Xs2",
        "xs" => "Xs",
        "sm" => "Sm",
        "md" => "Md",
        "lg" => "Lg",
        "xl" => "Xl",
        "2xl" => "Xl2",
        "3xl" => "Xl3",
        "4xl" => "Xl4",
        "5xl" => "Xl5",
        "6xl" => "Xl6",
        "7xl" => "Xl7",
        _ => return None,
    };
    Some(format_ident!("{container}"))
}

fn bracketed_px(value: &str) -> ClassResult<Option<f32>> {
    let Some(value) = value.strip_prefix('[') else {
        return Ok(None);
    };
    value.strip_suffix("px]").and_then(decimal).map(Some).ok_or("expected a finite, bracketed pixel length, for example `[13px]`")
}

fn tracking(value: &str) -> ClassResult<TokenStream> {
    let em: f32 = match value {
        "tighter" => -0.05,
        "tight" => -0.025,
        "normal" => 0.0,
        "wide" => 0.025,
        "wider" => 0.05,
        "widest" => 0.1,
        _ => {
            let value = value.strip_prefix('[').and_then(|value| value.strip_suffix("px]")).ok_or("unknown tracking value")?;
            let px = if let Some(value) = value.strip_prefix('-') { decimal(value).map(|px| -px) } else { decimal(value) }
                .ok_or("expected a finite tracking value in pixels")?;
            return Ok(prop!(LetterSpacing(#px)));
        }
    };
    Ok(prop!(LetterSpacingEm(#em)))
}

fn pixel_width(value: &str) -> ClassResult<Option<f32>> {
    Ok(bracketed_px(value)?.or_else(|| decimal(value)))
}

fn decimal(value: &str) -> Option<f32> {
    if value.bytes().all(|ch| ch.is_ascii_digit() || ch == b'.') {
        value.parse::<f32>().ok().filter(|value| value.is_finite())
    } else {
        None
    }
}

fn opacity(value: &str) -> ClassResult<f32> {
    decimal(value).filter(|percent| *percent <= 100.0).map(|percent| percent / 100.0).ok_or("expected an opacity percentage from 0 to 100")
}

fn positive_count(value: &str, max: u16) -> Option<u16> {
    if value.bytes().all(|ch| ch.is_ascii_digit()) { value.parse::<u16>().ok().filter(|count| (1..=max).contains(count)) } else { None }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proc_macro2::Span;

    fn classes(source: &str) -> syn::Result<TokenStream> {
        expand(&LitStr::new(source, Span::call_site()))
    }

    #[test]
    fn expands_properties_and_conditional_slices_in_authored_order() {
        let actual = classes("p-2 hover:bg-surface/80 hover:text-muted p-[13px] dark:p-px").unwrap().to_string();
        assert!(actual.starts_with("const { const BASE"), "{actual}");
        assert!(actual.contains("TwStyle :: new (BASE , OVERRIDES)"), "{actual}");
        let (base, conditional) = actual.split_once("When :: State").unwrap();
        let (theme_spacing, pixels) = base.split_once("Length :: Px (13f32)").unwrap();
        assert_eq!(theme_spacing.matches("Length :: Spacing (2f32)").count(), 4);
        assert_eq!(pixels.matches("Length :: Px (13f32)").count(), 3);

        let (hover, dark) = conditional.split_once("When :: Dark").unwrap();
        assert!(hover.contains("State :: HOVER"));
        let (background, text) = hover.split_once("StyleProp :: TextColor").unwrap();
        assert!(background.contains("ColorValue :: Alpha"));
        assert!(background.contains("ColorToken :: Surface , alpha : 0.8f32"));
        assert!(text.contains("ColorToken :: TextMuted"));
        assert_eq!(dark.matches("Length :: Px (1f32)").count(), 4);
    }

    #[test]
    fn retains_order_between_separate_hover_groups() {
        let actual = classes("group-hover/row:p-1 group-hover/card:bg-red-500 group-hover/row:bg-blue-500").unwrap().to_string();
        let groups = actual.matches("When :: GroupHover").count();
        assert_eq!(groups, 3, "{actual}");
        assert!(actual.find("RED").unwrap() < actual.find("BLUE").unwrap());
    }

    #[test]
    fn accepts_all_default_palette_shades_with_opacity() {
        let families = [
            "slate", "gray", "zinc", "neutral", "stone", "red", "orange", "amber", "yellow", "lime", "green", "emerald", "teal", "cyan",
            "sky", "blue", "indigo", "violet", "purple", "fuchsia", "pink", "rose",
        ];
        for family in families {
            for shade in [50, 100, 200, 300, 400, 500, 600, 700, 800, 900, 950] {
                let class = format!("bg-{family}-{shade}/80");
                let actual = classes(&class).unwrap().to_string();
                assert!(actual.contains(&format!("palette :: {} . shade", family.to_ascii_uppercase())), "{class}: {actual}");
                assert!(actual.contains(&format!("Shade :: S{shade}")), "{class}: {actual}");
                assert!(actual.contains("with_alpha (0.8f32)"), "{class}: {actual}");
            }
        }
    }

    #[test]
    fn accepts_wave_one_layout_and_interaction_utilities() {
        let families = [
            "hidden flex flex-row flex-row-reverse flex-col flex-col-reverse flex-wrap flex-wrap-reverse flex-nowrap",
            "flex-1 flex-auto flex-initial flex-none grow grow-0 shrink shrink-0 grid col-span-2 col-span-full row-span-2 row-span-full",
            "static relative absolute fixed sticky inset-0 inset-x-1.5 inset-y-px top-1/2 right-0 bottom-[13px] -left-1",
            "items-start items-end items-center items-baseline items-stretch items-normal",
            "self-auto self-start self-end self-center self-baseline self-stretch",
            "justify-start justify-end justify-center justify-between justify-around justify-evenly justify-normal justify-stretch",
            "mx-auto ml-auto mr-auto aspect-square aspect-video",
            "border border-0 border-2 border-4 border-t border-r-0 border-b-2 border-l-4 border-x-2 border-y-4 border-border/60",
            "ring ring-0 ring-1 ring-2 ring-4 ring-offset-2 ring-offset-[13px] ring-focus/50 ring-blue-500/25",
            "transition-colors transition-opacity transition-all truncate whitespace-nowrap",
            "text-left text-center text-right text-justify font-sans font-mono",
            "cursor-auto cursor-default cursor-pointer cursor-wait cursor-text cursor-move cursor-help cursor-not-allowed cursor-none",
            "cursor-context-menu cursor-progress cursor-cell cursor-crosshair cursor-vertical-text cursor-alias cursor-copy cursor-no-drop",
            "cursor-grab cursor-grabbing cursor-all-scroll cursor-col-resize cursor-row-resize cursor-n-resize cursor-e-resize",
            "cursor-s-resize cursor-w-resize cursor-ne-resize cursor-nw-resize cursor-se-resize cursor-sw-resize",
            "cursor-ew-resize cursor-ns-resize cursor-nesw-resize cursor-nwse-resize cursor-zoom-in cursor-zoom-out",
            "select-none select-text pointer-events-none pointer-events-auto",
            "hover:flex active:flex focus:flex focus-visible:flex focus-within:flex disabled:flex selected:flex checked:flex expanded:flex",
            "drag-over:flex dark:flex light:flex touch:flex group-hover:flex group-hover/row:flex sm:flex md:flex lg:flex xl:flex 2xl:flex",
        ];
        for class in families.into_iter().flat_map(str::split_whitespace) {
            assert!(classes(class).is_ok(), "{class}");
        }
        for axis in ["overflow", "overflow-x", "overflow-y"] {
            for value in ["auto", "scroll", "visible", "clip", "hidden"] {
                let class = format!("{axis}-{value}");
                assert!(classes(&class).is_ok(), "{class}");
            }
        }
        for axis in ["w", "h", "size", "min-w", "min-h", "max-w", "max-h"] {
            for value in [
                "full", "auto", "0", "1/3", "2.5", "px", "[13px]", "3xs", "2xs", "xs", "sm", "md", "lg", "xl", "2xl", "3xl", "4xl", "5xl",
                "6xl", "7xl",
            ] {
                let class = format!("{axis}-{value}");
                assert!(classes(&class).is_ok(), "{class}");
            }
        }
        for (kind, max) in [("grid-cols", 12), ("grid-rows", 6), ("line-clamp", 6), ("opacity", 100)] {
            let min = u16::from(kind != "opacity");
            for n in min..=max {
                let class = format!("{kind}-{n}");
                assert!(classes(&class).is_ok(), "{class}");
            }
        }
    }

    #[test]
    fn distinguishes_overflow_auto_from_explicit_scrolling() {
        let actual = classes("overflow-x-auto overflow-y-scroll").unwrap().to_string();
        let (horizontal, vertical) = actual.split_once("StyleProp :: OverflowY").unwrap();
        assert!(horizontal.contains("StyleProp :: OverflowX") && horizontal.contains("Overflow :: Auto"), "{actual}");
        assert!(vertical.contains("Overflow :: Scroll"), "{actual}");
    }

    #[test]
    fn emits_independent_border_ring_and_interaction_properties() {
        let actual = classes(
            "border-2 border-x-0 ring-2 ring-offset-4 ring-focus/50 group-hover:cursor-pointer md:select-text disabled:pointer-events-none",
        )
        .unwrap()
        .to_string();
        assert_eq!(actual.matches("StyleProp :: BorderLeft").count(), 2, "{actual}");
        assert_eq!(actual.matches("StyleProp :: BorderRight").count(), 2, "{actual}");
        assert_eq!(actual.matches("StyleProp :: BorderTop").count(), 1, "{actual}");
        assert_eq!(actual.matches("StyleProp :: BorderBottom").count(), 1, "{actual}");
        for fragment in [
            "StyleProp :: RingWidth",
            "StyleProp :: RingOffset",
            "StyleProp :: RingColor",
            "ColorToken :: Focus , alpha : 0.5f32",
            "When :: GroupHover (\"group\")",
            "CursorStyle :: PointingHand",
            "ResponsiveBreakpoint :: Md",
            "UserSelect :: Text",
            "State :: DISABLED",
            "StyleProp :: PointerEvents (false)",
        ] {
            assert!(actual.contains(fragment), "{fragment}: {actual}");
        }
    }

    #[test]
    fn reports_the_invalid_class_and_reason() {
        for (class, reason) in [
            ("made-up", "unknown utility"),
            ("hover:made-up", "unknown utility"),
            ("made-up:flex", "unknown condition"),
            ("dark:hover:flex", "only one condition"),
            ("group-hover/:flex", "expected a group name"),
            ("p-999999999999999999999999999999999999999999", "finite spacing"),
            ("-p-2", "negative values"),
            ("p-[13rem]", "bracketed pixel length"),
            ("w-1/0", "finite length fraction"),
            ("bg-rose-150", "expected a palette shade"),
            ("border-border/101", "opacity percentage"),
            ("text-white/NaN", "opacity percentage"),
            ("opacity-101", "opacity percentage"),
            ("grid-cols-13", "1 to 12 grid columns"),
            ("grid-rows-7", "1 to 6 grid rows"),
            ("col-span-0", "positive grid span"),
            ("line-clamp-7", "line clamp from 1 to 6"),
            ("border-t-", "nonnegative border width"),
            ("ring-offset--1", "nonnegative ring offset"),
            ("cursor-[url(hand.cur)]", "unknown cursor"),
        ] {
            let error = classes(&format!("flex {class}")).unwrap_err().to_string();
            assert!(error.contains(&format!("`{class}`")) && error.contains(reason), "{error}");
        }
    }
}
