//! Device-independent paint requests, before atlas allocation and draw scheduling.

use super::glyph_instance::GlyphInst;
use super::packing::{ArrayId, Region};
use super::{Material, TransitionRequest, union};
use color::LinearSrgb;
use octara_model::geometry::Rect;
use octara_model::paint::{CustomPaint, ExternalTexturePaint, ImagePaint, Quad, Shadow};
use octara_model::theme::{Color, Gradient};
use std::ops::Range;

pub(crate) const NO_CLIP: [f32; 4] = [-1.0e6, -1.0e6, 2.0e6, 2.0e6];

#[derive(Clone, Copy)]
pub enum Shape {
    Quad { fill: Option<Color>, gradient: Option<Gradient>, border_widths: [f32; 4], border_color: Color },
    Shadow { color: Color, sigma: f32 },
    Image { bounds: [f32; 4], opacity: f32, region: Option<Region> },
    Path { bounds: [f32; 4], color: Color, gradient: Option<Gradient>, region: Option<Region> },
    Glyph(Option<GlyphInst>),
    Layer { opacity: f32 },
    Canvas { opacity: f32 },
    Material { opacity: f32, params: [f32; 4] },
}

impl Default for Shape {
    fn default() -> Self {
        Self::Quad { fill: None, gradient: None, border_widths: [0.0; 4], border_color: Color::TRANSPARENT }
    }
}

#[derive(Clone, Copy)]
pub struct Paint {
    pub rect: [f32; 4],
    pub clip: [f32; 4],
    pub clip_head: u32,
    pub spatial: u32,
    pub radius: [f32; 4],
    pub shape: Shape,
}

impl Default for Paint {
    fn default() -> Self {
        Self { rect: [0.0; 4], clip: NO_CLIP, clip_head: 0, spatial: 0, radius: [0.0; 4], shape: Shape::default() }
    }
}

pub(crate) fn rect([x, y, w, h]: [f32; 4]) -> Rect {
    Rect::new(x, y, w, h)
}

pub(crate) fn xywh(r: Rect) -> [f32; 4] {
    [r.x, r.y, r.w, r.h]
}

pub(crate) fn grow(rect: Rect, padding: f32) -> Rect {
    let inset = xywh(rect);
    Rect::new(inset[0] - padding, inset[1] - padding, inset[2] + 2.0 * padding, inset[3] + 2.0 * padding)
}

impl Paint {
    pub(crate) fn padding(&self, aa: f32) -> f32 {
        match self.shape {
            Shape::Quad { .. } | Shape::Image { .. } | Shape::Canvas { .. } | Shape::Material { .. } => aa,
            Shape::Shadow { sigma, .. } => 3.0 * sigma + aa,
            _ => 0.0,
        }
    }

    pub(crate) fn visible_bounds(&self, window: &Rect) -> Rect {
        grow(rect(self.rect), self.padding(1.0)).intersect(&clip_envelope(rect(self.clip))).intersect(window)
    }
}

pub(crate) fn clip_envelope(clip: Rect) -> Rect {
    if clip.is_empty() { Rect::default() } else { grow(clip, 1.0) }
}

pub(crate) fn straight(c: Color) -> [f32; 4] {
    c.convert::<LinearSrgb>().components
}

pub(crate) fn custom_quad(paint: &CustomPaint, fallback: &Paint) -> Paint {
    Paint { shape: Shape::Material { opacity: paint.opacity, params: paint.input.params }, ..*fallback }
}

pub(crate) fn image(i: &ImagePaint, scale: f32, clip: [f32; 4], clip_head: u32) -> Paint {
    image_shape(i.rect, i.bounds, i.radius, i.opacity, scale, Paint { clip, clip_head, ..Paint::default() })
}

pub(crate) fn external(i: &ExternalTexturePaint, scale: f32, clip: [f32; 4], clip_head: u32) -> Paint {
    image_shape(i.rect, i.bounds, i.radius, i.opacity, scale, Paint { clip, clip_head, ..Paint::default() })
}

fn image_shape(rect: Rect, bounds: Rect, radius: [f32; 4], opacity: f32, scale: f32, base: Paint) -> Paint {
    Paint {
        rect: xywh(rect).map(|px| px * scale),
        radius: bounds.corner_radii(radius).map(|radius| radius * scale),
        shape: Shape::Image { bounds: xywh(bounds).map(|px| px * scale), opacity, region: None },
        ..base
    }
}

pub(crate) fn shadow(s: &Shadow, scale: f32, clip: [f32; 4], clip_head: u32) -> Paint {
    Paint {
        rect: xywh(s.rect).map(|px| px * scale),
        clip,
        clip_head,
        spatial: 0,
        radius: s.rect.corner_radii(s.radius).map(|radius| radius * scale),
        shape: Shape::Shadow { color: s.color, sigma: s.blur * 0.5 * scale },
    }
}

pub(crate) fn quad(q: &Quad, scale: f32, clip: [f32; 4], clip_head: u32) -> Paint {
    Paint {
        rect: xywh(q.rect).map(|px| px * scale),
        clip,
        clip_head,
        spatial: 0,
        radius: q.rect.corner_radii(q.radius).map(|radius| radius * scale),
        shape: Shape::Quad { fill: q.fill, gradient: q.gradient, border_widths: q.border.map(|b| b * scale), border_color: q.border_color },
    }
}

/// Requests drawn into one target: the window, an opacity layer or a transition region.
#[derive(Default)]
pub struct Pass {
    pub paints: Vec<Paint>,
    pub batches: Vec<Batch>,
    pub spatial: bool,
    pub opacity: f32,
    pub clip_limit: Option<Rect>,
    pub transition: Option<TransitionRequest>,
    /// Index in the spatial table for projecting source pixels into the parent.
    pub projection: Option<usize>,
    /// Source coordinates visible through the surrounding projected layers.
    pub(crate) raster_window: Option<Rect>,
}

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub enum Draw {
    Quad,
    Shadow,
    Image(ArrayId),
    Cutout,
    Layer(usize),
    Material(Material),
    RequestedMaterial(usize),
    Sample(usize),
    External(usize),
    Glyph(ArrayId),
    Canvas(usize),
    Backdrop(usize),
}

#[derive(Debug, PartialEq, Eq)]
pub struct Batch {
    pub instances: Range<usize>,
    pub draw: Draw,
}

/// A consecutive run of instances sharing a pipeline and resource binding.
#[derive(Debug, PartialEq)]
pub struct PlannedBatch {
    pub draw: Draw,
    pub instances: Range<u32>,
    bounds: Vec<Rect>,
    spans: Vec<DrawSpan>,
}

#[derive(Debug, PartialEq)]
struct DrawSpan {
    bounds: Rect,
    spatial: u32,
    end: u32,
}

impl PlannedBatch {
    pub(crate) fn new(draw: Draw, first: u32) -> Self {
        Self { draw, instances: first..first, bounds: Vec::new(), spans: Vec::new() }
    }

    pub(crate) fn extend(&mut self, index: u32, bounds: Rect, spatial: u32) {
        const SPAN_LEN: usize = 32;
        self.instances.end = index + 1;
        if self.bounds.len().is_multiple_of(SPAN_LEN) || self.spans.last().is_none_or(|last| last.spatial != spatial) {
            self.spans.push(DrawSpan { bounds, spatial, end: index + 1 });
        } else if let Some(last) = self.spans.last_mut() {
            last.bounds = union(last.bounds, bounds);
            last.end = index + 1;
        }
        self.bounds.push(bounds);
    }

    /// Emits consecutive ranges intersecting the damage bounds, including spatial offsets.
    pub fn visible_ranges(&self, damage: Rect, spatials: &[[f32; 4]], mut emit: impl FnMut(Range<u32>)) -> u32 {
        let mut start = self.instances.start;
        let mut next = start..start;
        let mut draws = 0;
        for span in &self.spans {
            let [x, y, ..] = spatials.get(span.spatial as usize).copied().unwrap_or([0.0; 4]);
            let local = Rect::new(damage.x - x, damage.y - y, damage.w, damage.h);
            if !span.bounds.intersect(&local).is_empty() {
                for index in start..span.end {
                    if self.bounds[(index - self.instances.start) as usize].intersect(&local).is_empty() {
                        continue;
                    }
                    if next.end == index || next.is_empty() {
                        if next.is_empty() {
                            next.start = index;
                        }
                        next.end = index + 1;
                    } else {
                        emit(next);
                        draws += 1;
                        next = index..index + 1;
                    }
                }
            }
            start = span.end;
        }
        if !next.is_empty() {
            emit(next);
            draws += 1;
        }
        draws
    }
}

impl Pass {
    pub fn push(&mut self, paint: Paint, draw: Draw) {
        let start = self.paints.len();
        self.paints.push(paint);
        if let Some(last) = self.batches.last_mut()
            && last.draw == draw
        {
            last.instances.end += 1;
        } else {
            self.batches.push(Batch { instances: start..start + 1, draw });
        }
    }

    pub(crate) fn visible_window(&self, window: &Rect) -> Rect {
        let window = self.raster_window.unwrap_or(*window);
        self.clip_limit.map_or(window, |limit| window.intersect(&limit))
    }
}
