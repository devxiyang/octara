//! Inline ranges, accessible spoiler activation, and link pill projection.

use super::{MarkdownLocale, RenderBlock};
use crate::{
    BlockSlot, ImageOccurrence, MarkdownTheme, SmartCopy,
    ast::{Inline, Style},
    pills::{LinkPillContent, RecognizedLink},
    store::inline_images,
};
use octara::{InlinePill, PillStyle, prelude::*};
use std::{ops::Range, sync::Arc};

#[derive(Clone, Default, PartialEq)]
struct InlineOutput {
    rich: Rich,
    spoilers: Vec<usize>,
    images: Vec<ImageOccurrence>,
}

pub(super) fn inline_view(
    build: &mut BuildCx<'_>,
    block: &RenderBlock,
    run: impl Fn(&mut ReadCx<'_>) -> usize + 'static,
    source: impl Fn(&mut ReadCx<'_>) -> Inline + 'static,
) -> El {
    let locale = MarkdownLocale::get(build);
    let revealed = build.signal(Vec::<usize>::new());
    let overlay = block.context.overlay;
    let block_source = block.source;
    let image_actions = block.renderer().on_image_press().is_some() || block.renderer().on_context_menu().is_some();
    let overlays = build.memo(move |r| {
        let run = run(r);
        r.with2(block_source, overlay, |block, overlay| {
            let Some(block) = block.as_deref() else { return (Arc::from([]), Vec::new()) };
            let links = overlay.recognized_for(block, run);
            let images = if image_actions {
                overlay.images_in(block).filter(|image| image.inline.as_ref().is_some_and(|(index, _)| *index == run)).cloned().collect()
            } else {
                Vec::new()
            };
            (links, images)
        })
    });
    let view_block = block.clone();
    let plan = build.memo(move |r| {
        let inline = source(r);
        let theme = view_block.theme(r);
        let revealed = r.get(revealed);
        let (recognized, images) = r.get(overlays);
        Arc::new(InlinePlan::new(inline, &recognized, images, &revealed, &view_block, &theme, locale))
    });

    let renderer = block.context.renderer.clone();
    let metadata_renderer = renderer.clone();
    let identity = Arc::<str>::from(format!("octara-inline:{:?}/", build.scope()));
    let link_prefix = identity.clone();
    let output = build.memo(move |r| {
        let plan = r.get(plan);
        let rich = plan.draw(locale, &link_prefix, |url| metadata_renderer.link_pill_content(r, url));
        InlineOutput { rich, spoilers: plan.spoilers.clone(), images: plan.images.clone() }
    });
    let action_prefix = identity.clone();
    let action_renderer = renderer.clone();
    let node = text(move |r: &mut ReadCx<'_>| r.with(output, |out| out.rich.clone())).on_link(move |w, event| {
        let Some(suffix) = event.href.strip_prefix(action_prefix.as_ref()) else { return false };
        if let Some(at) = action_index(suffix, "spoiler/")
            && w.with(output, |out| out.spoilers.contains(&at))
        {
            if event.kind == LinkEventKind::Press {
                w.update(revealed, |revealed| revealed.push(at));
            }
            return true;
        }
        if let Some(index) = action_index(suffix, "image/")
            && let Some(image) = w.with(output, |out| out.image(index).cloned())
        {
            if event.kind == LinkEventKind::Press
                && let Some(press) = action_renderer.on_image_press()
            {
                press(
                    w,
                    &BlockSlot::Image {
                        id: image.id,
                        placement: image.placement,
                        url: image.url,
                        alt: image.alt,
                        title: image.title,
                        occurrence_index: image.occurrence_index,
                    },
                );
            }
            return true;
        }
        false
    });
    let node = if renderer.on_context_menu().is_some() {
        node.on_context_menu(move |w, event| {
            let Some(callback) = renderer.on_context_menu() else { return false };
            let internal = event.items.iter().find_map(|item| match item {
                ContextMenuItem::CopyLink(url) => url.strip_prefix(identity.as_ref()).map(|suffix| (url, suffix)),
                _ => None,
            });
            let Some((target, action)) = internal else { return callback(w, event) };
            let mut menu = event.clone();
            menu.items.retain(|item| !matches!(item, ContextMenuItem::CopyLink(url) if url == target));
            if let Some(index) = action_index(action, "image/")
                && let Some(url) = w.with(output, |out| out.image(index).map(|image| image.url.to_string()))
            {
                let format = SmartCopy::ImageUrl.into();
                if !menu
                    .items
                    .iter()
                    .any(|item| matches!(item, ContextMenuItem::Copy { format: item_format, .. } if *item_format == format))
                {
                    menu.items.push(ContextMenuItem::Copy { format, text: url });
                }
            }
            callback(w, &menu)
        })
    } else {
        node
    };
    block.with_reveal(node, move |r| (r.with(output, |out| out.rich.as_str().len()), false))
}

impl InlineOutput {
    fn image(&self, index: usize) -> Option<&ImageOccurrence> {
        self.images.iter().find(|image| image.occurrence_index == index)
    }
}

fn action_index(suffix: &str, prefix: &str) -> Option<usize> {
    suffix.strip_prefix(prefix)?.parse().ok()
}

pub(super) fn rich_of(read: &mut ReadCx<'_>, block: &RenderBlock, inline: &Inline) -> Rich {
    let locale = MarkdownLocale::default();
    let recognized = read.with(block.context.overlay, |overlay| overlay.links(inline));
    let theme = block.theme(read);
    let renderer = block.renderer();
    let plan = InlinePlan::new(inline.clone(), &recognized, Vec::new(), &[], block, &theme, locale);
    plan.draw(locale, "", |url| renderer.link_pill_content(read, url))
}

#[derive(Clone, PartialEq)]
struct InlinePlan {
    text: String,
    pieces: Vec<InlinePiece>,
    links: Vec<(Range<usize>, Arc<str>)>,
    spoilers: Vec<usize>,
    images: Vec<ImageOccurrence>,
}

#[derive(Clone, PartialEq)]
enum InlinePiece {
    #[cfg(feature = "media")]
    DecorativeImage {
        at: usize,
        url: String,
    },
    Spoiler {
        start: usize,
        span: Span,
    },
    Text {
        range: Range<usize>,
        span: Span,
        link: Option<usize>,
        image: Option<InlineImage>,
        #[cfg(feature = "math")]
        math: Option<MathPiece>,
    },
}

#[cfg(feature = "math")]
#[derive(Clone, PartialEq)]
struct MathPiece {
    source: Arc<str>,
    layout: Option<crate::math::MathLayout>,
}

#[derive(Clone, PartialEq)]
struct InlineImage {
    bytes: usize,
    press: Option<usize>,
    #[cfg(feature = "media")]
    url: String,
}

impl InlinePlan {
    /// Applies the theme, static link matches and authored inline styles independently of
    /// per-URL metadata so a label or icon update only reassembles the rich text.
    fn new(
        inline: Inline,
        recognized: &[RecognizedLink],
        images: Vec<ImageOccurrence>,
        revealed: &[usize],
        block: &RenderBlock,
        theme: &MarkdownTheme,
        locale: MarkdownLocale,
    ) -> Self {
        let renderer = block.renderer();
        let mut edges = vec![0, inline.text.len()];
        let styles = inline.styles.iter().filter(|(range, _)| !range.is_empty() && inline.text.get(range.clone()).is_some());
        let mut concealed = Vec::new();
        let mut links = Vec::new();
        for (range, style) in styles.clone() {
            edges.extend([range.start, range.end]);
            match style {
                Style::Spoiler if !revealed.contains(&range.start) => concealed.push(range),
                Style::Link(url) => links.push((range.clone(), Arc::<str>::from(url.as_str()))),
                _ => {}
            }
        }
        let inline_images = inline_images(&inline);
        edges.extend(inline_images.iter().map(|(range, _)| range.start));
        for recognized in recognized {
            if inline.text.get(recognized.range.clone()).is_some() {
                edges.extend([recognized.range.start, recognized.range.end]);
                links.push((recognized.range.clone(), recognized.url.clone()));
            }
        }
        links.retain(|(range, _)| !concealed.iter().any(|concealed| ranges_overlap(concealed, range)));
        links.sort_by_key(|(range, _)| range.start);
        let variants = links
            .iter()
            .map(|(_, url)| renderer.link_variant_rules().and_then(|rules| rules.find(url)).map(|rule| rule.style))
            .collect::<Vec<_>>();

        edges.sort_unstable();
        edges.dedup();
        let mut pieces = Vec::new();
        for (index, &start) in edges.iter().enumerate() {
            if let Some(range) = concealed.iter().find(|range| range.contains(&start)) {
                if start == range.start {
                    let mut span = Span::default();
                    renderer.style_inline(theme, &Style::Spoiler, locale.reveal_spoiler, &mut span);
                    pieces.push(InlinePiece::Spoiler { start, span });
                }
                continue;
            }
            #[cfg(feature = "media")]
            for (_, url) in inline_images.iter().filter_map(|(range, style)| match style {
                Style::Image { url, .. } if range.start == start && range.is_empty() => Some((range, url)),
                _ => None,
            }) {
                pieces.push(InlinePiece::DecorativeImage { at: start, url: url.clone() });
            }
            let Some(&end) = edges.get(index + 1) else { break };
            let mut span = Span::default();
            for (range, style) in styles.clone().filter(|(range, style)| range.contains(&start) && !matches!(style, Style::Link(_))) {
                #[cfg(feature = "math")]
                let style = if style.is_math() { &Style::Math } else { style };
                renderer.style_inline(theme, style, &inline.text[range.clone()], &mut span);
            }

            let link = links.iter().position(|(range, _)| range.contains(&start));
            if let Some(index) = link {
                let (range, url) = &links[index];
                renderer.style_inline(theme, &Style::Link(url.to_string()), &inline.text[range.clone()], &mut span);
                span = variants[index].unwrap_or(span);
            }
            let image = inline_images.iter().find_map(|(range, style)| match style {
                Style::Image { url: _url, .. }
                    if range.start == start && !range.is_empty() && concealed.iter().all(|spoiler| !ranges_overlap(spoiler, range)) =>
                {
                    let press = if links.iter().any(|(link, _)| ranges_overlap(link, range)) {
                        None
                    } else {
                        images
                            .iter()
                            .find(|image| image.inline.as_ref().is_some_and(|(_, source)| source == *range))
                            .map(|image| image.occurrence_index)
                    };
                    Some(InlineImage {
                        bytes: range.len(),
                        press,
                        #[cfg(feature = "media")]
                        url: _url.clone(),
                    })
                }
                _ => None,
            });
            #[cfg(feature = "math")]
            let math = styles.clone().find_map(|(range, style)| {
                if range != &(start..end) || !style.is_math() || inline_images.iter().any(|(image, _)| ranges_overlap(image, range)) {
                    return None;
                }
                let tex = &inline.text[range.clone()];
                let (source, display, complete) = match style {
                    Style::MathSource { source, display, is_complete } => (source.clone(), *display, *is_complete),
                    _ => (Arc::from(format!("${tex}$")), false, true),
                };
                let layout = block.context.math.render(renderer, tex, crate::math::render_style(theme, Some(span), display), complete);
                if layout.is_none() {
                    span.mono = true;
                }
                Some(MathPiece { source, layout })
            });
            pieces.push(InlinePiece::Text {
                range: start..end,
                span,
                link,
                image,
                #[cfg(feature = "math")]
                math,
            });
        }
        Self { text: inline.text, pieces, links, spoilers: concealed.into_iter().map(|range| range.start).collect(), images }
    }

    fn draw(&self, locale: MarkdownLocale, spoiler_prefix: &str, mut content: impl FnMut(&str) -> Option<LinkPillContent>) -> Rich {
        let metadata = self.links.iter().map(|(_, url)| content(url)).collect::<Vec<_>>();
        let mut pills = vec![None; self.links.len()];
        let mut link_targets = vec![0..0; self.links.len()];
        let mut targets = Vec::new();
        let mut out = rich();
        for piece in &self.pieces {
            let target = out.as_str().len();
            let (range, span, link) = match piece {
                InlinePiece::Text { range, span, link, .. } => (range, span, link),
                #[cfg(feature = "media")]
                InlinePiece::DecorativeImage { at, url } => {
                    let replaced =
                        self.links.iter().zip(&pills).any(|((range, _), pill)| pill.is_some() && range.start < *at && range.contains(at));
                    if !replaced && let Some(attachment) = crate::media::inline_image(target..target, url) {
                        out = out.attachment(attachment);
                    }
                    continue;
                }
                InlinePiece::Spoiler { start, span } => {
                    out = out.span(locale.reveal_spoiler, *span);
                    if !spoiler_prefix.is_empty() {
                        targets.push((target..out.as_str().len(), Arc::from(format!("{spoiler_prefix}spoiler/{start}"))));
                    }
                    continue;
                }
            };
            let text = &self.text[range.clone()];
            #[cfg(feature = "math")]
            let math = match piece {
                InlinePiece::Text { math, .. } => math.as_ref(),
                _ => None,
            };
            #[cfg(feature = "math")]
            let text = math.map_or(text, |math| math.source.as_ref());
            let mut style = *span;
            if let Some(link) = *link {
                let (source, url) = &self.links[link];
                let metadata = metadata[link].as_ref();
                let label = metadata.and_then(|content| content.label.as_ref()).filter(|label| !label.is_empty());
                let first = range.start == source.start;
                style.color = metadata.and_then(|content| content.tint).or(style.color);
                if first {
                    link_targets[link].start = target;
                    if let Some(content) = metadata
                        && (label.is_some() || content.icon.is_some())
                    {
                        pills[link] = Some(InlinePill {
                            icon: content.icon.as_ref().map(|icon| NodeIcon::new(icon, None)),
                            label: label.cloned().unwrap_or_else(|| Arc::from(&self.text[source.clone()])),
                            style: pill_style(style),
                        });
                    }
                }
                if pills[link].is_some() {
                    let logical = match label {
                        Some(_) if !first => continue,
                        Some(_) if !url.is_empty() => url,
                        Some(_) => &self.text[source.clone()],
                        None => text,
                    };
                    link_targets[link].end = target + logical.len();
                    out = out.text(logical);
                    continue;
                }
                link_targets[link].end = target + text.len();
            }
            out = if link.is_none() && style == Span::default() { out.text(text) } else { out.span(text, style) };
            #[cfg(feature = "math")]
            if let Some(layout) = math.and_then(|math| math.layout.clone()) {
                let end = out.as_str().len();
                out = out.attachment(layout.attachment(target..end));
            }

            if let InlinePiece::Text { image: Some(image), .. } = piece {
                let range = target..target + image.bytes;
                if let Some(index) = image.press {
                    targets.push((range.clone(), Arc::from(format!("{spoiler_prefix}image/{index}"))));
                }
                #[cfg(feature = "media")]
                if let Some(attachment) = crate::media::inline_image(range, &image.url) {
                    out = out.attachment(attachment);
                }
            }
        }
        for (((_, url), range), pill) in self.links.iter().zip(link_targets).zip(pills) {
            if !range.is_empty() {
                if let Some(pill) = pill {
                    out = out.attachment(pill.attachment(range.clone()));
                }
                targets.push((range, url.clone()));
            }
        }
        targets.sort_by_key(|(range, _)| range.start);
        for (range, url) in targets {
            out = out.link_range(range, url);
        }
        out
    }
}

fn ranges_overlap(a: &Range<usize>, b: &Range<usize>) -> bool {
    a.start < b.end && b.start < a.end
}

fn pill_style(span: Span) -> PillStyle {
    let default = PillStyle::default();
    PillStyle {
        size: span.size.unwrap_or(default.size),
        weight: span.weight.unwrap_or(default.weight),
        color: span.color,
        background: span.background,
        border_color: span.border_color,
        radius: span.radius.unwrap_or(default.radius),
        padding_x: span.padding_x.unwrap_or(default.padding_x),
        padding_y: span.padding_y.unwrap_or(default.padding_y),
        ..default
    }
}
