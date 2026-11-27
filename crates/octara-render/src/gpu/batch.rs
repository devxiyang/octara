//! Draw kinds and adjacent batch coalescing within a target.

use super::atlas::{Bindings, PageId};
use super::glyph_pipeline::GlyphInst;
use super::{custom, plan::TransitionRequest};
use bytemuck::{Pod, Zeroable};
use color::LinearSrgb;
use octara_model::geometry::Rect;
use octara_model::paint::{CustomPaint, ImagePaint, Quad, Shadow};
use octara_model::theme::Color;
use std::ops::Range;

pub(super) const NO_CLIP: [f32; 4] = [-1.0e6, -1.0e6, 2.0e6, 2.0e6];
const KIND_QUAD: u32 = 0;
pub(super) const KIND_SHADOW: u32 = 3;
const KIND_IMAGE: u32 = 4;
pub(super) const KIND_LAYER: u32 = 5;
pub(super) const KIND_PATH: u32 = 6;
pub(super) const KIND_CANVAS: u32 = 7;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub(super) struct Inst {
    pub(super) rect: [f32; 4],
    pub(super) fill: [f32; 4],
    pub(super) border_color: [f32; 4],
    pub(super) border: [f32; 4],
    pub(super) clip: [f32; 4],
    pub(super) params: InstParams,
    /// Atlas coordinates, or the four author parameters for a material.
    pub(super) uv: [f32; 4],
    /// A gradient's end color (straight); `fill` is its start.
    pub(super) grad_color: [f32; 4],
    /// A gradient's angle (degrees), start and end fractions, and 1 if there is one.
    pub(super) grad: [f32; 4],
    pub(super) radius: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Default, Pod, Zeroable)]
pub(super) struct InstParams {
    pub(super) kind: u32,
    pub(super) clip_head: u32,
    pub(super) filled: u32,
}

impl Inst {
    pub(super) fn visible_bounds(&self, window: &Rect) -> Rect {
        // Match the vertex shader's expansion for antialiasing and shadows.
        let grow = match self.params.kind {
            KIND_QUAD | KIND_IMAGE | KIND_CANVAS => 1.0,
            KIND_SHADOW => 3.0 * self.border[0] + 1.0,
            _ => 0.0,
        };
        let [x, y, w, h] = self.rect;
        let bounds = Rect::new(x - grow, y - grow, w + 2.0 * grow, h + 2.0 * grow);
        let [x, y, w, h] = self.clip;
        bounds.intersect(&clip_envelope(Rect::new(x, y, w, h))).intersect(window)
    }
}

pub(super) fn clip_envelope(clip: Rect) -> Rect {
    if clip.is_empty() {
        return Rect::default();
    }
    // Rounded clips antialias past their nominal edge.
    Rect::new(clip.x - 1.0, clip.y - 1.0, clip.w + 2.0, clip.h + 2.0)
}

pub(super) fn straight(c: Color) -> [f32; 4] {
    c.convert::<LinearSrgb>().components
}

pub(super) fn custom_quad(paint: &CustomPaint, fallback: &Inst) -> Inst {
    Inst {
        rect: fallback.rect,
        fill: [0.0, 0.0, 0.0, paint.opacity],
        clip: fallback.clip,
        params: InstParams { filled: 0, ..fallback.params },
        uv: paint.input.params,
        radius: fallback.radius,
        ..Inst::zeroed()
    }
}

pub(super) fn image(i: &ImagePaint, scale: f32, clip: [f32; 4], clip_head: u32) -> Inst {
    let (r, b) = (i.rect, i.bounds);
    Inst {
        rect: [r.x * scale, r.y * scale, r.w * scale, r.h * scale],
        fill: [0.0, 0.0, 0.0, i.opacity],
        border_color: [0.0; 4],
        border: [b.x * scale, b.y * scale, b.w * scale, b.h * scale],
        clip,
        params: InstParams { kind: KIND_IMAGE, clip_head, ..InstParams::default() },
        radius: i.bounds.corner_radii(i.radius).map(|radius| radius * scale),
        uv: [0.0; 4],
        grad_color: [0.0; 4],
        grad: [0.0; 4],
    }
}

pub(super) fn shadow(s: &Shadow, scale: f32, clip: [f32; 4], clip_head: u32) -> Inst {
    let r = s.rect;
    Inst {
        rect: [r.x * scale, r.y * scale, r.w * scale, r.h * scale],
        fill: straight(s.color),
        border_color: [0.0; 4],
        border: [s.blur * 0.5 * scale, 0.0, 0.0, 0.0],
        clip,
        params: InstParams { kind: KIND_SHADOW, clip_head, ..InstParams::default() },
        radius: r.corner_radii(s.radius).map(|radius| radius * scale),
        uv: [0.0; 4],
        grad_color: [0.0; 4],
        grad: [0.0; 4],
    }
}

pub(super) fn quad(q: &Quad, scale: f32, clip: [f32; 4], clip_head: u32) -> Inst {
    let r = q.rect;
    let (fill, grad_color, grad) = match (q.gradient, q.fill) {
        (Some(g), _) => (straight(g.from.0), straight(g.to.0), [g.angle, g.from.1, g.to.1, 1.0]),
        (None, f) => (f.map_or([0.0; 4], straight), [0.0; 4], [0.0; 4]),
    };
    let filled = q.fill.is_some() || q.gradient.is_some();
    Inst {
        rect: [r.x * scale, r.y * scale, r.w * scale, r.h * scale],
        fill,
        border_color: straight(q.border_color),
        border: q.border.map(|b| b * scale),
        clip,
        params: InstParams { kind: KIND_QUAD, clip_head, filled: u32::from(filled) },
        radius: r.corner_radii(q.radius).map(|radius| radius * scale),
        uv: [0.0; 4],
        grad_color,
        grad,
    }
}

/// The instances drawn into one target: the window, an opacity layer or a transition region.
#[derive(Default)]
pub(super) struct Pass {
    pub(super) insts: Vec<Inst>,
    pub(super) glyphs: Vec<GlyphInst>,
    pub(super) glyph_bounds: Rect,
    pub(super) batches: Vec<Batch>,
    pub(super) opacity: f32,
    /// Allocation limit inherited from enclosing transition regions' outer clips.
    pub(super) clip_limit: Option<Rect>,
    pub(super) transition: Option<TransitionRequest>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Draw {
    Standard(Bindings),
    Cutout,
    Layer(usize),
    Material(custom::Material),
    RequestedMaterial(usize),
    Sample(usize),
    GlyphSample,
    Glyph(PageId),
    Canvas(usize),
    Backdrop(usize),
}

impl Draw {
    pub(super) const PLAIN: Self = Self::Standard(Bindings::EMPTY);

    fn merge(&mut self, next: Self) -> bool {
        if *self == next {
            return true;
        }
        match (self, next) {
            (Self::Standard(bindings), Self::Standard(next)) => bindings.merge(next),
            _ => false,
        }
    }
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
    pub(super) fn push(&mut self, instance: Inst, draw: Draw) {
        let start = self.insts.len();
        self.insts.push(instance);
        self.push_batch(draw, start);
    }

    pub(super) fn push_glyph(&mut self, instance: GlyphInst, page: PageId) {
        let start = self.glyphs.len();
        self.glyphs.push(instance);
        self.push_batch(Draw::Glyph(page), start);
    }

    pub(super) fn push_glyph_sample(&mut self, index: usize, visible: Rect) {
        self.glyph_bounds = super::plan::union(self.glyph_bounds, visible);
        self.push_batch(Draw::GlyphSample, index);
    }

    fn push_batch(&mut self, draw: Draw, start: usize) {
        if let Some(last) = self.batches.last_mut()
            && last.instances.end == start
            && last.draw.merge(draw)
        {
            last.instances.end += 1;
        } else {
            self.batches.push(Batch { instances: start..start + 1, draw });
        }
    }

    /// Coalesce adjacent batches again after canvas preparation has chosen any fallbacks.
    pub(super) fn append_batches(&self, [base, glyph_base]: [u32; 2], batches: &mut Vec<PlannedBatch>) -> Range<usize> {
        let start = batches.len();
        for batch in &self.batches {
            let base = if matches!(batch.draw, Draw::Glyph(_)) { glyph_base } else { base };
            let instances = base + batch.instances.start as u32..base + batch.instances.end as u32;
            if let Some(last) = batches[start..].last_mut()
                && last.instances.end == instances.start
                && last.draw.merge(batch.draw)
            {
                last.instances.end = instances.end;
            } else {
                batches.push(PlannedBatch { draw: batch.draw, instances });
            }
        }
        start..batches.len()
    }

    pub(super) fn visible_window(&self, window: &Rect) -> Rect {
        self.clip_limit.map_or(*window, |limit| window.intersect(&limit))
    }
}
