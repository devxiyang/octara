//! Projection and logical offsets for rich DOM text.

use crate::{css, dom, image};
use octara_model::geometry::{Edges, Size};
use octara_model::paint::{CanvasOp, FillRule, PathStyle};
use octara_model::style::Visual;
use octara_model::text::{InlineAlign, InlineAttachment, InlineContent};
use octara_model::theme::{Fill, Theme};
use std::ops::Range;
use wasm_bindgen::{JsCast, JsValue};
use web_sys::{Document, Element, HtmlCanvasElement, HtmlElement, Node};

const SOURCE: &str = "data-octara-inline-source";
const MEDIA_INDEX: &str = "data-octara-inline-media";
const SVG: &str = "http://www.w3.org/2000/svg";

pub(crate) fn has_images(attachments: &[InlineAttachment]) -> bool {
    attachments.iter().any(|item| match &item.content {
        InlineContent::Image { .. } => true,
        InlineContent::Vector { ops } => ops.iter().any(|op| matches!(op, CanvasOp::Image { .. })),
        _ => false,
    })
}

type Piece<'a> = (Range<usize>, Option<(usize, &'a InlineAttachment)>);

pub(crate) fn pieces<'a>(
    value: &str,
    ranges: impl IntoIterator<Item = &'a Range<usize>>,
    attachments: &'a [InlineAttachment],
) -> Vec<Piece<'a>> {
    let mut boundaries = vec![0, value.len()];
    for range in ranges.into_iter().chain(attachments.iter().map(|item| &item.range)) {
        boundaries.extend([range.start, range.end]);
    }
    boundaries.sort_unstable();
    boundaries.dedup();
    let mut pieces = Vec::new();
    let mut attachments = attachments.iter().enumerate().peekable();
    let mut skip = 0;
    for (index, &start) in boundaries.iter().enumerate() {
        while attachments.peek().is_some_and(|(_, attachment)| attachment.range.start == start) {
            let (index, attachment) = attachments.next().expect("attachment at boundary");
            skip = attachment.range.end;
            pieces.push((attachment.range.clone(), Some((index, attachment))));
        }
        if start >= skip
            && let Some(&end) = boundaries.get(index + 1)
        {
            pieces.push((start..end, None));
        }
    }
    pieces
}

pub(crate) fn attachment(
    doc: &Document,
    index: usize,
    attachment: &InlineAttachment,
    source: &str,
    visual: Visual,
    theme: &Theme,
) -> Result<HtmlElement, JsValue> {
    let Size { width, height } = attachment.layout_size();
    let align = match attachment.align {
        InlineAlign::Baseline => {
            attachment.layout_baseline().map_or_else(|| "baseline".into(), |baseline| format!("{}px", baseline - height))
        }
        InlineAlign::CapHeightCenter => format!("calc(0.5cap - {}px)", height * 0.5),
        InlineAlign::Middle => "middle".into(),
        InlineAlign::Bottom => "bottom".into(),
    };
    let element = dom::html(doc, "span")?;
    element.set_attribute(SOURCE, source)?;
    element.set_attribute("contenteditable", "false")?;
    if source.is_empty() || (width == 0.0 && matches!(attachment.content, InlineContent::Spacer { .. })) {
        element.set_attribute("aria-hidden", "true")?;
    } else {
        element.set_attribute("role", "img")?;
        element.set_attribute("aria-label", source)?;
    }
    element.set_attribute(
        "style",
        &format!("display:inline-block;position:relative;overflow:hidden;width:{width}px;height:{height}px;vertical-align:{align};"),
    )?;
    if width == 0.0 || height == 0.0 {
        return Ok(element);
    }
    match &attachment.content {
        InlineContent::Spacer { .. } => {}
        InlineContent::ImageSource { radius, .. } => {
            element.set_attribute(MEDIA_INDEX, &index.to_string())?;
            element.style().set_property("border-radius", &format!("{}px", radius.max(0.0)))?;
        }
        InlineContent::Icon { icon, color } => {
            let svg = dom::icon(doc, icon)?;
            if let Some(color) = color {
                svg.set_attribute("color", &css::color(color.resolve(theme).0))?;
            }
            element.append_child(&svg)?;
        }
        InlineContent::Image { image: source, fit, radius } => {
            let scratch = dom::html(doc, "canvas")?.dyn_into::<HtmlCanvasElement>()?;
            let sample = source.sample();
            if sample.frame().width != 0 && sample.frame().height != 0 {
                image::upload(&scratch, Some(&sample))?;
                let img = dom::html(doc, "img")?;
                img.set_attribute("src", &scratch.to_data_url()?)?;
                img.set_attribute("alt", "")?;
                img.set_attribute("draggable", "false")?;
                let fit = match fit {
                    octara_model::paint::ObjectFit::Fill => "fill",
                    octara_model::paint::ObjectFit::Contain => "contain",
                    octara_model::paint::ObjectFit::Cover => "cover",
                    octara_model::paint::ObjectFit::ScaleDown => "scale-down",
                    octara_model::paint::ObjectFit::None => "none",
                };
                img.set_attribute(
                    "style",
                    &format!(
                        "display:block;width:100%;height:100%;object-fit:{fit};border-radius:{}px;pointer-events:none;",
                        radius.max(0.0)
                    ),
                )?;
                element.append_child(&img)?;
            }
        }
        InlineContent::Vector { ops } => {
            if let Some(svg) = paths(doc, ops, attachment.layout_size(), theme)? {
                element.append_child(&svg)?;
            } else {
                let samples = ops.iter().filter_map(|op| match op {
                    CanvasOp::Image { image, .. } => Some(image.sample()),
                    _ => None,
                });
                let scale = doc.default_view().map_or(1.0, |window| window.device_pixel_ratio() as f32);
                let size = image::CanvasSize { area: attachment.layout_size(), padding: Edges::default(), scale };
                let canvas = image::draw(doc, ops, samples, size, visual, theme)?;
                element.append_child(&canvas)?;
            }
        }
    }
    Ok(element)
}

pub(crate) fn media_element(owner: &HtmlElement, index: usize) -> Option<HtmlElement> {
    owner.query_selector(&format!("[{MEDIA_INDEX}=\"{index}\"]")).ok().flatten()?.dyn_into().ok()
}

fn paths(doc: &Document, ops: &[CanvasOp], size: Size, theme: &Theme) -> Result<Option<Element>, JsValue> {
    if !ops.iter().all(|op| matches!(op, CanvasOp::Path { fill: Fill::Solid(_), .. })) {
        return Ok(None);
    }
    let svg = doc.create_element_ns(Some(SVG), "svg")?;
    svg.set_attribute("viewBox", &format!("0 0 {} {}", size.width, size.height))?;
    svg.set_attribute("width", "100%")?;
    svg.set_attribute("height", "100%")?;
    svg.set_attribute("aria-hidden", "true")?;
    svg.set_attribute("style", "display:block;pointer-events:none;")?;
    for op in ops {
        let CanvasOp::Path { path, style, fill: Fill::Solid(color), transform: t } = op else { unreachable!() };
        let element = doc.create_element_ns(Some(SVG), "path")?;
        element.set_attribute("d", &dom::svg_path(path))?;
        element.set_attribute("transform", &format!("matrix({} {} {} {} {} {})", t.xx, t.xy, t.yx, t.yy, t.x, t.y))?;
        let color = css::color(color.resolve(theme).0);
        match style {
            PathStyle::Fill(rule) => {
                element.set_attribute("fill", &color)?;
                if *rule == FillRule::EvenOdd {
                    element.set_attribute("fill-rule", "evenodd")?;
                }
            }
            PathStyle::Stroke(stroke) => {
                dom::svg_stroke(&element, stroke, &color)?;
            }
        }
        svg.append_child(&element)?;
    }
    Ok(Some(svg))
}

fn source(node: &Node) -> Option<String> {
    node.dyn_ref::<Element>()?.get_attribute(SOURCE)
}

fn children(node: &Node) -> impl Iterator<Item = Node> {
    std::iter::successors(node.first_child(), Node::next_sibling)
}

pub(crate) fn logical_text(node: &Node) -> String {
    fn append(node: &Node, text: &mut String) {
        if let Some(value) = source(node).or_else(|| node.node_value()) {
            text.push_str(&value);
        } else {
            for child in children(node) {
                append(&child, text);
            }
        }
    }

    let mut text = String::new();
    append(node, &mut text);
    text
}

pub(crate) fn logical_offset(owner: &HtmlElement, endpoint: &Node, offset: u32) -> Option<usize> {
    let doc = owner.owner_document()?;
    let range = doc.create_range().ok()?;
    range.set_start(owner, 0).ok()?;
    if let Some(object) = std::iter::successors(Some(endpoint.clone()), Node::parent_node).find(|node| source(node).is_some()) {
        if offset == 0 {
            range.set_end_before(&object).ok()?;
        } else {
            range.set_end_after(&object).ok()?;
        }
    } else {
        range.set_end(endpoint, offset).ok()?;
    }
    let prefix = range.clone_contents().ok()?;
    Some(logical_text(&prefix).len())
}

pub(crate) fn placeholder(doc: &Document, value: &str, theme: &Theme) -> Result<HtmlElement, JsValue> {
    let element = dom::html(doc, "span")?;
    element.set_attribute(SOURCE, "")?;
    element.set_attribute("aria-hidden", "true")?;
    element.set_attribute("contenteditable", "false")?;
    let color = css::color(theme.get(octara_model::theme::ColorToken::TextMuted));
    element.set_attribute("style", &format!("position:absolute;pointer-events:none;color:{color};"))?;
    element.set_text_content(Some(value));
    Ok(element)
}

pub(crate) fn selection(element: &HtmlElement) -> Option<(usize, usize)> {
    let selection = element.owner_document()?.get_selection().ok()??;
    let anchor = selection.anchor_node()?;
    let caret = selection.focus_node()?;
    if !element.contains(Some(&anchor)) || !element.contains(Some(&caret)) {
        return None;
    }
    Some((logical_offset(element, &anchor, selection.anchor_offset())?, logical_offset(element, &caret, selection.focus_offset())?))
}

fn endpoint(node: &Node, byte: &mut usize) -> Option<(Node, u32)> {
    if let Some(value) = node.node_value() {
        if *byte <= value.len() {
            return Some((node.clone(), value[..value.floor_char_boundary(*byte)].encode_utf16().count() as u32));
        }
        *byte -= value.len();
        return None;
    }
    for (index, child) in children(node).enumerate() {
        let indivisible =
            child.dyn_ref::<Element>().is_some_and(|element| element.get_attribute("contenteditable").as_deref() == Some("false"));
        if indivisible {
            let size = logical_text(&child).len();
            if *byte <= size {
                return Some((node.clone(), index as u32 + u32::from(*byte > size / 2)));
            }
            *byte -= size;
        } else if let Some(point) = endpoint(&child, byte) {
            return Some(point);
        }
    }
    None
}

pub(crate) fn point_at(element: &HtmlElement, mut byte: usize) -> (Node, u32) {
    endpoint(element, &mut byte).unwrap_or_else(|| (element.clone().into(), element.child_nodes().length()))
}

pub(crate) fn select(element: &HtmlElement, endpoints: (usize, usize)) -> Result<(), JsValue> {
    if let Some(selection) = element.owner_document().and_then(|doc| doc.get_selection().ok().flatten()) {
        let (anchor, start) = point_at(element, endpoints.0);
        let (caret, end) = point_at(element, endpoints.1);
        selection.set_base_and_extent(&anchor, start, &caret, end)?;
    }
    Ok(())
}
