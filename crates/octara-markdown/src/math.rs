//! TeX typesetting whose output uses Octara's normal vector and text selection paths.

mod layout;
mod parser;

use crate::{MarkdownRenderer, MarkdownTheme};
use octara::prelude::*;
use std::{
    collections::{HashSet, VecDeque},
    fmt,
    ops::Range,
    sync::{Arc, Mutex},
};

/// Rendering parameters in logical pixels. Pass color tokens through to your
/// drawing operations to follow the window theme without typesetting again.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MathRenderStyle {
    pub font_size: f32,
    pub color: ColorValue,
    /// Display formulas have room for larger operators and stacked limits.
    pub display: bool,
}

/// A formula's intrinsic dimensions and drawing in local coordinates.
#[derive(Clone, Debug, PartialEq)]
pub struct MathLayout {
    pub size: Size,
    /// The distance from the top of the formula to its baseline in logical pixels.
    pub baseline: f32,
    pub ops: Arc<[CanvasOp]>,
}

impl MathLayout {
    pub(super) fn attachment(self, range: Range<usize>) -> InlineAttachment {
        InlineAttachment {
            range,
            size: self.size,
            align: InlineAlign::Baseline,
            baseline: Some(self.baseline),
            content: InlineContent::Vector { ops: self.ops },
        }
    }
}

/// An error from a TeX parser or typesetter.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MathError(Arc<str>);

impl MathError {
    pub fn new(message: impl Into<Arc<str>>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for MathError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for MathError {}

/// Receives each failing formula at most once per mounted Markdown document.
pub type MathErrorHandler = dyn Fn(&str, &MathError);

/// Scoped renderer for both inline and display formulas. The input omits the
/// surrounding dollar delimiters. An error displays selectable monospace TeX.
///
/// Supply an `Arc<dyn MathRenderer>` with `BuildCx::provide`, or override
/// `MarkdownRenderer::math_renderer` for a particular document.
pub trait MathRenderer: Send + Sync + 'static {
    fn render(&self, tex: &str, style: MathRenderStyle) -> Result<MathLayout, MathError>;
}

/// The built-in TeX subset with DPI-independent outlines. Glyphs come from the
/// bundled STIX Two Math subset under the SIL Open Font License.
#[derive(Clone, Copy, Debug, Default)]
pub struct BuiltinMathRenderer;

impl MathRenderer for BuiltinMathRenderer {
    fn render(&self, tex: &str, style: MathRenderStyle) -> Result<MathLayout, MathError> {
        geometry(tex, style.display).map(|geometry| geometry.render(style))
    }
}

fn geometry(tex: &str, display: bool) -> Result<layout::Geometry, MathError> {
    layout::layout(&parser::parse(tex)?, display)
}

#[derive(Clone)]
pub(super) struct MathDocument {
    scoped: Option<Arc<dyn MathRenderer>>,
    state: Arc<Mutex<State>>,
}

#[derive(Default)]
struct State {
    reported: HashSet<Arc<str>>,
    recent: VecDeque<CacheEntry>,
}

struct CacheEntry {
    tex: Arc<str>,
    display: bool,
    geometry: Result<Arc<layout::Geometry>, MathError>,
}

impl State {
    fn geometry(&mut self, tex: &str, display: bool) -> Result<Arc<layout::Geometry>, MathError> {
        if let Some(index) = self.recent.iter().position(|entry| entry.display == display && entry.tex.as_ref() == tex) {
            let entry = self.recent.remove(index).expect("known geometry cache entry");
            let result = entry.geometry.clone();
            self.recent.push_back(entry);
            return result;
        }
        let geometry = geometry(tex, display).map(Arc::new);
        if tex.len() <= 512 {
            self.recent.push_back(CacheEntry { tex: tex.into(), display, geometry: geometry.clone() });
            if self.recent.len() > 48 {
                self.recent.pop_front();
            }
        }
        geometry
    }
}

impl MathDocument {
    pub(super) fn new(build: &BuildCx<'_>) -> Self {
        Self { scoped: build.context::<Arc<dyn MathRenderer>>(), state: Arc::new(Mutex::new(State::default())) }
    }

    pub(super) fn render(&self, renderer: &dyn MarkdownRenderer, tex: &str, style: MathRenderStyle, complete: bool) -> Option<MathLayout> {
        let result = if let Some(renderer) = renderer.math_renderer().or(self.scoped.as_deref()) {
            renderer.render(tex, style)
        } else {
            let geometry = self.state.lock().expect("math document cache").geometry(tex, style.display);
            geometry.map(|geometry| geometry.render(style))
        };
        match result {
            Ok(layout) => Some(layout),
            Err(error) => {
                if complete && let Some(handle) = renderer.on_math_error() {
                    let first = self.state.lock().expect("math document cache").reported.insert(tex.into());
                    if first {
                        handle(tex, &error);
                    }
                }
                None
            }
        }
    }
}

pub(super) fn render_style(theme: &MarkdownTheme, span: Option<Span>, display: bool) -> MathRenderStyle {
    let defaults = Theme::default();
    let span = span.unwrap_or_default();
    let font_size = span.size.unwrap_or_else(|| match theme.text_size {
        FontSize::Len(length) => length.px(&defaults).unwrap_or_else(|| defaults.text_scale_of(FontSizeToken::Base).size),
        FontSize::Token(token) => defaults.text_scale_of(token).size,
    });
    let color = span.color.or(theme.math.color).unwrap_or(theme.text_color);
    MathRenderStyle { font_size, color, display }
}
