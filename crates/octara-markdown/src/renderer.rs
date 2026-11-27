//! Renderer extension points and the fluent renderer for local customization.

use crate::{
    MarkdownTheme, ParseOptions, RenderBlock, Style,
    pills::{LinkPillContent, LinkVariantRules, TextRecognizer},
    render::*,
    slots::BlockSlot,
    stream::StreamingEffects,
};
use octara::prelude::*;
use std::ops::Range;
#[cfg(feature = "math")]
use {
    crate::math::{MathError, MathErrorHandler, MathRenderer},
    std::sync::Arc,
};

/// Return true to consume a link event. Unconsumed presses retain host navigation.
pub type LinkHandler = dyn Fn(&mut WriteCx<'_>, &LinkEvent) -> bool;

/// Return true to handle a context menu, or false to offer the default actions.
pub type ContextMenuHandler = dyn Fn(&mut WriteCx<'_>, &ContextMenuEvent) -> bool;

/// Handles an image press. The slot includes its occurrence index in the document.
pub type ImageHandler = dyn Fn(&mut WriteCx<'_>, &BlockSlot);

/// Receives the code slot when the block is pressed or its clipboard write succeeds.
pub type CodeHandler = dyn Fn(&mut WriteCx<'_>, &BlockSlot);

/// Receives the source byte range of a task marker and its desired next state.
pub type TaskToggle = dyn Fn(&mut WriteCx<'_>, Range<usize>, bool);

/// Shared rendering policy. The default parts keep their nodes as the source changes.
///
/// Provide an `Arc<dyn MarkdownRenderer>` in a build context to use it for every
/// descendant Markdown document. Theme reads can follow signals.
pub trait MarkdownRenderer: 'static {
    fn theme(&self, _read: &mut ReadCx<'_>) -> MarkdownTheme {
        MarkdownTheme::document()
    }

    fn parse_options(&self) -> ParseOptions {
        ParseOptions::default()
    }

    fn render_block(&self, build: &mut BuildCx<'_>, block: &RenderBlock) -> El {
        default_block(build, block)
    }

    fn render_paragraph(&self, build: &mut BuildCx<'_>, block: &RenderBlock) -> El {
        default_paragraph(build, block)
    }

    fn render_heading(&self, build: &mut BuildCx<'_>, block: &RenderBlock, level: u8) -> El {
        default_heading(build, block, level)
    }

    fn render_code_block(&self, build: &mut BuildCx<'_>, block: &RenderBlock) -> El {
        default_code_block(build, block)
    }

    fn render_quote(&self, build: &mut BuildCx<'_>, block: &RenderBlock) -> El {
        default_quote(build, block)
    }

    fn render_list(&self, build: &mut BuildCx<'_>, block: &RenderBlock) -> El {
        default_list(build, block)
    }

    fn render_table(&self, build: &mut BuildCx<'_>, block: &RenderBlock) -> El {
        default_table(build, block)
    }

    fn render_rule(&self, _build: &mut BuildCx<'_>, block: &RenderBlock) -> El {
        default_rule(block)
    }

    fn render_image(&self, build: &mut BuildCx<'_>, block: &RenderBlock) -> El {
        default_image(build, block)
    }

    fn render_math(&self, _build: &mut BuildCx<'_>, block: &RenderBlock) -> El {
        default_math(block)
    }

    /// An explicit formula renderer takes precedence over the scoped renderer.
    #[cfg(feature = "math")]
    fn math_renderer(&self) -> Option<&dyn MathRenderer> {
        None
    }

    #[cfg(feature = "math")]
    fn on_math_error(&self) -> Option<&MathErrorHandler> {
        None
    }

    /// Applies one inline style to a run. The label is the full text of this style's range.
    fn style_inline(&self, theme: &MarkdownTheme, style: &Style, label: &str, span: &mut Span) {
        default_style_inline(theme, style, label, span);
    }

    /// Without a callback, an enclosing view can handle link activations.
    fn on_link(&self) -> Option<&LinkHandler> {
        None
    }

    /// Without a callback, task markers are read-only.
    fn on_task_toggle(&self) -> Option<&TaskToggle> {
        None
    }

    fn on_context_menu(&self) -> Option<&ContextMenuHandler> {
        None
    }

    fn on_image_press(&self) -> Option<&ImageHandler> {
        None
    }

    fn on_code_block_press(&self) -> Option<&CodeHandler> {
        None
    }

    fn on_code_copy(&self) -> Option<&CodeHandler> {
        None
    }

    fn text_recognizer(&self) -> Option<&TextRecognizer> {
        None
    }

    fn link_variant_rules(&self) -> Option<&LinkVariantRules> {
        None
    }

    fn link_pill_content(&self, _read: &mut ReadCx<'_>, _url: &str) -> Option<LinkPillContent> {
        None
    }

    /// Chooses the reveal effect for streaming text. Reads can follow application signals.
    fn streaming_effects(&self, _read: &mut ReadCx<'_>) -> StreamingEffects {
        StreamingEffects::None
    }

    /// Whether streaming effects remain enabled while the system requests reduced motion.
    fn allow_reduced_motion_effects(&self) -> bool {
        false
    }

    fn render_video(&self, build: &mut BuildCx<'_>, block: &RenderBlock) -> El {
        default_video(build, block)
    }

    fn render_directive(&self, _build: &mut BuildCx<'_>, block: &RenderBlock) -> El {
        default_directive(block)
    }

    fn render_spoiler(&self, build: &mut BuildCx<'_>, block: &RenderBlock) -> El {
        default_spoiler(build, block)
    }
}

pub(super) struct DocumentRenderer;
impl MarkdownRenderer for DocumentRenderer {}

type ThemeReader = dyn Fn(&mut ReadCx<'_>) -> MarkdownTheme;
type InlineStyler = dyn Fn(&Style, &str, &mut Span);
type PillContentReader = dyn Fn(&mut ReadCx<'_>, &str) -> Option<LinkPillContent>;
type StreamingEffectsReader = dyn Fn(&mut ReadCx<'_>) -> StreamingEffects;

/// A Markdown renderer with a theme preset and optional application callbacks.
#[derive(Default)]
pub struct MarkdownConfig {
    theme: MarkdownTheme,
    theme_reader: Option<Box<ThemeReader>>,
    parse: ParseOptions,
    link: Option<Box<LinkHandler>>,
    inline: Option<Box<InlineStyler>>,
    task_toggle: Option<Box<TaskToggle>>,
    context_menu: Option<Box<ContextMenuHandler>>,
    image_press: Option<Box<ImageHandler>>,
    code_press: Option<Box<CodeHandler>>,
    code_copy: Option<Box<CodeHandler>>,
    recognizer: Option<TextRecognizer>,
    variants: Option<LinkVariantRules>,
    pills: Option<Box<PillContentReader>>,
    effects: StreamingEffects,
    effects_reader: Option<Box<StreamingEffectsReader>>,
    allow_reduced_motion: bool,
    #[cfg(feature = "math")]
    math: Option<Arc<dyn MathRenderer>>,
    #[cfg(feature = "math")]
    math_error: Option<Box<MathErrorHandler>>,
}

impl MarkdownConfig {
    pub fn document() -> Self {
        Self::default()
    }

    pub fn chat() -> Self {
        Self::default().theme(MarkdownTheme::chat())
    }

    pub fn compact() -> Self {
        Self::default().theme(MarkdownTheme::compact())
    }

    pub fn theme(mut self, theme: MarkdownTheme) -> Self {
        self.theme = theme;
        self.theme_reader = None;
        self
    }

    /// Tracks the signals read by the function for updates to the mounted document.
    pub fn theme_with(mut self, theme: impl Fn(&mut ReadCx<'_>) -> MarkdownTheme + 'static) -> Self {
        self.theme_reader = Some(Box::new(theme));
        self
    }

    pub fn parse_options(mut self, options: ParseOptions) -> Self {
        self.parse = options;
        self
    }

    pub fn on_link(mut self, handle: impl Fn(&mut WriteCx<'_>, &LinkEvent) -> bool + 'static) -> Self {
        self.link = Some(Box::new(handle));
        self
    }

    /// Edits each run after its corresponding default inline style is applied.
    pub fn style_inline(mut self, style: impl Fn(&Style, &str, &mut Span) + 'static) -> Self {
        self.inline = Some(Box::new(style));
        self
    }

    pub fn on_task_toggle(mut self, handle: impl Fn(&mut WriteCx<'_>, Range<usize>, bool) + 'static) -> Self {
        self.task_toggle = Some(Box::new(handle));
        self
    }

    pub fn on_context_menu(mut self, handle: impl Fn(&mut WriteCx<'_>, &ContextMenuEvent) -> bool + 'static) -> Self {
        self.context_menu = Some(Box::new(handle));
        self
    }

    pub fn on_image_press(mut self, handle: impl Fn(&mut WriteCx<'_>, &BlockSlot) + 'static) -> Self {
        self.image_press = Some(Box::new(handle));
        self
    }

    pub fn on_code_block_press(mut self, handle: impl Fn(&mut WriteCx<'_>, &BlockSlot) + 'static) -> Self {
        self.code_press = Some(Box::new(handle));
        self
    }

    pub fn on_code_copy(mut self, handle: impl Fn(&mut WriteCx<'_>, &BlockSlot) + 'static) -> Self {
        self.code_copy = Some(Box::new(handle));
        self
    }

    pub fn text_recognizer(mut self, recognizer: TextRecognizer) -> Self {
        self.recognizer = Some(recognizer);
        self
    }

    pub fn link_variant_rules(mut self, rules: LinkVariantRules) -> Self {
        self.variants = Some(rules);
        self
    }

    /// Reads per-URL metadata reactively when a link is rendered.
    pub fn link_pill_content(mut self, content: impl Fn(&mut ReadCx<'_>, &str) -> Option<LinkPillContent> + 'static) -> Self {
        self.pills = Some(Box::new(content));
        self
    }

    pub fn streaming_effects(mut self, effects: StreamingEffects) -> Self {
        self.effects = effects;
        self.effects_reader = None;
        self
    }

    /// Tracks the signals read by the function for updates to the mounted document.
    pub fn streaming_effects_with(mut self, effects: impl Fn(&mut ReadCx<'_>) -> StreamingEffects + 'static) -> Self {
        self.effects_reader = Some(Box::new(effects));
        self
    }

    /// Keeps streaming effects enabled when the system requests reduced motion.
    pub fn allow_reduced_motion_effects(mut self, allow: bool) -> Self {
        self.allow_reduced_motion = allow;
        self
    }

    /// Overrides typesetting for inline and display formulas in this document.
    #[cfg(feature = "math")]
    pub fn math_renderer(mut self, renderer: Arc<dyn MathRenderer>) -> Self {
        self.math = Some(renderer);
        self
    }

    /// Handles an error once per distinct failing formula in this document.
    #[cfg(feature = "math")]
    pub fn on_math_error(mut self, handle: impl Fn(&str, &MathError) + 'static) -> Self {
        self.math_error = Some(Box::new(handle));
        self
    }
}

impl MarkdownRenderer for MarkdownConfig {
    fn theme(&self, read: &mut ReadCx<'_>) -> MarkdownTheme {
        self.theme_reader.as_ref().map_or(self.theme, |theme| theme(read))
    }

    fn parse_options(&self) -> ParseOptions {
        self.parse
    }

    fn style_inline(&self, theme: &MarkdownTheme, style: &Style, label: &str, span: &mut Span) {
        default_style_inline(theme, style, label, span);
        if let Some(customize) = &self.inline {
            customize(style, label, span);
        }
    }

    fn on_link(&self) -> Option<&LinkHandler> {
        self.link.as_deref()
    }

    fn on_task_toggle(&self) -> Option<&TaskToggle> {
        self.task_toggle.as_deref()
    }

    fn on_context_menu(&self) -> Option<&ContextMenuHandler> {
        self.context_menu.as_deref()
    }

    fn on_image_press(&self) -> Option<&ImageHandler> {
        self.image_press.as_deref()
    }

    fn on_code_block_press(&self) -> Option<&CodeHandler> {
        self.code_press.as_deref()
    }

    fn on_code_copy(&self) -> Option<&CodeHandler> {
        self.code_copy.as_deref()
    }

    fn text_recognizer(&self) -> Option<&TextRecognizer> {
        self.recognizer.as_ref()
    }

    fn link_variant_rules(&self) -> Option<&LinkVariantRules> {
        self.variants.as_ref()
    }

    fn link_pill_content(&self, read: &mut ReadCx<'_>, url: &str) -> Option<LinkPillContent> {
        self.pills.as_ref().and_then(|content| content(read, url))
    }

    fn streaming_effects(&self, read: &mut ReadCx<'_>) -> StreamingEffects {
        self.effects_reader.as_ref().map_or(self.effects, |effects| effects(read))
    }

    fn allow_reduced_motion_effects(&self) -> bool {
        self.allow_reduced_motion
    }

    #[cfg(feature = "math")]
    fn math_renderer(&self) -> Option<&dyn MathRenderer> {
        self.math.as_deref()
    }

    #[cfg(feature = "math")]
    fn on_math_error(&self) -> Option<&MathErrorHandler> {
        self.math_error.as_deref()
    }
}
