//! Device-free target geometry and painter-order draw planning.

use super::batch::{
    Draw, Inst, InstParams, KIND_CANVAS, KIND_LAYER, KIND_PATH, NO_CLIP, Pass, PlannedBatch, clip_envelope, custom_quad, image, quad,
    shadow, straight,
};
use super::custom::{self, CanvasMode, CanvasPaint};
use super::outputs::CanvasCache;
use crate::glyphs::{GlyphCache, GlyphImage, GlyphKey, SUBPIXEL_STEPS};
use crate::paths;
use crate::shape::{ClipStack, ClipState};
use bytemuck::Zeroable;
use octara_model::capture::{PaintFallback, PaintOutcome, PaintReceipt, PaintSample};
use octara_model::geometry::Rect;
use octara_model::paint::Item;
use octara_model::paint::{DisplayList, ImagePaint, PathPaint, Quad, TransitionPaint};
use octara_model::theme::Color;
use rustc_hash::{FxHashMap, FxHashSet};
use std::ops::Range;
use std::sync::Weak;

/// A pass's pixel bounds in window coordinates, before rounding its allocation up.
#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct PassBounds {
    pub(super) origin: [u32; 2],
    pub(super) size: [u32; 2],
}

impl PassBounds {
    pub(super) fn from_rect(rect: Rect) -> Self {
        if rect.is_empty() {
            return Self::default();
        }
        let origin = [rect.x.floor() as u32, rect.y.floor() as u32];
        Self { origin, size: [rect.right().ceil() as u32 - origin[0], rect.bottom().ceil() as u32 - origin[1]] }
    }

    pub(super) fn rect(self) -> [f32; 4] {
        [self.origin[0] as f32, self.origin[1] as f32, self.size[0] as f32, self.size[1] as f32]
    }

    pub(super) fn allocation(self, window: [u32; 2]) -> [u32; 2] {
        std::array::from_fn(|axis| self.size[axis].max(1).next_multiple_of(64).min(window[axis].max(1)))
    }

    pub(super) fn is_empty(self) -> bool {
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

pub(super) struct CanvasRequest {
    pub(super) pass: usize,
    pub(super) paint: CanvasPaint,
    pub(super) fallback: Inst,
    pub(super) receipt: usize,
    pub(super) retained_mount: Option<Weak<()>>,
}

pub(super) struct BackdropRequest {
    pub(super) paint: CanvasPaint,
    pub(super) fallback: Inst,
    pub(super) receipt: usize,
    pub(super) mount: Weak<()>,
    pub(super) exiting: bool,
    pub(super) target: usize,
}

#[derive(Clone)]
pub(super) struct TransitionRequest {
    pub(super) paint: TransitionPaint,
    pub(super) receipt: Option<usize>,
    pub(super) clip: ClipState,
    pub(super) clip_scope: usize,
}

#[derive(Default)]
pub(super) struct PaintRequests {
    pub(super) canvases: Vec<CanvasRequest>,
    pub(super) backdrops: Vec<BackdropRequest>,
    pub(super) skipped_transitions: Vec<(Weak<()>, u64)>,
    pub(super) receipts: Vec<PaintReceipt>,
}

/// Device-independent pass geometry in painter order. Each child is composited at its
/// original position in its parent. Active transitions can expand these bounds with
/// accepted history when the encoder prepares the frame.
pub struct FramePlan {
    pub(super) passes: Vec<Pass>,
    pub(super) requests: PaintRequests,
    pub(super) clips: ClipStack,
    pub(super) resources: ResourcePlan,
    pub(super) bounds: Vec<PassBounds>,
    pub(super) resolve_output: bool,
    pub(super) backdrop_inputs: Vec<BackdropInput>,
}

impl FramePlan {
    /// Pixel bounds in window coordinates, visiting child targets before their parents.
    pub fn pass_bounds(&self) -> impl Iterator<Item = [u32; 4]> + '_ {
        self.bounds.iter().rev().map(|b| [b.origin[0], b.origin[1], b.size[0], b.size[1]])
    }

    /// Backdrop reads in display-list order.
    pub fn backdrop_inputs(&self) -> &[BackdropInput] {
        &self.backdrop_inputs
    }

    /// Current bounds of transition regions in display-list order.
    pub fn transitions(&self) -> impl Iterator<Item = (octara_model::identity::NodeId, [u32; 4])> + '_ {
        self.passes.iter().zip(&self.bounds).filter_map(|(pass, bounds)| {
            let request = pass.transition.as_ref()?;
            Some((request.paint.source, [bounds.origin[0], bounds.origin[1], bounds.size[0], bounds.size[1]]))
        })
    }

    /// Root output needs an intermediate attachment for encoding or a visible backdrop read.
    pub fn resolves_output(&self) -> bool {
        self.resolve_output
    }
}

#[derive(Default)]
pub(super) struct ResourcePlan {
    pub(super) samples: Vec<DrawSample>,
    pub(super) materials: Vec<MaterialRequest>,
}

pub(super) struct MaterialRequest {
    pub(super) material: custom::Material,
    pub(super) fallback: Inst,
    pub(super) receipt: usize,
}

pub(super) enum DrawSample {
    Image(ImagePaint),
    Glyph(GlyphRequest),
    Path(PathPaint, paths::Placed),
}

pub(super) struct GlyphRequest {
    pub(super) key: GlyphKey,
    pub(super) image: GlyphImage,
    pub(super) pos: [f32; 2],
    pub(super) color: u32,
    pub(super) clip: u32,
}

/// Builds a render graph without a GPU device or access to presentation history.
pub fn plan_frame(list: &DisplayList, target: TargetInfo) -> FramePlan {
    plan_frame_cached(list, target, None, &mut GlyphCache::default())
}

pub(super) fn plan_frame_cached(list: &DisplayList, target: TargetInfo, sample: Option<PaintSample>, glyphs: &mut GlyphCache) -> FramePlan {
    let scale = target.scale;
    let mut passes = vec![Pass::default()];
    let mut requests = PaintRequests::default();
    let mut resources = ResourcePlan::default();
    let mut open: Vec<usize> = vec![0];
    let mut clips = ClipStack::default();
    let mut glyph_rect_clips = FxHashMap::default();
    let [width, height] = target.extent;
    let window = Rect::new(0.0, 0.0, width as f32, height as f32);
    let items = list.items();
    let mut native_transitions = None;
    for (index, item) in items.iter().enumerate() {
        let clip_state = clips.current();
        let clip = clip_state.bounds.map_or(NO_CLIP, |c| [c.x, c.y, c.w, c.h]);
        let clip_head = clip_state.head;
        let current = *open.last().expect("a pass");
        match item {
            Item::Shadow(s) => passes[current].push(shadow(s, scale, clip, clip_head), Draw::PLAIN),
            Item::Quad(q) => passes[current].push(quad(q, scale, clip, clip_head), Draw::PLAIN),
            Item::Custom(paint) => {
                let fallback = quad(&paint.fallback, scale, clip, clip_head);
                if fallback.visible_bounds(&window).is_empty() {
                    continue;
                }
                let mut instance = custom_quad(paint, &fallback);
                let source = paint.fallback.source;
                let receipt = requests.receipts.len();
                requests
                    .receipts
                    .push(PaintReceipt { source, outcome: PaintOutcome::Fallback { reason: PaintFallback::UnsupportedProgram } });
                if let Some(&material) = paint.program.downcast_ref::<custom::Material>() {
                    let index = resources.materials.len();
                    resources.materials.push(MaterialRequest { material, fallback, receipt });
                    passes[current].push(instance, Draw::RequestedMaterial(index));
                    continue;
                } else if let Some(canvas) = paint.program.downcast_ref::<custom::Canvas>()
                    && canvas.mode != CanvasMode::Transition
                {
                    let max_dimension = target.max_texture_dimension;
                    let Some(entry) = CanvasPaint::from_custom(paint, fallback, canvas.mode, scale, sample, max_dimension) else {
                        requests.receipts[receipt].outcome = PaintOutcome::Fallback { reason: PaintFallback::InvalidSize };
                        passes[current].push(fallback, Draw::PLAIN);
                        continue;
                    };
                    instance.params.kind = KIND_CANVAS;
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
                            (canvas.mode == CanvasMode::Retained && CanvasCache::can_retain(entry.size)).then(|| paint.mount.downgrade());
                        requests.canvases.push(CanvasRequest { pass: current, paint: entry, fallback, receipt, retained_mount });
                        Draw::Canvas(index)
                    };
                    passes[current].push(instance, draw);
                    continue;
                }
                passes[current].push(fallback, Draw::PLAIN);
            }
            Item::Image(i) => {
                let frame = i.image.frame();
                if i.opacity <= 0.0 || frame.width == 0 || frame.height == 0 {
                    continue;
                }
                let instance = image(i, scale, clip, clip_head);
                let [x, y, w, h] = instance.border;
                let bounds = passes[current].visible_window(&window).intersect(&clip_envelope(Rect::new(x, y, w, h)));
                if instance.visible_bounds(&bounds).is_empty() {
                    continue;
                }
                let index = resources.samples.len();
                resources.samples.push(DrawSample::Image(i.clone()));
                passes[current].push(instance, Draw::Sample(index));
            }
            Item::Glyphs(run) => {
                let size = run.size * scale;
                let visible = passes[current].visible_window(&window);
                let visible = clip_state.bounds.map_or(visible, |rect| visible.intersect(&clip_envelope(rect)));
                if visible.is_empty() {
                    continue;
                }
                let clip_head = match clip_state {
                    ClipState { bounds: Some(rect), head: 0 } => {
                        let key = [rect.x, rect.y, rect.w, rect.h].map(f32::to_bits);
                        *glyph_rect_clips.entry(key).or_insert_with(|| clips.add_node(rect, [0.0; 4], 0))
                    }
                    _ => clip_head,
                };
                for (g, color) in run.placed_colored() {
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
                    let rect = Rect::new(x + glyph.left as f32, y - glyph.top as f32, glyph.width as f32, glyph.height as f32);
                    let bounds = rect.intersect(&visible);
                    if bounds.is_empty() {
                        continue;
                    }
                    let index = resources.samples.len();
                    resources.samples.push(DrawSample::Glyph(GlyphRequest {
                        key,
                        image: glyph.clone(),
                        pos: [rect.x, rect.y],
                        color,
                        clip: clip_head,
                    }));
                    passes[current].push_glyph_sample(index, bounds);
                }
            }
            Item::Path(p) => {
                let window = paths::window((width, height), clip_state.bounds.map(|c| (c.x, c.y, c.w, c.h)));
                let Some(at) = paths::place(p, scale, window) else { continue };

                let b = p.bounds;
                let (fill, grad_color, grad) = match p.gradient {
                    Some(g) => (straight(g.from.0), straight(g.to.0), [g.angle, g.from.1, g.to.1, 1.0]),
                    None => (straight(p.color), [0.0; 4], [0.0; 4]),
                };
                let index = resources.samples.len();
                resources.samples.push(DrawSample::Path(p.clone(), at));
                passes[current].push(
                    Inst {
                        rect: [at.x as f32, at.y as f32, at.width as f32, at.height as f32],
                        fill,
                        border: [b.x * scale, b.y * scale, b.w * scale, b.h * scale],
                        clip,
                        params: InstParams { kind: KIND_PATH, clip_head, ..InstParams::default() },
                        grad_color,
                        grad,
                        ..Inst::zeroed()
                    },
                    Draw::Sample(index),
                );
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
            Item::PopClip => clips.pop(),
            Item::PushLayer { opacity, .. } => {
                passes.push(Pass { opacity: *opacity, clip_limit: passes[current].clip_limit, ..Pass::default() });
                open.push(passes.len() - 1);
            }
            Item::SkipTransition { source, mount, revision } => {
                requests.skipped_transitions.push((mount.downgrade(), *revision));
                let outcome = PaintOutcome::Fallback { reason: PaintFallback::UnsupportedRegion };
                requests.receipts.push(PaintReceipt { source: *source, outcome });
            }
            Item::PushTransition(paint) => {
                let native = native_transitions.get_or_insert_with(|| transitions_with_native(&items)).contains(&index);
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
                passes.push(Pass { opacity: paint.opacity, clip_limit, transition, ..Pass::default() });
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
                let outer = passes[layer].transition.as_ref().and_then(|request| {
                    clips.leave_scope(request.clip_scope);
                    request.clip.bounds.map(|bounds| ([bounds.x, bounds.y, bounds.w, bounds.h], request.clip.head))
                });
                passes[parent].push(
                    Inst {
                        fill: [0.0, 0.0, 0.0, opacity],
                        clip: outer.map_or(NO_CLIP, |(clip, _)| clip),
                        params: InstParams {
                            kind: KIND_LAYER,
                            clip_head: outer.map_or(0, |(_, head)| head),
                            filled: u32::from(outer.is_some()),
                        },
                        ..Inst::zeroed()
                    },
                    Draw::Layer(layer),
                );
            }
        }
    }
    for index in open.into_iter().skip(1) {
        passes[index].transition = None;
    }
    let bounds = bound_passes(&mut passes, target.extent, |_, _, bounds, _| bounds);
    let (backdrop_inputs, resolve_output) = backdrop_inputs(&requests.backdrops, &bounds, target);
    FramePlan { passes, requests, clips, resources, bounds, resolve_output, backdrop_inputs }
}

pub(super) fn backdrop_inputs(backdrops: &[BackdropRequest], bounds: &[PassBounds], output: TargetInfo) -> (Vec<BackdropInput>, bool) {
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

pub(super) struct PlannedBackdrop {
    pub(super) request: BackdropRequest,
    pub(super) input: BackdropInput,
    pub(super) fallback: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Attachment {
    Output,
    Layer(usize),
}

pub(super) struct TargetPlan {
    pub(super) attachment: Attachment,
    pub(super) batches: Range<usize>,
}

pub(super) struct EncodedPlan {
    pub(super) targets: Vec<TargetPlan>,
    pub(super) batches: Vec<PlannedBatch>,
    pub(super) backdrops: Vec<PlannedBackdrop>,
    pub(super) resolve_output: bool,
}

impl EncodedPlan {
    pub(super) fn new(
        passes: &[Pass],
        instance_offsets: &[[u32; 2]],
        backdrops: Vec<BackdropRequest>,
        fallback_offset: u32,
        backdrop_inputs: Vec<BackdropInput>,
        resolve_output: bool,
    ) -> Self {
        let mut batches = Vec::new();
        let targets = passes
            .iter()
            .enumerate()
            .rev()
            .map(|(index, pass)| {
                let attachment = if index == 0 { Attachment::Output } else { Attachment::Layer(index) };
                TargetPlan { attachment, batches: pass.append_batches(instance_offsets[index], &mut batches) }
            })
            .collect();
        let backdrops = backdrops
            .into_iter()
            .zip(backdrop_inputs)
            .enumerate()
            .map(|(index, (request, input))| PlannedBackdrop { request, input, fallback: fallback_offset + index as u32 })
            .collect();
        Self { targets, batches, backdrops, resolve_output }
    }
}

/// Size the inner layers first, then use their composite bounds in the parent.
pub(super) fn bound_passes(
    passes: &mut [Pass],
    size: [u32; 2],
    mut transition_bounds: impl FnMut(usize, &TransitionRequest, PassBounds, Rect) -> PassBounds,
) -> Vec<PassBounds> {
    let window = Rect::new(0.0, 0.0, size[0] as f32, size[1] as f32);
    let mut bounds = vec![PassBounds::default(); passes.len()];
    let mut composites = FxHashMap::default();
    bounds[0].size = size;
    for (index, pass) in passes.iter_mut().enumerate().rev() {
        for batch in &pass.batches {
            if let Draw::Layer(layer) = batch.draw {
                pass.insts[batch.instances.start].rect = composites.get(&layer).copied().unwrap_or(bounds[layer]).rect();
            }
        }
        if index == 0 {
            break;
        }
        let visible = pass.visible_window(&window);
        let covered = pass.insts.iter().map(|inst| inst.visible_bounds(&visible)).fold(pass.glyph_bounds, union);
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

pub(super) fn transition_composite_bounds(
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

pub(super) fn union(a: Rect, b: Rect) -> Rect {
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
pub(super) fn transitions_with_native(items: &[Item]) -> FxHashSet<usize> {
    let mut blocked = FxHashSet::default();
    let mut layers = Vec::new();
    let mut native_count = 0;
    for (index, item) in items.iter().enumerate() {
        match item {
            Item::PushLayer { .. } => layers.push(None),
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
    use super::super::batch::KIND_SHADOW;
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
            program: custom::backdrop(|_| |_, _| {}),
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
            program: custom::transition(|_| |_, _| {}),
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
    fn frame_plan_orders_targets_and_resolves_backdrop_inputs() {
        let mut output = Pass::default();
        output.push(Inst::zeroed(), Draw::PLAIN);
        output.push(Inst::zeroed(), Draw::Layer(1));
        output.push(Inst::zeroed(), Draw::Backdrop(0));
        output.push(Inst::zeroed(), Draw::PLAIN);
        output.push(Inst::zeroed(), Draw::Backdrop(2));
        let mut layer = Pass::default();
        layer.push(Inst::zeroed(), Draw::PLAIN);
        layer.push(Inst::zeroed(), Draw::Backdrop(1));
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
                fallback: Inst { rect: [2.25, 3.25, 14.0, 10.0], clip: [4.5, 5.25, 20.0, 15.0], ..Inst::zeroed() },
                receipt: 0,
                mount: Weak::new(),
                exiting,
                target,
            }))
        };
        let passes = [output, layer];
        let plan = |passes, bounds, offsets, requests: Vec<BackdropRequest>, fallback, output| {
            let (inputs, resolve) = backdrop_inputs(&requests, bounds, output);
            EncodedPlan::new(passes, offsets, requests, fallback, inputs, resolve)
        };
        let attachment = TargetInfo { extent: [12, 10], scale: 1.0, max_texture_dimension: 1024, copyable: true, encode_output: false };
        let main = plan(&passes, &bounds, &[[0, 0], [5, 0]], requests(), 7, attachment);
        assert_eq!(main.targets.iter().map(|target| target.attachment).collect::<Vec<_>>(), [Attachment::Layer(1), Attachment::Output]);
        assert_eq!(main.targets[0].batches, 0..2);
        assert_eq!(main.targets[1].batches, 2..7);
        assert_eq!(
            main.batches,
            [
                PlannedBatch { draw: Draw::PLAIN, instances: 5..6 },
                PlannedBatch { draw: Draw::Backdrop(1), instances: 6..7 },
                PlannedBatch { draw: Draw::PLAIN, instances: 0..1 },
                PlannedBatch { draw: Draw::Layer(1), instances: 1..2 },
                PlannedBatch { draw: Draw::Backdrop(0), instances: 2..3 },
                PlannedBatch { draw: Draw::PLAIN, instances: 3..4 },
                PlannedBatch { draw: Draw::Backdrop(2), instances: 4..5 },
            ],
        );
        assert_eq!(main.backdrops[0].input, BackdropInput::Prefix(BackdropRegion { origin: [4, 5], size: [8, 5], offset: [2, 2] }));
        assert_eq!(main.backdrops[1].input, BackdropInput::Prefix(BackdropRegion { origin: [0, 0], size: [5, 4], offset: [3, 3] }));
        assert_eq!(main.backdrops[2].input, BackdropInput::Accepted);
        assert_eq!(main.backdrops[2].fallback, 9);
        assert!(!main.resolve_output);
        let render_only = TargetInfo { copyable: false, ..attachment };
        let no_copy = plan(&passes, &bounds, &[[0, 0], [5, 0]], requests(), 7, render_only);
        assert!(no_copy.resolve_output);
        assert_eq!(no_copy.backdrops[0].input, main.backdrops[0].input);
        assert_eq!(no_copy.backdrops[1].input, main.backdrops[1].input);
        assert_eq!(no_copy.backdrops[2].input, BackdropInput::Accepted);

        let mut clipped = requests();
        clipped[0].fallback.clip = [20.0, 20.0, 5.0, 4.0];
        let local = plan(&passes, &bounds, &[[0, 0], [5, 0]], clipped, 7, render_only);
        assert!(!local.resolve_output, "a clipped root read, a layer read, and an accepted image require no output resolve");
        assert_eq!(local.backdrops[0].input, BackdropInput::Unavailable);
        assert_eq!(local.backdrops[1].input, main.backdrops[1].input);

        let mut plain = Pass::default();
        for _ in 0..4 {
            plain.push(Inst::zeroed(), Draw::PLAIN);
        }
        let direct = plan(&[plain], &bounds[..1], &[[0, 0]], Vec::new(), 4, render_only);
        assert!(!direct.resolve_output);
        assert_eq!(direct.targets.len(), 1);
        assert_eq!(direct.targets[0].attachment, Attachment::Output);
        assert_eq!(direct.batches, [PlannedBatch { draw: Draw::PLAIN, instances: 0..4 }]);
    }

    #[test]
    fn nested_layers_bound_their_painted_pixels_instead_of_the_window() {
        let window = [1600, 1200];
        let composite = Inst { params: InstParams { kind: KIND_LAYER, ..InstParams::default() }, clip: NO_CLIP, ..Inst::zeroed() };
        let quad = Inst { rect: [370.5, 460.25, 28.0, 18.0], clip: NO_CLIP, ..Inst::zeroed() };
        let shadow = Inst {
            rect: [400.25, 480.5, 21.0, 12.0],
            clip: [404.0, 480.0, 10.0, 10.0],
            border: [2.0, 0.0, 0.0, 0.0],
            params: InstParams { kind: KIND_SHADOW, ..InstParams::default() },
            ..Inst::zeroed()
        };
        let mut passes = [Pass::default(), Pass::default(), Pass::default()];
        passes[0].push(composite, Draw::Layer(1));
        passes[1].push(quad, Draw::PLAIN);
        passes[1].push(composite, Draw::Layer(2));
        passes[2].push(shadow, Draw::PLAIN);
        let bounds = bound_passes(&mut passes, window, |_, _, _, _| unreachable!("no transition regions"));
        assert_eq!(bounds[0].size, window);
        assert_eq!(bounds[1].rect(), [369.0, 459.0, 46.0, 32.0]);
        assert_eq!(bounds[2].rect(), [403.0, 479.0, 12.0, 12.0]);
        assert_eq!(passes[0].insts[0].rect, bounds[1].rect());
        assert_eq!(passes[1].insts[1].rect, bounds[2].rect());
        assert_eq!(bounds[1].allocation(window), [64, 64]);
        assert_eq!(bounds[2].allocation(window), [64, 64]);
    }
}
