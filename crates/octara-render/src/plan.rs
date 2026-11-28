//! Device-free target geometry and painter-order draw planning.

use crate::glyphs::{GlyphImage, GlyphKey, SUBPIXEL_STEPS};
use crate::paths;
use crate::shape::{ClipAxes, raster_offsets};
use batch::{NO_CLIP, clip_envelope, custom_quad, external, image, quad, shadow, xywh};
use octara_model::capture::{PaintFallback, PaintOutcome, PaintReceipt, PaintSample};
use octara_model::geometry::{Rect, Vec2};
pub use octara_model::image::{ExternalTextureHandle, ExternalTextureKind};
use octara_model::paint::{DisplayList, GlyphRun, ImagePaint, Item, PathPaint, PlacedItem, Quad, TransitionPaint};
use octara_model::theme::Color;
use paint::can_retain_canvas;
use rustc_hash::{FxHashMap, FxHashSet};
use std::borrow::Cow;
use std::ops::Range;
use std::sync::Weak;

pub(crate) mod batch;
mod damage;
pub(crate) mod glyph_instance;
pub(crate) mod instances;
mod metrics;
mod output;
pub(crate) mod packing;
pub(crate) mod paint;
mod profile;
pub(crate) mod projective;
mod schedule;
mod spatial;

pub use crate::cpu::CpuRenderer;
pub use crate::glyphs::GlyphCache;
pub use crate::image_mips::{IMAGE_ATLAS_EDGE, ImageLevel, ImageMips, PACKED_IMAGE_MAX_LOD, full_lod, packed_image_layout};
pub use crate::shape::{ClipNode, ClipStack, ClipState};
pub use batch::{Batch, Draw, Paint, Pass, PlannedBatch, Shape};
pub use damage::{DamageRect, SceneSnapshot};
pub use glyph_instance::GlyphInst;
pub use instances::{
    DrawInst, FRACTIONAL_CLIP, GlyphClip, GlyphDraw, GradientInst, ImageInst, Instances, MaterialInst, QuadInst, SPATIAL_CLIP,
    SPATIAL_SHIFT, ShadowInst,
};
pub use metrics::{TextFrameMetrics, TextFramePlanner};
pub use output::OutputColor;
pub use packing::{ArrayId, Region};
pub use paint::{CanvasMode, CanvasPaint, CanvasProgram, Material, material};
pub use profile::{FrameProfile, GpuPassKind, GpuPassTiming, GpuTimings};
pub use schedule::{fallback_draw, schedule_pass};
pub use spatial::{SpatialTable, SpatialWrite};
#[doc(hidden)]
pub use {bytemuck, color};

/// A pass's pixel bounds in window coordinates, before rounding its allocation up.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PassBounds {
    pub origin: [i32; 2],
    pub size: [u32; 2],
}

impl PassBounds {
    pub fn from_rect(rect: Rect) -> Self {
        if rect.is_empty() {
            return Self::default();
        }
        let [x, y] = [rect.x.floor() as i32, rect.y.floor() as i32];
        let [right, bottom] = [rect.right().ceil() as i32, rect.bottom().ceil() as i32];
        Self { origin: [x, y], size: [right.abs_diff(x), bottom.abs_diff(y)] }
    }

    pub fn rect(self) -> [f32; 4] {
        [self.origin[0] as f32, self.origin[1] as f32, self.size[0] as f32, self.size[1] as f32]
    }

    pub fn allocation(self, window: [u32; 2]) -> [u32; 2] {
        std::array::from_fn(|axis| {
            let size = self.size[axis].max(1);
            if size > window[axis] { size } else { size.next_multiple_of(64).min(window[axis]) }
        })
    }

    pub fn is_empty(self) -> bool {
        self.size.contains(&0)
    }
}

/// Texture dimensions and compositing requirements supplied by the caller.
#[derive(Clone, Copy, Debug)]
pub struct TargetInfo {
    /// Size of the output attachment in device pixels.
    pub extent: [u32; 2],
    pub scale: f32,
    /// Limit for an individual resource, supplied as a number independent of any device handle.
    pub max_texture_dimension: u32,
    pub copyable: bool,
    pub encode_output: bool,
}

pub struct CanvasRequest {
    pub pass: usize,
    pub paint: CanvasPaint,
    pub fallback: Paint,
    pub receipt: usize,
    pub retained_mount: Option<Weak<()>>,
}

pub struct BackdropRequest {
    pub paint: CanvasPaint,
    pub fallback: Paint,
    pub receipt: usize,
    pub mount: Weak<()>,
    pub exiting: bool,
    pub target: usize,
}

#[derive(Clone)]
pub struct TransitionRequest {
    pub paint: TransitionPaint,
    pub receipt: Option<usize>,
    pub clip: ClipState,
    pub clip_scope: usize,
}

#[derive(Default)]
pub struct PaintRequests {
    pub canvases: Vec<CanvasRequest>,
    pub backdrops: Vec<BackdropRequest>,
    pub skipped_transitions: Vec<(Weak<()>, u64)>,
    pub receipts: Vec<PaintReceipt>,
}

/// Device-independent pass geometry in painter order. Each child is composited at its
/// original position in its parent. Active transitions can expand these bounds with
/// accepted history when the encoder prepares the frame.
pub struct FramePlan {
    /// Painter-order passes, with the output at index zero and descendants later.
    pub passes: Vec<Pass>,
    pub requests: PaintRequests,
    pub clips: ClipStack,
    pub resources: ResourcePlan,
    pub spatial: SpatialTable,
    pub bounds: Vec<PassBounds>,
    pub resolve_output: bool,
    pub backdrop_inputs: Vec<BackdropInput>,
}

impl FramePlan {
    /// Pixel bounds in window coordinates, visiting child targets before their parents.
    pub fn pass_bounds(&self) -> impl Iterator<Item = [i32; 4]> + '_ {
        self.bounds.iter().rev().map(|b| [b.origin[0], b.origin[1], b.size[0] as i32, b.size[1] as i32])
    }

    /// Backdrop reads in display-list order.
    pub fn backdrop_inputs(&self) -> &[BackdropInput] {
        &self.backdrop_inputs
    }

    /// Current bounds of transition regions in display-list order.
    pub fn transitions(&self) -> impl Iterator<Item = (octara_model::identity::NodeId, [i32; 4])> + '_ {
        self.passes.iter().zip(&self.bounds).filter_map(|(pass, bounds)| {
            let request = pass.transition.as_ref()?;
            Some((request.paint.source, [bounds.origin[0], bounds.origin[1], bounds.size[0] as i32, bounds.size[1] as i32]))
        })
    }

    /// Root output needs an intermediate attachment for encoding or a visible backdrop read.
    pub fn resolves_output(&self) -> bool {
        self.resolve_output
    }
}

#[derive(Default)]
pub struct ResourcePlan {
    pub samples: Vec<DrawSample>,
    pub materials: Vec<MaterialRequest>,
    pub external_textures: Vec<ExternalTextureHandle>,
}

pub struct MaterialRequest {
    pub material: Material,
    pub fallback: Paint,
    pub receipt: usize,
}

pub enum DrawSample {
    Image(ImagePaint),
    Glyph(GlyphRequest),
    Path(PathPaint, paths::Placed),
}

pub struct GlyphRequest {
    pub key: GlyphKey,
    pub image: GlyphImage,
    pub pos: [f32; 2],
    pub color: u32,
}

/// Builds a render graph without a GPU device or access to presentation history.
pub fn plan_frame(list: &DisplayList, target: TargetInfo) -> FramePlan {
    plan_frame_cached(list, target, None, &mut GlyphCache::default())
}

/// Plan stable geometry without pruning by the current scroll position. Effects with
/// position-dependent target allocations or authored callbacks use the regular planner.
/// The returned table supplies the device-pixel offsets for that plan.
pub fn plan_frame_spatial(list: &DisplayList, target: TargetInfo, glyphs: &mut GlyphCache) -> Option<(FramePlan, SpatialTable)> {
    let scale = target.scale;
    let spatial = SpatialTable::new(list, scale)?;
    let mut pass = Pass { spatial: true, ..Pass::default() };
    let mut clips = ClipStack::default();
    let mut resources = ResourcePlan::default();
    for placed in list.placed_items_unculled_at(scale) {
        let spatial = if placed.spatial_id == 0 { 0 } else { spatial.row(placed.spatial_id)? };
        if spatial > u32::MAX >> SPATIAL_SHIFT {
            return None;
        }
        let local = placed.local_offset;
        let head = clips.current().head;
        let base = Paint { clip_head: head, spatial, ..Paint::default() };
        if let Item::Glyphs(run) = placed.item {
            if placed.spatial_offset_px.x.fract() != 0.0 || placed.spatial_offset_px.y.fract() != 0.0 {
                return None;
            }
            push_glyphs((&mut pass, &mut resources), glyphs, run, placed, scale, None, base);
            continue;
        }
        let item = if local == Default::default() && placed.fade >= 1.0 {
            Cow::Borrowed(placed.item)
        } else {
            Cow::Owned(placed.item.placed(local, placed.fade))
        };
        let (mut instance, draw) = match item.as_ref() {
            Item::Quad(q) => (quad(q, scale, NO_CLIP, head), Draw::Quad),
            Item::Shadow(s) => (shadow(s, scale, NO_CLIP, head), Draw::Shadow),
            Item::Image(i) => {
                let frame = i.image.frame();
                if i.opacity <= 0.0 || frame.width == 0 || frame.height == 0 {
                    continue;
                }
                let index = resources.samples.len();
                resources.samples.push(DrawSample::Image(i.clone()));
                (image(i, scale, NO_CLIP, head), Draw::Sample(index))
            }
            Item::Path(p) => {
                if placed.spatial_offset_px.x.fract() != 0.0 || placed.spatial_offset_px.y.fract() != 0.0 {
                    return None;
                }
                let Some(at) = paths::place(p, scale, (f32::NEG_INFINITY, f32::NEG_INFINITY, f32::INFINITY, f32::INFINITY)) else {
                    continue;
                };
                // Fall back to viewport-limited rasterization for large vector shapes.
                if u64::from(at.width) * u64::from(at.height) > u64::from(target.extent[0]) * u64::from(target.extent[1]) {
                    return None;
                }
                push_path(&mut pass, &mut resources, p, at, scale, Vec2::default(), base);
                continue;
            }
            Item::PushClip { rect, radius, .. } => {
                clips.push_spatial(*rect, *radius, scale, spatial, ClipAxes::Both);
                continue;
            }
            Item::PushAxisClip { rect, horizontal, .. } => {
                let (rect, axes) = if *horizontal {
                    (Rect::new(rect.x, 0.0, rect.w, list.size.height), ClipAxes::Horizontal)
                } else {
                    (Rect::new(0.0, rect.y, list.size.width, rect.h), ClipAxes::Vertical)
                };
                clips.push_spatial(rect, [0.0; 4], scale, spatial, axes);
                continue;
            }
            Item::PopClip => {
                clips.pop();
                continue;
            }
            Item::Native { cut: false, .. } => continue,
            _ => return None,
        };
        instance.spatial = spatial;
        pass.push(instance, draw);
    }
    let plan = FramePlan {
        passes: vec![pass],
        requests: PaintRequests::default(),
        clips,
        resources,
        spatial: SpatialTable::default(),
        bounds: vec![PassBounds { size: target.extent, ..PassBounds::default() }],
        resolve_output: target.encode_output,
        backdrop_inputs: Vec::new(),
    };
    Some((plan, spatial))
}

fn push_glyphs(
    (pass, resources): (&mut Pass, &mut ResourcePlan),
    glyphs: &mut GlyphCache,
    run: &GlyphRun,
    placed: PlacedItem<'_>,
    scale: f32,
    visible: Option<Rect>,
    base: Paint,
) {
    let size = run.size * scale;
    let (offset, shift) = if pass.spatial { (placed.local_offset, Vec2::default()) } else { raster_offsets(placed) };
    for (g, color) in run.placed_colored_at(offset, placed.fade) {
        let color = color.to_rgba8();
        if color.a == 0 {
            continue;
        }
        let color = u32::from_le_bytes([color.r, color.g, color.b, color.a]);
        let x = (g.x * scale).floor();
        let y = (g.y * scale).round();
        let subpixel = (((g.x * scale - x) * SUBPIXEL_STEPS).floor() as u8).min(SUBPIXEL_STEPS as u8 - 1);
        let key = GlyphKey::new(run, g.id, size, subpixel);
        let Some(glyph) = glyphs.rasterize(&run.font, key.clone()) else { continue };
        let rect = Rect::new(x + shift.x + glyph.left as f32, y + shift.y - glyph.top as f32, glyph.width as f32, glyph.height as f32);
        if visible.is_some_and(|visible| rect.intersect(&visible).is_empty()) {
            continue;
        }
        let index = resources.samples.len();
        resources.samples.push(DrawSample::Glyph(GlyphRequest { key, image: glyph.clone(), pos: [rect.x, rect.y], color }));
        pass.push(Paint { rect: xywh(rect), shape: Shape::Glyph(None), ..base }, Draw::Sample(index));
    }
}

fn push_path(pass: &mut Pass, resources: &mut ResourcePlan, paint: &PathPaint, at: paths::Placed, scale: f32, shift: Vec2, base: Paint) {
    let index = resources.samples.len();
    resources.samples.push(DrawSample::Path(paint.clone(), at));
    let Rect { x, y, w, h } = paint.bounds;
    pass.push(
        Paint {
            rect: [at.x as f32, at.y as f32, at.width as f32, at.height as f32],
            shape: Shape::Path {
                bounds: [x * scale + shift.x, y * scale + shift.y, w * scale, h * scale],
                color: paint.color,
                gradient: paint.gradient,
                region: None,
            },
            ..base
        },
        Draw::Sample(index),
    );
}

/// Plans a frame using a reusable glyph raster cache and optional author paint sample time.
pub fn plan_frame_cached(list: &DisplayList, target: TargetInfo, sample: Option<PaintSample>, glyphs: &mut GlyphCache) -> FramePlan {
    let scale = target.scale;
    let mut passes = vec![Pass::default()];
    let mut requests = PaintRequests::default();
    let mut resources = ResourcePlan::default();
    let mut open: Vec<usize> = vec![0];
    let mut projections = Vec::new();
    let mut spatial = SpatialTable::default();
    let mut clips = ClipStack::default();
    let [width, height] = target.extent;
    let window = Rect::new(0.0, 0.0, width as f32, height as f32);
    let mut native_transitions = None;
    for (index, placed) in list.placed_items_at(scale).enumerate() {
        let current = *open.last().expect("a pass");
        let visible = passes[current].visible_window(&window);
        let (item, path_shift) = match placed.item {
            Item::Glyphs(_) => (Cow::Borrowed(placed.item), Vec2::default()),
            Item::Path(_) => paths::raster_item(placed),
            Item::PushAxisClip { .. } if passes[current].raster_window.is_some() => {
                (placed.resolve_in(unscale(visible, scale)), Vec2::default())
            }
            _ => (placed.resolve(), Vec2::default()),
        };
        let clip_state = clips.current();
        let clip = clip_state.bounds.map_or(NO_CLIP, |c| [c.x, c.y, c.w, c.h]);
        let clip_head = clip_state.head;
        match item.as_ref() {
            Item::Shadow(s) => passes[current].push(shadow(s, scale, clip, clip_head), Draw::Shadow),
            Item::Quad(q) => passes[current].push(quad(q, scale, clip, clip_head), Draw::Quad),
            Item::Custom(paint) => {
                let fallback = quad(&paint.fallback, scale, clip, clip_head);
                if fallback.visible_bounds(&visible).is_empty() {
                    continue;
                }
                let mut instance = custom_quad(paint, &fallback);
                let source = paint.fallback.source;
                let receipt = requests.receipts.len();
                requests
                    .receipts
                    .push(PaintReceipt { source, outcome: PaintOutcome::Fallback { reason: PaintFallback::UnsupportedProgram } });
                if let Some(&material) = paint.program.downcast_ref::<Material>() {
                    let index = resources.materials.len();
                    resources.materials.push(MaterialRequest { material, fallback, receipt });
                    passes[current].push(instance, Draw::RequestedMaterial(index));
                    continue;
                } else if let Some(canvas) = paint.program.downcast_ref::<CanvasProgram>()
                    && canvas.mode != CanvasMode::Transition
                {
                    let max_dimension = target.max_texture_dimension;
                    let Some(entry) = CanvasPaint::from_custom(paint, fallback, canvas.mode, scale, sample, max_dimension) else {
                        requests.receipts[receipt].outcome = PaintOutcome::Fallback { reason: PaintFallback::InvalidSize };
                        passes[current].push(fallback, Draw::Quad);
                        continue;
                    };
                    instance.shape = Shape::Canvas { opacity: paint.opacity };
                    let draw = if canvas.mode == CanvasMode::Backdrop {
                        let index = requests.backdrops.len();
                        requests.backdrops.push(BackdropRequest {
                            paint: entry,
                            fallback,
                            receipt,
                            mount: paint.mount.downgrade(),
                            exiting: paint.exiting,
                            target: current,
                        });
                        Draw::Backdrop(index)
                    } else {
                        let index = requests.canvases.len();
                        let retained_mount =
                            (canvas.mode == CanvasMode::Retained && can_retain_canvas(entry.size)).then(|| paint.mount.downgrade());
                        requests.canvases.push(CanvasRequest { pass: current, paint: entry, fallback, receipt, retained_mount });
                        Draw::Canvas(index)
                    };
                    passes[current].push(instance, draw);
                    continue;
                }
                passes[current].push(fallback, Draw::Quad);
            }
            Item::Image(i) => {
                let frame = i.image.frame();
                if i.opacity <= 0.0 || frame.width == 0 || frame.height == 0 {
                    continue;
                }
                let instance = image(i, scale, clip, clip_head);
                let Shape::Image { bounds: [x, y, w, h], .. } = instance.shape else { unreachable!("image paint") };
                let bounds = passes[current].visible_window(&window).intersect(&clip_envelope(Rect::new(x, y, w, h)));
                if instance.visible_bounds(&bounds).is_empty() {
                    continue;
                }
                let index = resources.samples.len();
                resources.samples.push(DrawSample::Image(i.clone()));
                passes[current].push(instance, Draw::Sample(index));
            }
            Item::ExternalTexture(i) => {
                let (width, height) = i.texture.size();
                if i.opacity <= 0.0 || width == 0 || height == 0 {
                    continue;
                }
                let instance = external(i, scale, clip, clip_head);
                let Shape::Image { bounds, .. } = instance.shape else { unreachable!("external image paint") };
                let bounds = visible.intersect(&clip_envelope(batch::rect(bounds)));
                if instance.visible_bounds(&bounds).is_empty() {
                    continue;
                }
                let index = resources.external_textures.len();
                resources.external_textures.push(i.texture.clone());
                passes[current].push(instance, Draw::External(index));
            }
            Item::Glyphs(run) => {
                let visible = passes[current].visible_window(&window);
                let visible = clip_state.bounds.map_or(visible, |rect| visible.intersect(&clip_envelope(rect)));
                if visible.is_empty() {
                    continue;
                }
                let base = Paint { clip, clip_head, ..Paint::default() };
                push_glyphs((&mut passes[current], &mut resources), glyphs, run, placed, scale, Some(visible), base);
            }
            Item::Path(p) => {
                let crop = paths::raster_window(visible, clip_state.bounds);
                let Some(at) = paths::place_translated(p, scale, crop, path_shift) else { continue };
                let base = Paint { clip, clip_head, ..Paint::default() };
                push_path(&mut passes[current], &mut resources, p, at, scale, path_shift, base);
            }
            // An underlay view the platform shows: its hole, drawn destination-out.
            Item::Native { source, rect, radius, cut: true, .. } => {
                let hole = Quad {
                    source: *source,
                    rect: *rect,
                    radius: *radius,
                    fill: Some(Color::BLACK),
                    gradient: None,
                    border: [0.0; 4],
                    border_color: Color::TRANSPARENT,
                };
                passes[current].push(quad(&hole, scale, clip, clip_head), Draw::Cutout);
            }
            // Otherwise its placeholder or snapshot, drawn before, shows.
            Item::Native { .. } => {}
            Item::PushClip { rect, radius, .. } => clips.push(*rect, *radius, scale),
            Item::PushAxisClip { .. } => unreachable!("placed clips resolve to window coordinates"),
            Item::PopClip => clips.pop(),
            Item::PushLayer { opacity, .. } => {
                passes.push(Pass {
                    opacity: *opacity,
                    clip_limit: passes[current].clip_limit,
                    raster_window: passes[current].raster_window,
                    ..Pass::default()
                });
                open.push(passes.len() - 1);
            }
            Item::PushTransform { rect, transform, .. } => {
                let layer = passes.len();
                let matrix = transform.device_matrix(*rect, scale);
                let crop = clip_state.bounds.map_or(visible, |clip| visible.intersect(&clip_envelope(clip)));
                let border = batch::rect(xywh(*rect).map(|px| px * scale));
                let raster_window = Some(projective::source_window(&matrix, crop, border, target.max_texture_dimension));
                passes.push(Pass { opacity: 1.0, projection: Some(spatial.push_projection(matrix)), raster_window, ..Pass::default() });
                projections.push((layer, clip_state, clips.enter_scope()));
                open.push(layer);
            }
            Item::SkipTransition { source, mount, revision } => {
                requests.skipped_transitions.push((mount.downgrade(), *revision));
                let outcome = PaintOutcome::Fallback { reason: PaintFallback::UnsupportedRegion };
                requests.receipts.push(PaintReceipt { source: *source, outcome });
            }
            Item::PushTransition(paint) => {
                let native = native_transitions.get_or_insert_with(|| transitions_with_native(list, scale)).contains(&index);
                let supported = !native;
                let receipt = (paint.progress < 1.0).then(|| {
                    let receipt = requests.receipts.len();
                    let reason = if supported { PaintFallback::HistoryUnavailable } else { PaintFallback::UnsupportedRegion };
                    requests.receipts.push(PaintReceipt { source: paint.source, outcome: PaintOutcome::Fallback { reason } });
                    receipt
                });
                let mut clip_limit = passes[current].clip_limit;
                let transition = if supported {
                    if let Some(clip) = clip_state.bounds.map(clip_envelope) {
                        clip_limit = Some(clip_limit.map_or(clip, |outer| outer.intersect(&clip)));
                    }
                    Some(TransitionRequest { paint: paint.clone(), receipt, clip: clip_state, clip_scope: clips.enter_scope() })
                } else {
                    requests.skipped_transitions.push((paint.mount.downgrade(), paint.revision));
                    if paint.opacity == 1.0 {
                        // The native cutout must stay on the same target as ordinary painting.
                        open.push(current);
                        continue;
                    }
                    None
                };
                passes.push(Pass {
                    opacity: paint.opacity,
                    clip_limit,
                    transition,
                    raster_window: passes[current].raster_window,
                    ..Pass::default()
                });
                open.push(passes.len() - 1);
            }
            Item::PopLayer => {
                if open.len() < 2 {
                    continue;
                }
                let layer = open.pop().expect("a layer");
                let parent = *open.last().expect("a pass");
                if layer == parent {
                    continue;
                }
                let opacity = passes[layer].opacity;
                // Regular layers apply their clips inside. Transition regions apply their
                // outer clips here so snapshots can be composed through them exactly once.
                let outer = if projections.last().is_some_and(|&(index, _, _)| index == layer) {
                    let (_, state, scope) = projections.pop().expect("open projection");
                    clips.leave_scope(scope);
                    state
                } else if let Some(request) = &passes[layer].transition {
                    clips.leave_scope(request.clip_scope);
                    request.clip
                } else {
                    ClipState::default()
                };
                passes[parent].push(
                    Paint {
                        clip: outer.bounds.map_or(NO_CLIP, xywh),
                        clip_head: outer.head,
                        shape: Shape::Layer { opacity },
                        ..Paint::default()
                    },
                    Draw::Layer(layer),
                );
            }
        }
    }
    for index in open.into_iter().skip(1) {
        passes[index].transition = None;
    }
    let bounds = bound_passes(&mut passes, &spatial, target.extent, |_, _, bounds, _| bounds);
    let (backdrop_inputs, resolve_output) = backdrop_inputs(&requests.backdrops, &bounds, target);
    FramePlan { passes, requests, clips, resources, spatial, bounds, resolve_output, backdrop_inputs }
}

pub(crate) fn unscale(r: Rect, scale: f32) -> Rect {
    Rect::new(r.x / scale, r.y / scale, r.w / scale, r.h / scale)
}

pub fn backdrop_inputs(backdrops: &[BackdropRequest], bounds: &[PassBounds], output: TargetInfo) -> (Vec<BackdropInput>, bool) {
    let mut reads_output = false;
    let inputs = backdrops
        .iter()
        .map(|request| {
            let target = bounds[request.target];
            let at_output = request.target == 0;
            let extent = if at_output { output.extent } else { target.allocation(bounds[0].size) };
            if request.exiting {
                BackdropInput::Accepted
            } else if let Some(region) = BackdropRegion::crop(extent, target, request) {
                reads_output |= at_output;
                BackdropInput::Prefix(region)
            } else {
                BackdropInput::Unavailable
            }
        })
        .collect();
    (inputs, output.encode_output || (reads_output && !output.copyable))
}

/// A cropped copy expressed in target and canvas coordinates.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BackdropRegion {
    /// Origin in the current target.
    pub origin: [u32; 2],
    /// Dimensions in device pixels.
    pub size: [u32; 2],
    /// Offset of the cropped sample within the requested canvas.
    pub offset: [i32; 2],
}

impl BackdropRegion {
    fn crop(extent: [u32; 2], bounds: PassBounds, request: &BackdropRequest) -> Option<Self> {
        let [x, y, _, _] = request.fallback.rect;
        let [x, y] = [x.floor(), y.floor()];
        let [width, height] = request.paint.size;
        let canvas = Rect::new(x, y, width as f32, height as f32);
        let [cx, cy, cw, ch] = request.fallback.clip;
        let [tx, ty, tw, th] = bounds.rect();
        let target = Rect::new(tx, ty, tw.min(extent[0] as f32), th.min(extent[1] as f32));
        let crop = canvas.intersect(&Rect::new(cx, cy, cw, ch)).intersect(&target);
        if crop.is_empty() {
            return None;
        }
        let [left, top] = [crop.x.floor(), crop.y.floor()];
        Some(Self {
            origin: [(left - tx) as u32, (top - ty) as u32],
            size: [(crop.right().ceil() - left) as u32, (crop.bottom().ceil() - top) as u32],
            offset: [(left - x) as i32, (top - y) as i32],
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BackdropInput {
    /// End the current pass, copy the painter prefix, then resume with Load.
    Prefix(BackdropRegion),
    /// An exiting effect reads its last accepted pixels.
    Accepted,
    /// The copy is fully clipped or falls outside the target.
    Unavailable,
}

/// Ranges into the scheduled batches for one opaque-core and blended draw sequence.
pub struct DrawSegment {
    pub cores: Range<usize>,
    pub blended: Range<usize>,
}

/// Size the inner layers first, then use their composite bounds in the parent.
pub fn bound_passes(
    passes: &mut [Pass],
    spatial: &SpatialTable,
    size: [u32; 2],
    mut transition_bounds: impl FnMut(usize, &TransitionRequest, PassBounds, Rect) -> PassBounds,
) -> Vec<PassBounds> {
    let window = Rect::new(0.0, 0.0, size[0] as f32, size[1] as f32);
    let mut bounds = vec![PassBounds::default(); passes.len()];
    let mut composites = FxHashMap::default();
    bounds[0].size = size;
    for index in (0..passes.len()).rev() {
        let (current, children) = passes.split_at_mut(index + 1);
        let pass = &mut current[index];
        for batch in &pass.batches {
            if let Draw::Layer(layer) = batch.draw {
                let source = composites.get(&layer).copied().unwrap_or(bounds[layer]).rect();
                let rect = children[layer - index - 1]
                    .projection
                    .map_or(source, |index| xywh(projective::bounds(batch::rect(source), spatial.projection(index))));
                pass.paints[batch.instances.start].rect = rect;
            }
        }
        if index == 0 {
            break;
        }
        let visible = pass.visible_window(&window);
        let covered = pass.paints.iter().map(|inst| inst.visible_bounds(&visible)).fold(Rect::default(), union);
        bounds[index] = PassBounds::from_rect(covered);
        if let Some(request) = pass.transition.as_ref() {
            let composite = transition_bounds(index, request, bounds[index], visible);
            if composite != bounds[index] {
                composites.insert(index, composite);
            }
        }
    }
    bounds
}

pub fn transition_composite_bounds(
    current: PassBounds,
    visible: Rect,
    previous: PassBounds,
    previous_scale: f32,
    scale: f32,
) -> PassBounds {
    let [x, y, w, h] = previous.rect().map(|px| px / previous_scale * scale);
    let [nx, ny, nw, nh] = current.rect();
    let output = union(Rect::new(x, y, w, h), Rect::new(nx, ny, nw, nh)).intersect(&visible);
    PassBounds::from_rect(output)
}

pub(crate) fn union(a: Rect, b: Rect) -> Rect {
    if a.is_empty() {
        return b;
    }
    if b.is_empty() {
        return a;
    }
    let x = a.x.min(b.x);
    let y = a.y.min(b.y);
    Rect::new(x, y, a.right().max(b.right()) - x, a.bottom().max(b.bottom()) - y)
}

/// An independent native surface cannot be drawn into an Octara layer.
fn transitions_with_native(list: &DisplayList, scale: f32) -> FxHashSet<usize> {
    let mut blocked = FxHashSet::default();
    let mut layers = Vec::new();
    let mut native_count = 0;
    for (index, placed) in list.placed_items_at(scale).enumerate() {
        match placed.item {
            Item::PushLayer { .. } | Item::PushTransform { .. } => layers.push(None),
            Item::PushTransition(_) => layers.push(Some((index, native_count))),
            Item::PopLayer => {
                if let Some(Some((start, natives))) = layers.pop()
                    && natives != native_count
                {
                    blocked.insert(start);
                }
            }
            Item::Native { .. } => native_count += 1,
            _ => {}
        }
    }
    blocked
}

#[cfg(test)]
mod tests {
    use super::*;
    use octara_model::identity::NodeId;
    use octara_model::paint::{PaintInput, PaintProgram};
    use std::sync::Weak;

    #[test]
    fn plans_a_scaled_transition_and_root_backdrop_without_a_device() {
        use octara_model::geometry::Size;
        use octara_model::paint::{CustomPaint, Part, TransitionPaint};
        let source = NodeId::default();
        let rect = Rect::new(8.0, 8.0, 20.0, 10.0);
        let quad = Quad {
            source,
            rect,
            radius: [0.0; 4],
            fill: Some(Color::BLACK),
            gradient: None,
            border: [0.0; 4],
            border_color: Color::TRANSPARENT,
        };
        let backdrop = Item::Custom(CustomPaint {
            fallback: quad,
            program: PaintProgram::new(CanvasProgram::new(CanvasMode::Backdrop, ())),
            mount: Default::default(),
            exiting: false,
            input: Default::default(),
            opacity: 1.0,
        });
        let transition = Item::PushTransition(TransitionPaint {
            source,
            mount: Default::default(),
            run: Default::default(),
            revision: 3,
            program: PaintProgram::new(()),
            progress: 0.6,
            opacity: 1.0,
        });
        let list = DisplayList {
            size: Size::new(60.0, 50.0),
            background: Color::WHITE,
            parts: [backdrop, transition, Item::Quad(quad), Item::PopLayer].into_iter().map(Part::Item).collect(),
        };
        let info = TargetInfo { extent: [120, 100], scale: 2.0, max_texture_dimension: 1024, copyable: false, encode_output: false };
        let plan = plan_frame(&list, info);
        assert_eq!(plan.pass_bounds().collect::<Vec<_>>(), [[15, 15, 42, 22], [0, 0, 120, 100]]);
        assert_eq!(plan.transitions().collect::<Vec<_>>(), [(source, [15, 15, 42, 22])]);
        assert_eq!(plan.backdrop_inputs(), [BackdropInput::Prefix(BackdropRegion { origin: [16, 16], size: [40, 20], offset: [0, 0] })]);
        assert!(plan.resolves_output());
        assert!(!plan_frame(&list, TargetInfo { copyable: true, ..info }).resolves_output());
        let previous = PassBounds { origin: [5, 5], size: [12, 10] };
        let clip = Rect::new(12.0, 0.0, 54.0, 60.0);
        let composite = transition_composite_bounds(plan.bounds[1], clip, previous, 1.0, info.scale);
        assert_eq!(composite.rect(), [12.0, 10.0, 45.0, 27.0]);
    }

    #[test]
    fn frame_plan_resolves_backdrop_inputs() {
        let bounds = [PassBounds { origin: [0, 0], size: [32, 24] }, PassBounds { origin: [5, 6], size: [5, 4] }];
        let requests = || {
            Vec::from([(0, false), (1, false), (0, true)].map(|(target, exiting)| BackdropRequest {
                paint: CanvasPaint {
                    program: PaintProgram::new(()),
                    source: NodeId::default(),
                    size: [14, 10],
                    input: PaintInput::default(),
                    rect: Rect::new(2.25, 3.25, 14.0, 10.0),
                    radius: [0.0; 4],
                    sampled_at: None,
                },
                fallback: Paint { rect: [2.25, 3.25, 14.0, 10.0], clip: [4.5, 5.25, 20.0, 15.0], ..Paint::default() },
                receipt: 0,
                mount: Weak::new(),
                exiting,
                target,
            }))
        };
        let attachment = TargetInfo { extent: [12, 10], scale: 1.0, max_texture_dimension: 1024, copyable: true, encode_output: false };
        let (main, resolve) = backdrop_inputs(&requests(), &bounds, attachment);
        assert_eq!(main[0], BackdropInput::Prefix(BackdropRegion { origin: [4, 5], size: [8, 5], offset: [2, 2] }));
        assert_eq!(main[1], BackdropInput::Prefix(BackdropRegion { origin: [0, 0], size: [5, 4], offset: [3, 3] }));
        assert_eq!(main[2], BackdropInput::Accepted);
        assert!(!resolve);
        let render_only = TargetInfo { copyable: false, ..attachment };
        assert_eq!(backdrop_inputs(&requests(), &bounds, render_only), (main, true));
        let mut clipped = requests();
        clipped[0].fallback.clip = [20.0, 20.0, 5.0, 4.0];
        let (local, resolve) = backdrop_inputs(&clipped, &bounds, render_only);
        assert!(!resolve, "a clipped root read, a layer read, and an accepted image require no output resolve");
        assert_eq!(local[0], BackdropInput::Unavailable);
    }

    #[test]
    fn nested_layers_bound_their_painted_pixels_instead_of_the_window() {
        let window = [1600, 1200];
        let composite = Paint { shape: Shape::Layer { opacity: 1.0 }, ..Paint::default() };
        let quad = Paint { rect: [370.5, 460.25, 28.0, 18.0], clip: NO_CLIP, ..Paint::default() };
        let shadow = Paint {
            rect: [400.25, 480.5, 21.0, 12.0],
            clip: [404.0, 480.0, 10.0, 10.0],
            shape: Shape::Shadow { color: Color::BLACK, sigma: 2.0 },
            ..Paint::default()
        };
        let mut passes = [Pass::default(), Pass::default(), Pass::default()];
        passes[0].push(composite, Draw::Layer(1));
        passes[1].push(quad, Draw::Quad);
        passes[1].push(composite, Draw::Layer(2));
        passes[2].push(shadow, Draw::Shadow);
        let bounds = bound_passes(&mut passes, &SpatialTable::default(), window, |_, _, _, _| unreachable!("no transition regions"));
        assert_eq!(bounds[0].size, window);
        assert_eq!(bounds[1].rect(), [369.0, 459.0, 46.0, 32.0]);
        assert_eq!(bounds[2].rect(), [403.0, 479.0, 12.0, 12.0]);
        assert_eq!(passes[0].paints[0].rect, bounds[1].rect());
        assert_eq!(passes[1].paints[1].rect, bounds[2].rect());
        assert_eq!(bounds[1].allocation(window), [64, 64]);
        assert_eq!(bounds[2].allocation(window), [64, 64]);
    }
}
