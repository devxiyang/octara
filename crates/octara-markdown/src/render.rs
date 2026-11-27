//! Blocks into elements: each block its own element under a stable key, so a block that did
//! not change keeps its node, and the block that grows updates its text only.

mod code;
mod inline;
mod table;

pub use code::default_code_block;
pub use table::default_table;

use crate::{
    AstOverlay, CalloutKind, MarkdownLocale, MarkdownRenderer, MarkdownTheme,
    ast::{Block, Inline, Kind, Style},
    slots::{BlockSlot, BlockSlotRenderer, SlotContent, SlotRecord},
    store::standalone_link,
    stream::StreamingEffects,
};
use inline::inline_view;
use octara::prelude::*;
use octara_icons as icons;
use std::fmt;
use std::mem::{Discriminant, discriminant};
use std::ops::Deref;
use std::sync::Arc;

/// Position is the display label; structure also participates in row identity so a new kind
/// gets a new element. Content changes keep the same row.
#[derive(Clone, PartialEq, Eq, Hash)]
pub(super) struct BlockKey {
    index: usize,
    kind: Discriminant<Kind>,
    detail: u8,
}

impl fmt::Display for BlockKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "b{}", self.index)
    }
}

/// Reference identity lets a row's memo cut off an unchanged prefix without comparing or
/// cloning its blocks again when a snapshot changes elsewhere.
#[derive(Clone)]
pub(super) struct BlockRef(Arc<Block>);

impl PartialEq for BlockRef {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl Deref for BlockRef {
    type Target = Block;

    fn deref(&self) -> &Block {
        &self.0
    }
}

#[derive(Default)]
pub(super) struct BlockCache(Vec<BlockRef>);

impl BlockCache {
    pub(super) fn update(&mut self, blocks: &[Arc<Block>], kept: usize) -> Vec<(BlockKey, BlockRef)> {
        for (i, block) in blocks.iter().enumerate().skip(kept.min(self.0.len())) {
            if self.0.get(i).is_some_and(|old| Arc::ptr_eq(&old.0, block) || old.0 == *block) {
                continue;
            }
            let block = BlockRef(block.clone());
            match self.0.get_mut(i) {
                Some(old) => *old = block,
                None => self.0.push(block),
            }
        }
        self.0.truncate(blocks.len());
        self.0
            .iter()
            .enumerate()
            .map(|(index, block)| {
                let detail = match &block.kind {
                    Kind::Heading(level, _) => *level,
                    Kind::Paragraph(inline) => u8::from(standalone_link(inline).is_some()),
                    Kind::List { start, .. } => u8::from(start.is_some()),
                    _ => 0,
                };
                (BlockKey { index, kind: discriminant(&block.kind), detail }, block.clone())
            })
            .collect()
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct PositionKey {
    prefix: char,
    index: usize,
}

impl fmt::Display for PositionKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}{}", self.prefix, self.index)
    }
}

fn keyed_rows<T>(prefix: char, items: impl IntoIterator<Item = T>) -> Vec<(PositionKey, T)> {
    items.into_iter().enumerate().map(|(index, item)| (PositionKey { prefix, index }, item)).collect()
}

#[derive(Clone)]
pub(super) struct RenderContext {
    renderer: Arc<dyn MarkdownRenderer>,
    theme: ReadSignal<MarkdownTheme>,
    overlay: ReadSignal<Arc<AstOverlay>>,
    effects: StreamingEffects,
    finished: ReadSignal<bool>,
    #[cfg(feature = "math")]
    math: crate::math::MathDocument,
}

impl RenderContext {
    pub(super) fn new(
        renderer: Arc<dyn MarkdownRenderer>,
        theme: ReadSignal<MarkdownTheme>,
        overlay: ReadSignal<Arc<AstOverlay>>,
        effects: StreamingEffects,
        finished: ReadSignal<bool>,
        #[cfg(feature = "math")] math: crate::math::MathDocument,
    ) -> Self {
        Self {
            renderer,
            theme,
            overlay,
            effects,
            finished,
            #[cfg(feature = "math")]
            math,
        }
    }
}

/// A mounted block's reactive data and inherited rendering context.
#[derive(Clone)]
pub struct RenderBlock {
    source: ReadSignal<Option<BlockRef>>,
    context: RenderContext,
    slot: ReadSignal<Option<SlotRecord>>,
    finished: Option<ReadSignal<bool>>,
    #[cfg(feature = "math")]
    math_source: Option<ReadSignal<Option<Arc<str>>>>,
}

impl RenderBlock {
    /// Reads the current AST. Unchanged blocks share their immutable allocation.
    pub fn read(&self, read: &mut ReadCx<'_>) -> Option<Arc<Block>> {
        read.with(self.source, |block| block.as_ref().map(|block| block.0.clone()))
    }

    pub fn theme(&self, read: &mut ReadCx<'_>) -> MarkdownTheme {
        read.get(self.context.theme)
    }

    pub fn renderer(&self) -> &dyn MarkdownRenderer {
        self.context.renderer.as_ref()
    }

    /// Renders an inline value with this document's theme and renderer.
    pub fn rich(&self, read: &mut ReadCx<'_>, inline: &Inline) -> Rich {
        inline::rich_of(read, self, inline)
    }

    fn project<T: Default>(&self, read: &mut ReadCx<'_>, project: impl FnOnce(&Block) -> T) -> T {
        read.with(self.source, |block| block.as_deref().map(project).unwrap_or_default())
    }

    fn reveal(&self, read: &mut ReadCx<'_>, bytes: usize, settled: bool) -> Option<TextReveal> {
        let finished = settled || self.finished.is_none_or(|finished| read.get(finished));
        self.context.effects.reveal(bytes, finished)
    }
}

fn blocks_view(build: &mut BuildCx<'_>, context: RenderContext, get: impl Fn(&mut ReadCx<'_>) -> Vec<Block> + 'static) -> El {
    let mut cache = BlockCache::default();
    let blocks = build.derive_list(move |r| {
        let blocks = get(r).into_iter().map(Arc::new).collect::<Vec<_>>();
        cache.update(&blocks, 0)
    });
    col().child(each(blocks, move |b, _, block| block_view(b, block, context.clone())))
}

pub(super) fn block_view(build: &mut BuildCx<'_>, block: RowSignal<BlockRef>, context: RenderContext) -> El {
    let source = build.memo(move |r| r.try_get(block));
    let overlay = context.overlay;
    let slot =
        build.memo(move |r| r.with2(source, overlay, |block, overlay| block.as_deref().and_then(|block| overlay.slot(block)).cloned()));
    let document_finished = context.finished;
    let finished = (context.effects != StreamingEffects::None).then(|| {
        build.memo(move |r| {
            r.get(document_finished)
                || r.with2(source, overlay, |block, overlay| block.as_deref().is_none_or(|block| overlay.is_finished(block)))
        })
    });
    #[cfg(feature = "math")]
    let math_source = build
        .sample(source)
        .filter(|block| matches!(block.kind, Kind::Math { .. }))
        .map(|_| build.memo(move |r| r.with2(source, overlay, |block, overlay| overlay.math_source(block.as_deref()?).cloned())));
    let block = RenderBlock {
        source,
        context,
        slot,
        finished,
        #[cfg(feature = "math")]
        math_source,
    };
    block.renderer().render_block(build, &block)
}

/// Dispatches by AST kind, including for children nested in quotes and lists.
pub fn default_block(build: &mut BuildCx<'_>, block: &RenderBlock) -> El {
    let ast = build.sample(block.source).expect("mounted Markdown block");
    let fallback_kind = ast.clone();
    let fallback = move |build: &mut BuildCx<'_>, block: &RenderBlock| {
        let renderer = block.renderer();
        match fallback_kind.kind {
            Kind::Paragraph(_) => renderer.render_paragraph(build, block),
            Kind::Heading(level, _) => renderer.render_heading(build, block, level),
            Kind::Code { .. } => renderer.render_code_block(build, block),
            Kind::Quote { .. } => renderer.render_quote(build, block),
            Kind::List { .. } => renderer.render_list(build, block),
            Kind::Rule => renderer.render_rule(build, block),
            Kind::Table { .. } => renderer.render_table(build, block),
            Kind::Image { .. } => renderer.render_image(build, block),
            Kind::Math { .. } => renderer.render_math(build, block),
            Kind::Directive { .. } => renderer.render_directive(build, block),
            Kind::Video { .. } => renderer.render_video(build, block),
            Kind::Spoiler(_) => renderer.render_spoiler(build, block),
        }
    };
    let node = if build.sample(block.slot).is_some()
        && let Some(renderer) = build.context::<Arc<dyn BlockSlotRenderer>>()
    {
        let signal = block.slot;
        let block = block.clone();
        col().min_w(0).child(keyed_or_else(
            move |r| SlotKey(r.get(signal)),
            move |build, key| {
                let slot = key.0.as_ref()?.mount(build, signal);
                renderer.render_slot(build, &slot)
            },
            move |build| fallback(build, &block),
        ))
    } else {
        fallback(build, block)
    };
    let node = if matches!(ast.kind, Kind::Code { .. }) { node.role(Role::Code) } else { node };
    block_actions(build, block, &ast.kind, node)
}

// Only the header identifies active slots. Static slots include their authored data so
// a changed gallery occurrence or placement reaches a mounted replacement.
struct SlotKey(Option<SlotRecord>);

impl PartialEq for SlotKey {
    fn eq(&self, other: &Self) -> bool {
        match (&self.0, &other.0) {
            (Some(a), Some(b)) if a.id == b.id && a.placement == b.placement => {
                matches!(a.content, SlotContent::Code { .. } | SlotContent::Math { .. } | SlotContent::Directive { .. }) || a == b
            }
            (None, None) => true,
            _ => false,
        }
    }
}

impl fmt::Display for SlotKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.0 {
            Some(slot) => slot.id.fmt(f),
            None => f.write_str("default"),
        }
    }
}

fn block_actions(build: &mut BuildCx<'_>, block: &RenderBlock, kind: &Kind, node: El) -> El {
    let renderer = block.context.renderer.clone();
    match kind {
        Kind::Image { .. } => {
            let slot = block.slot;
            let node = if renderer.on_image_press().is_some() {
                let renderer = renderer.clone();
                focus_outline(node).focusable().track_interaction().on_press(move |w| {
                    let record = w.read(|r| r.get(slot));
                    if let Some(SlotRecord { id, placement, content: SlotContent::Image { url, alt, title, occurrence_index } }) = record
                        && let Some(press) = renderer.on_image_press()
                    {
                        press(w, &BlockSlot::Image { id, placement, url, alt, title, occurrence_index });
                    }
                })
            } else {
                node
            };
            if renderer.on_context_menu().is_some() {
                node.on_context_menu(move |w, event| {
                    let url = w.read(|r| {
                        r.with(slot, |record| match record.as_ref().map(|record| &record.content) {
                            Some(SlotContent::Image { url, .. }) => Some(url.clone()),
                            _ => None,
                        })
                    });
                    let Some(handler) = renderer.on_context_menu() else { return false };
                    let Some(url) = url else { return handler(w, event) };
                    let format = crate::SmartCopy::ImageUrl.into();
                    if event
                        .items
                        .iter()
                        .any(|item| matches!(item, ContextMenuItem::Copy { format: item_format, .. } if *item_format == format))
                    {
                        return handler(w, event);
                    }
                    let mut menu = event.clone();
                    menu.items.push(ContextMenuItem::Copy { format, text: url.to_string() });
                    handler(w, &menu)
                })
            } else {
                node
            }
        }
        Kind::Code { .. } if renderer.on_code_block_press().is_some() => {
            let action = code::CodeAction::new(build, block);
            focus_outline(node).focusable().track_interaction().on_press(move |w| {
                if let Some(slot) = action.slot(w)
                    && let Some(press) = renderer.on_code_block_press()
                {
                    press(w, &slot);
                }
            })
        }
        _ => node,
    }
}

pub(super) fn focus_outline<C>(node: El<C>) -> El<C> {
    node.when(State::FOCUS_VISIBLE, |p| p.ring(2).ring_color(ColorToken::Focus))
}

fn inline_text(build: &mut BuildCx<'_>, block: &RenderBlock) -> El {
    let source = block.clone();
    inline_view(
        build,
        block,
        |_| 0,
        move |r| {
            source.project(r, |block| match &block.kind {
                Kind::Paragraph(inline) | Kind::Heading(_, inline) => inline.clone(),
                Kind::Spoiler(inline) => {
                    let mut inline = inline.clone();
                    inline.styles.retain(|(range, style)| *style != Style::Spoiler || *range != (0..inline.text.len()));
                    inline
                }
                _ => Inline::default(),
            })
        },
    )
}

pub fn default_paragraph(build: &mut BuildCx<'_>, block: &RenderBlock) -> El {
    inline_text(build, block).role(Role::Paragraph)
}

pub fn default_heading(build: &mut BuildCx<'_>, block: &RenderBlock, level: u8) -> El {
    let theme = block.context.theme;
    inline_text(build, block).role(Role::Heading).level(level).style_with(move |r, p| {
        let h = r.with(theme, |theme| theme.headings[usize::from(level.clamp(1, 6) - 1)]);
        let p = p.text_size(h.size).text_weight(h.weight).line_height(h.line_height).mt(h.top_spacing).mb(h.bottom_spacing);
        let p = if let Some(color) = h.color { p.text_color(color) } else { p };
        if let Some(color) = h.border_bottom { p.border_b(1).border_color(color).pb(sp(1.0)) } else { p }
    })
}

pub fn default_style_inline(theme: &MarkdownTheme, style: &Style, _label: &str, span: &mut Span) {
    match style {
        Style::Strong => {
            span.weight = Some(theme.strong_weight);
            span.color = theme.strong_color.or(span.color);
        }
        Style::Emphasis => {
            span.italic = true;
            span.color = theme.emphasis_color.or(span.color);
        }
        Style::Strike => {
            span.strike = true;
            span.color = theme.strike_color.or(span.color);
        }
        Style::Code => {
            let code = theme.inline_code;
            span.mono = true;
            span.background = code.background;
            span.color = code.text_color.or(span.color);
            span.border_color = code.border_color;
            span.radius = Some(code.radius);
            span.padding_x = Some(code.padding_x);
            span.padding_y = Some(code.padding_y);
            span.size = code.font_size;
        }
        Style::Link(_) => {
            span.color = theme.link.color.or(span.color);
            span.underline = theme.link.underline;
        }
        Style::Math => {
            span.mono = true;
            span.color = theme.math.color.or(span.color);
        }
        #[cfg(feature = "math")]
        Style::MathSource { .. } => default_style_inline(theme, &Style::Math, "", span),
        Style::Spoiler => {
            span.background = Some(ColorToken::SurfacePressed.into());
            span.radius = Some(4.0);
        }
        Style::FootnoteRef | Style::Image { .. } => {
            span.color = span.color.or(Some(ColorToken::TextMuted.into()));
        }
    }
}

pub fn default_quote(build: &mut BuildCx<'_>, block: &RenderBlock) -> El {
    let content = block.clone();
    let callout = block.clone();
    let appearance = block.clone();
    let theme = block.context.theme;
    let locale = MarkdownLocale::get(build);
    row()
        .role(Role::Blockquote)
        .style_with(move |r, p| {
            let kind = callout_kind(&appearance, r);
            r.with(theme, |theme| {
                let quote = &theme.quote;
                match kind {
                    Some(kind) => {
                        let tone = quote.callouts.get(kind).tone;
                        p.border_l(quote.bar_width)
                            .border_color(tone.alpha(0.45))
                            .bg(tone.alpha(0.10))
                            .rounded(RadiusToken::Sm)
                            .p(theme.block_gap)
                    }
                    None => p.border_l(quote.bar_width).border_color(quote.bar_color).pl(theme.block_gap).text_color(quote.text_color),
                }
            })
        })
        .child(
            col()
                .grow(1.0)
                .min_w(0)
                .style_with(move |r, p| p.gap(r.with(theme, |theme| theme.block_gap)))
                .child(keyed(
                    move |r| QuoteHeader(callout_kind(&callout, r).map(|kind| (kind, r.with(theme, |t| t.quote.callouts.get(kind).icon)))),
                    move |_, key| {
                        let (kind, glyph) = key.0?;
                        let label = match kind {
                            CalloutKind::Note => locale.note,
                            CalloutKind::Tip => locale.tip,
                            CalloutKind::Important => locale.important,
                            CalloutKind::Warning => locale.warning,
                            CalloutKind::Caution => locale.caution,
                        };
                        Some(
                            row()
                                .align(AlignItems::Center)
                                .gap(sp(1.5))
                                .text_weight(650)
                                .select_none()
                                .style_with(move |r, p| p.text_color(r.with(theme, |t| t.quote.callouts.get(kind).tone)))
                                .child(icon(glyph.into()).size(sp(4.0)).a11y_hidden())
                                .child(text(label)),
                        )
                    },
                ))
                .child(
                    blocks_view(build, block.context.clone(), move |r| {
                        content.project(r, |block| match &block.kind {
                            Kind::Quote { blocks, .. } => blocks.clone(),
                            _ => Vec::new(),
                        })
                    })
                    .style_with(move |r, p| p.gap(r.with(theme, |theme| theme.block_gap))),
                ),
        )
}

fn callout_kind(block: &RenderBlock, read: &mut ReadCx<'_>) -> Option<CalloutKind> {
    block.project(read, |block| match block.kind {
        Kind::Quote { callout, .. } => callout,
        _ => None,
    })
}

#[derive(PartialEq)]
struct QuoteHeader(Option<(CalloutKind, icons::LucideIcon)>);

impl fmt::Display for QuoteHeader {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some((kind, glyph)) = self.0 { write!(f, "{kind:?}:{}", glyph.name()) } else { f.write_str("quote") }
    }
}

pub fn default_rule(block: &RenderBlock) -> El {
    let theme = block.context.theme;
    el().w(rel(1.0)).a11y_hidden().style_with(move |r, p| {
        let rule = r.with(theme, |theme| theme.rule);
        p.h(rule.height).bg(rule.color)
    })
}

/// An asynchronously resolved image with media enabled, otherwise selectable alt text.
pub fn default_image(_build: &mut BuildCx<'_>, block: &RenderBlock) -> El {
    let source = block.clone();
    let alt_text = move |r: &mut ReadCx<'_>| {
        source.project(r, |block| match &block.kind {
            Kind::Image { alt, .. } => alt.clone(),
            _ => String::new(),
        })
    };
    #[cfg(feature = "media")]
    {
        let source = block.clone();
        col().min_w(0).role(Role::Image).label(alt_text).child(keyed(
            move |r| {
                source.project(r, |block| match &block.kind {
                    Kind::Image { url, alt, .. } => ImageKey(url.clone(), alt.clone()),
                    _ => ImageKey::default(),
                })
            },
            |_, key| crate::media::block_image(&key.0, &key.1),
        ))
    }
    #[cfg(not(feature = "media"))]
    {
        text(alt_text.clone()).label(alt_text).role(Role::Image).text_color(ColorToken::TextMuted)
    }
}

#[cfg(feature = "media")]
#[derive(Default, PartialEq)]
struct ImageKey(String, String);

#[cfg(feature = "media")]
impl fmt::Display for ImageKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("image")
    }
}

/// The video source as a selectable link without the media feature.
pub fn default_video(_build: &mut BuildCx<'_>, block: &RenderBlock) -> El {
    let source = block.clone();
    #[cfg(feature = "media")]
    {
        col().min_w(0).child(keyed(
            move |r| {
                source.project(r, |block| match &block.kind {
                    Kind::Video { src, poster } => VideoKey(src.clone(), poster.clone()),
                    _ => VideoKey::default(),
                })
            },
            |_, key| crate::media::block_video(&key.0, key.1.as_deref()),
        ))
    }
    #[cfg(not(feature = "media"))]
    {
        text(move |r: &mut ReadCx<'_>| {
            source.project(r, |block| match &block.kind {
                Kind::Video { src, .. } => rich().link(src, src.as_str()),
                _ => Rich::default(),
            })
        })
    }
}

#[cfg(feature = "media")]
#[derive(Default, PartialEq)]
struct VideoKey(String, Option<String>);

#[cfg(feature = "media")]
impl fmt::Display for VideoKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("video")
    }
}

/// Selectable formula source, rendered as vector math when the `math` feature is enabled.
pub fn default_math(block: &RenderBlock) -> El {
    #[cfg(feature = "math")]
    {
        let theme = block.context.theme;
        let block = block.clone();
        let formula = text(move |r: &mut ReadCx<'_>| {
            let value = block.read(r);
            let Some(value) = value.as_deref() else { return rich() };
            let Kind::Math { tex, is_complete } = &value.kind else { return rich() };
            let theme = block.theme(r);
            let math = &block.context.math;
            let source = block
                .math_source
                .and_then(|source| r.get(source))
                .unwrap_or_else(|| if *is_complete { format!("$${tex}$$").into() } else { format!("$${tex}").into() });
            let layout = math.render(block.renderer(), tex, crate::math::render_style(&theme, None, true), *is_complete);
            let range = 0..source.len();
            let out = rich().text(source);
            if let Some(layout) = layout { out.attachment(layout.attachment(range)) } else { out }
        })
        .text_family(FontFamilyToken::Mono)
        .text_align(TextAlign::Center)
        .text_wrap(TextWrap::None)
        .shrink(0.0);
        row()
            .min_w(0)
            .align(AlignItems::Center)
            .overflow_x(Overflow::Auto)
            .style_with(move |r, p| {
                let math = r.with(theme, |theme| theme.math);
                p.p(math.padding).bg(math.background).rounded(math.radius).text_color(math.color.unwrap_or(r.with(theme, |t| t.text_color)))
            })
            .child(spacer())
            .child(formula)
            .child(spacer())
    }
    #[cfg(not(feature = "math"))]
    {
        let theme = block.context.theme;
        plain_text(block, |kind| match kind {
            Kind::Math { tex, .. } => tex,
            _ => "",
        })
        .text_family(FontFamilyToken::Mono)
        .style_with(move |r, p| {
            let m = r.with(theme, |theme| theme.math);
            let p = p.p(m.padding).bg(m.background).rounded(m.radius);
            if let Some(color) = m.color { p.text_color(color) } else { p }
        })
    }
}

/// A directive without a slot replacement displays its body as selectable text.
pub fn default_directive(block: &RenderBlock) -> El {
    plain_text(block, |kind| match kind {
        Kind::Directive { body, .. } => body,
        _ => "",
    })
}

fn plain_text(block: &RenderBlock, value: fn(&Kind) -> &str) -> El {
    let source = block.clone();
    let node = text(move |r: &mut ReadCx<'_>| source.project(r, |block| value(&block.kind).to_owned()));
    if block.context.effects == StreamingEffects::None {
        return node;
    }
    let block = block.clone();
    node.text_reveal(move |r: &mut ReadCx<'_>| {
        let bytes = block.project(r, |block| value(&block.kind).len());
        block.reveal(r, bytes, false)
    })
}

/// A keyboard- and pointer-activated control that reveals the block body.
pub fn default_spoiler(build: &mut BuildCx<'_>, block: &RenderBlock) -> El {
    let caption = MarkdownLocale::get(build).reveal_spoiler;
    let visible = build.signal(false);
    let body = block.clone();
    col()
        .min_w(0)
        .child(show(
            move |r| !r.get(visible),
            move |_| {
                focus_outline(octara_headless::interactive::button(text(caption)))
                    .align_self(AlignItems::Start)
                    .rounded(RadiusToken::Sm)
                    .px(sp(2.0))
                    .py(sp(1.0))
                    .bg(ColorToken::SurfacePressed)
                    .select_none()
                    .on_press(move |w| w.set(visible, true))
            },
        ))
        .child(show(move |r| r.get(visible), move |build| inline_text(build, &body)))
}

pub fn default_list(build: &mut BuildCx<'_>, block: &RenderBlock) -> El {
    let content = block.clone();
    let spacing = block.clone();
    let start = block.clone();
    let context = block.context.clone();
    let theme = context.theme;
    let items = build.derive_list(move |r| {
        content.project(r, |block| match &block.kind {
            Kind::List { items, .. } => keyed_rows('i', items.iter().cloned()),
            _ => Vec::new(),
        })
    });
    let start = build.memo(move |r| {
        start.project(r, |block| match block.kind {
            Kind::List { start, .. } => start,
            _ => None,
        })
    });
    let gap = build.memo(move |r| {
        let tight = spacing.project(r, |block| matches!(block.kind, Kind::List { tight: true, .. }));
        r.with(theme, |t| if tight { t.tight_list_gap } else { t.loose_list_gap })
    });
    col().role(Role::List).style_with(move |r, p| p.gap(r.get(gap))).child(each(items, move |b, key, item| {
        let item = b.memo(move |r| r.try_get(item));
        let index = key.index as u64;
        let bullet = move |r: &mut ReadCx<'_>| {
            r.with2(start, theme, |start, theme| match start {
                Some(n) => format!("{}{}", n.saturating_add(index), theme.list.ordered_suffix),
                None => theme.list.bullet.to_owned(),
            })
        };
        let is_task = move |r: &mut ReadCx<'_>| r.with(item, |item| item.as_ref().is_some_and(|item| item.task.is_some()));
        let done = move |r: &mut ReadCx<'_>| r.with(item, |item| item.as_ref().is_some_and(|item| item.task == Some(true)));
        let renderer = context.renderer.clone();
        row()
            .role(Role::ListItem)
            .align(AlignItems::Start)
            .style_with(move |r, p| p.gap(r.with(theme, |t| t.list.marker_gap)))
            .child(show(
                move |r| r.get(start).is_some() || !is_task(r),
                move |_| {
                    text(bullet).select_none().a11y_hidden().tabular_nums(true).shrink(0.0).style_with(move |r, p| {
                        p.min_w(if r.get(start).is_some() { sp(5.0) } else { sp(3.0) }).text_color(r.with(theme, |t| t.list.marker_color))
                    })
                },
            ))
            .child(show(is_task, move |_| {
                let marker = text(move |r: &mut ReadCx<'_>| if done(r) { "☑" } else { "☐" })
                    .shrink(0.0)
                    .select_none()
                    .role(Role::CheckBox)
                    .checked(done)
                    .label(move |r: &mut ReadCx<'_>| {
                        r.with(item, |item| item.as_ref().map(|item| crate::block_text(item.blocks.iter())).unwrap_or_default())
                    });
                if renderer.on_task_toggle().is_some() {
                    let renderer = renderer.clone();
                    focus_outline(marker).focusable().on_toggle(move |w| {
                        let task = w.with(item, |item| item.as_ref().and_then(|i| i.task.zip(i.task_source.clone())));
                        if let Some((checked, source)) = task
                            && let Some(toggle) = renderer.on_task_toggle()
                        {
                            toggle(w, source, !checked);
                        }
                    })
                } else {
                    marker
                }
            }))
            .child(
                blocks_view(b, context.clone(), move |r| {
                    r.with(item, |item| item.as_ref().map(|item| item.blocks.clone()).unwrap_or_default())
                })
                .grow(1.0)
                .min_w(0)
                .style_with(move |r, p| p.gap(r.get(gap))),
            )
    }))
}
