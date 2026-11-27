//! Device-independent paint requests, before atlas allocation and draw scheduling.

use super::atlas::{ArrayId, Region};
use super::glyph_pipeline::GlyphInst;
use super::{custom, plan::TransitionRequest};
use color::LinearSrgb;
use octara_model::geometry::Rect;
use octara_model::paint::{CustomPaint, ImagePaint, Quad, Shadow};
use octara_model::theme::{Color, Gradient};
use std::ops::Range;

pub(super) const NO_CLIP: [f32; 4] = [-1.0e6, -1.0e6, 2.0e6, 2.0e6];

#[derive(Clone, Copy)]
pub(super) enum Shape {
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
pub(super) struct Paint {
    pub(super) rect: [f32; 4],
    pub(super) clip: [f32; 4],
    pub(super) clip_head: u32,
    pub(super) radius: [f32; 4],
    pub(super) shape: Shape,
}

impl Default for Paint {
    fn default() -> Self {
        Self { rect: [0.0; 4], clip: NO_CLIP, clip_head: 0, radius: [0.0; 4], shape: Shape::default() }
    }
}

pub(super) fn rect([x, y, w, h]: [f32; 4]) -> Rect {
    Rect::new(x, y, w, h)
}

pub(super) fn xywh(r: Rect) -> [f32; 4] {
    [r.x, r.y, r.w, r.h]
}

pub(super) fn grow(rect: Rect, padding: f32) -> Rect {
    let inset = xywh(rect);
    Rect::new(inset[0] - padding, inset[1] - padding, inset[2] + 2.0 * padding, inset[3] + 2.0 * padding)
}

impl Paint {
    pub(super) fn padding(&self, aa: f32) -> f32 {
        match self.shape {
            Shape::Quad { .. } | Shape::Image { .. } | Shape::Canvas { .. } | Shape::Material { .. } => aa,
            Shape::Shadow { sigma, .. } => 3.0 * sigma + aa,
            _ => 0.0,
        }
    }

    pub(super) fn visible_bounds(&self, window: &Rect) -> Rect {
        grow(rect(self.rect), self.padding(1.0)).intersect(&clip_envelope(rect(self.clip))).intersect(window)
    }
}

pub(super) fn clip_envelope(clip: Rect) -> Rect {
    if clip.is_empty() { Rect::default() } else { grow(clip, 1.0) }
}

pub(super) fn straight(c: Color) -> [f32; 4] {
    c.convert::<LinearSrgb>().components
}

pub(super) fn custom_quad(paint: &CustomPaint, fallback: &Paint) -> Paint {
    Paint { shape: Shape::Material { opacity: paint.opacity, params: paint.input.params }, ..*fallback }
}

pub(super) fn image(i: &ImagePaint, scale: f32, clip: [f32; 4], clip_head: u32) -> Paint {
    Paint {
        rect: xywh(i.rect).map(|px| px * scale),
        clip,
        clip_head,
        radius: i.bounds.corner_radii(i.radius).map(|radius| radius * scale),
        shape: Shape::Image { bounds: xywh(i.bounds).map(|px| px * scale), opacity: i.opacity, region: None },
    }
}

pub(super) fn shadow(s: &Shadow, scale: f32, clip: [f32; 4], clip_head: u32) -> Paint {
    Paint {
        rect: xywh(s.rect).map(|px| px * scale),
        clip,
        clip_head,
        radius: s.rect.corner_radii(s.radius).map(|radius| radius * scale),
        shape: Shape::Shadow { color: s.color, sigma: s.blur * 0.5 * scale },
    }
}

pub(super) fn quad(q: &Quad, scale: f32, clip: [f32; 4], clip_head: u32) -> Paint {
    Paint {
        rect: xywh(q.rect).map(|px| px * scale),
        clip,
        clip_head,
        radius: q.rect.corner_radii(q.radius).map(|radius| radius * scale),
        shape: Shape::Quad { fill: q.fill, gradient: q.gradient, border_widths: q.border.map(|b| b * scale), border_color: q.border_color },
    }
}

/// Requests drawn into one target: the window, an opacity layer or a transition region.
#[derive(Default)]
pub(super) struct Pass {
    pub(super) paints: Vec<Paint>,
    pub(super) batches: Vec<Batch>,
    pub(super) opacity: f32,
    pub(super) clip_limit: Option<Rect>,
    pub(super) transition: Option<TransitionRequest>,
}

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub(super) enum Draw {
    Quad,
    Shadow,
    Image(ArrayId),
    Cutout,
    Layer(usize),
    Material(custom::Material),
    RequestedMaterial(usize),
    Sample(usize),
    Glyph(ArrayId),
    Canvas(usize),
    Backdrop(usize),
}

#[derive(Debug, PartialEq, Eq)]
pub(super) struct Batch {
    pub(super) instances: Range<usize>,
    pub(super) draw: Draw,
}

#[derive(Debug, PartialEq, Eq)]
pub(super) struct PlannedBatch {
    pub(super) draw: Draw,
    pub(super) instances: Range<u32>,
}

impl Pass {
    pub(super) fn push(&mut self, paint: Paint, draw: Draw) {
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

    pub(super) fn visible_window(&self, window: &Rect) -> Rect {
        self.clip_limit.map_or(*window, |limit| window.intersect(&limit))
    }
}
