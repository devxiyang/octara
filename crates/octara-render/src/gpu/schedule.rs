//! Opaque core extraction and ordering for the four micro-pipelines.

use super::batch::{Draw, NO_CLIP, Paint, Pass, PlannedBatch, Shape, grow, rect, xywh};
use super::instances::{DrawInst, Instances};
use super::plan::DrawSegment;
use crate::shape::{ClipStack, inner_rect};
use octara_model::geometry::Rect;
use rustc_hash::FxHashMap;
use std::ops::Range;

const FRACTIONAL_CLIP: u32 = 1;
// Stay comfortably within the depth precision and keep numeric order even on large lists.
const MAX_DEPTH_STEPS: usize = 1 << 20;

fn safe_box(rect: Rect, radius: [f32; 4]) -> Rect {
    grow(rect, -radius.into_iter().fold(0.5, f32::max))
}

/// Return a rectangle on pixel boundaries covering exactly the fully covered centers.
fn core_pixels(r: Rect) -> Rect {
    if r.w < 0.0 || r.h < 0.0 {
        return Rect::default();
    }
    let x = (r.x - 0.5).ceil();
    let y = (r.y - 0.5).ceil();
    Rect::new(x, y, ((r.right() - 0.5).floor() + 1.0 - x).max(0.0), ((r.bottom() - 0.5).floor() + 1.0 - y).max(0.0))
}

fn raster_pixels(r: Rect, open_edge: bool) -> Rect {
    if r.is_empty() {
        return Rect::default();
    }
    let first = |edge: f32| if open_edge { (edge - 0.5).floor() + 1.0 } else { (edge - 0.5).ceil() };
    let x = first(r.x);
    let y = first(r.y);
    Rect::new(x, y, (r.right() - 0.5).ceil() - x, (r.bottom() - 0.5).ceil() - y)
}

fn opaque_core(paint: Paint, draw: Draw, clips: &ClipStack, target: Rect) -> Rect {
    let shape = rect(paint.rect);
    let base = match paint.shape {
        Shape::Quad { fill, gradient, .. }
            if draw == Draw::Quad
                && gradient.map_or_else(
                    || fill.is_some_and(|c| c.components[3] == 1.0),
                    |g| g.from.0.components[3] == 1.0 && g.to.0.components[3] == 1.0,
                ) =>
        {
            core_pixels(safe_box(shape, paint.radius))
        }
        Shape::Image { bounds, opacity: 1.0, region: Some(region) } if region.opaque => {
            core_pixels(safe_box(shape, [0.0; 4])).intersect(&core_pixels(safe_box(rect(bounds), paint.radius)))
        }
        _ => return Rect::default(),
    };
    // A rounded or fractional clip can safely occlude only where its coverage is one.
    let clip = if paint.clip == NO_CLIP { target } else { clips.safe_box(paint.clip_head, safe_box(rect(paint.clip), [0.0; 4])) };
    if clip.is_empty() {
        return Rect::default();
    }
    base.intersect(&core_pixels(clip)).intersect(&target)
}

fn quad_hole(paint: Paint, core: Rect) -> Rect {
    let Shape::Quad { border_widths, border_color, .. } = paint.shape else { return core };
    if border_color.components[3] == 0.0 || border_widths == [0.0; 4] {
        return core;
    }
    let [x, y, w, h] = paint.rect;
    let (inner, radius) = inner_rect(x, y, w, h, paint.radius, border_widths);
    if inner.is_empty() {
        return Rect::default();
    }
    core.intersect(&core_pixels(safe_box(inner, radius)))
}

/// Disjoint top, bottom, left and right strips. The unsplit shape uses the first slot.
fn hollow_frame(bounds: Rect, hole: Rect) -> [Rect; 4] {
    let hole = bounds.intersect(&hole);
    let Rect { x, y, w, h } = bounds;
    if hole.is_empty() {
        return [bounds, Rect::default(), Rect::default(), Rect::default()];
    }
    [
        Rect::new(x, y, w, hole.y - y),
        Rect::new(x, hole.bottom(), w, y + h - hole.bottom()),
        Rect::new(x, hole.y, hole.x - x, hole.h),
        Rect::new(hole.right(), hole.y, x + w - hole.right(), hole.h),
    ]
}

struct Geometry {
    bounds: Rect,
    vertex_bounds: Rect,
    clip: Rect,
    head: u32,
    flags: u32,
}

fn geometry(paint: Paint, clips: &ClipStack, target: Rect) -> Geometry {
    let shape = rect(paint.rect);
    let shadow_tail = matches!(paint.shape, Shape::Shadow { sigma, .. } if sigma > 0.0);
    let aa = paint.padding(0.5) != 0.0 && !shadow_tail;
    let mut bounds = if shadow_tail {
        // Match the reference's floor/ceil envelope around the truncated Gaussian.
        let outer = grow(shape, paint.padding(1.0));
        Rect::new(outer.x.floor(), outer.y.floor(), outer.right().ceil() - outer.x.floor(), outer.bottom().ceil() - outer.y.floor())
    } else {
        grow(shape, paint.padding(0.5))
    };
    if let Shape::Image { bounds: image, .. } = paint.shape {
        bounds = bounds.intersect(&grow(rect(image), 0.5));
    }
    let authored = rect(paint.clip);
    let mut head = paint.clip_head;
    let mut flags = 0;
    let mut clip = authored;
    if head != 0 {
        let safe = clips.safe_box(head, safe_box(authored, [0.0; 4]));
        if !safe.is_empty() && bounds.x > safe.x && bounds.y > safe.y && bounds.right() < safe.right() && bounds.bottom() < safe.bottom() {
            head = 0;
            clip = rect(NO_CLIP);
        } else {
            clip = grow(authored, 0.5);
        }
    } else if paint.clip != NO_CLIP && [authored.x, authored.y, authored.right(), authored.bottom()].iter().any(|x| x.fract() != 0.0) {
        flags = FRACTIONAL_CLIP;
    }
    let envelope = if flags == FRACTIONAL_CLIP { grow(clip, 0.5) } else { clip };
    let vertex_bounds = if authored.is_empty() || (matches!(paint.shape, Shape::Image { .. }) && shape.is_empty()) {
        Rect::default()
    } else {
        raster_pixels(bounds, aa).intersect(&target)
    };
    let bounds = vertex_bounds.intersect(&raster_pixels(envelope, flags == FRACTIONAL_CLIP));
    Geometry { bounds, vertex_bounds, clip, head, flags }
}

#[derive(Clone, Copy)]
struct Entry {
    key: u64,
    draw: Draw,
    inst: DrawInst,
}

fn radix_sort(entries: &mut [Entry]) {
    if entries.len() < 2 || entries.windows(2).all(|w| w[0].key <= w[1].key) {
        return;
    }
    let mut scratch = vec![entries[0]; entries.len()];
    let differing = entries.iter().fold(0, |bits, e| bits | (e.key ^ entries[0].key));
    for shift in (0..64).step_by(8) {
        if (differing >> shift) & 255 == 0 {
            continue;
        }
        let byte = |entry: &Entry| ((entry.key >> shift) & 255) as usize;
        let mut counts = [0; 256];
        for entry in entries.iter() {
            counts[byte(entry)] += 1;
        }
        let mut end = 0;
        for count in &mut counts {
            let len = *count;
            *count = end;
            end += len;
        }
        for entry in entries.iter() {
            let next = &mut counts[byte(entry)];
            scratch[*next] = *entry;
            *next += 1;
        }
        entries.copy_from_slice(&scratch);
    }
}

#[derive(Clone, Copy)]
struct Coverage {
    bounds: Rect,
    state: u32,
    rank: u32,
}

#[derive(Default)]
struct Tile {
    recent: Vec<Coverage>,
    // In dense tiles collapse older rectangles into conservative rank bounds.
    older: Option<(u32, u32)>,
}

impl Tile {
    fn rank(&self, bounds: Rect, state: u32) -> u32 {
        let older = self.older.map_or(0, |(rank, prior)| rank + u32::from(prior != state));
        self.recent
            .iter()
            .filter(|e| !bounds.intersect(&e.bounds).is_empty())
            .fold(older, |rank, e| rank.max(e.rank + u32::from(e.state != state)))
    }

    fn push(&mut self, coverage: Coverage) {
        if self.recent.len() == 16 {
            let state = self.recent[0].state;
            let (max_rank, mut same) = self.older.map_or((0, true), |(rank, prior)| (rank, prior == state));
            let rank = self.recent.iter().fold(max_rank, |rank, e| {
                same &= e.state == state;
                rank.max(e.rank)
            });
            self.older = Some((rank, if same { state } else { u32::MAX }));
            self.recent.clear();
        }
        self.recent.push(coverage);
    }
}

struct DrawOrder {
    edge: f32,
    states: FxHashMap<Draw, u32>,
    tiles: FxHashMap<(u32, u32), Tile>,
}

impl DrawOrder {
    fn new(target: Rect) -> Self {
        Self { edge: (target.w.max(target.h) / 32.0).ceil().max(32.0), states: FxHashMap::default(), tiles: FxHashMap::default() }
    }

    fn key(&mut self, draw: Draw, bounds: Rect) -> u64 {
        let next_state = self.states.len() as u32;
        let state = *self.states.entry(draw).or_insert(next_state);
        let tile = |v: f32| (v / self.edge).floor() as u32;
        let x = tile(bounds.x)..=tile(bounds.right() - 1.0);
        let y = tile(bounds.y)..=tile(bounds.bottom() - 1.0);
        let coords = || y.clone().flat_map(|y| x.clone().map(move |x| (x, y)));
        let rank = coords().filter_map(|xy| self.tiles.get(&xy)).fold(0, |rank, tile| rank.max(tile.rank(bounds, state)));
        for xy in coords() {
            self.tiles.entry(xy).or_default().push(Coverage { bounds, state, rank });
        }
        u64::from(rank) << 32 | u64::from(state)
    }
}

fn append_batches(
    entries: impl IntoIterator<Item = Entry>,
    paints: &[Paint],
    instances: &mut Instances,
    batches: &mut Vec<PlannedBatch>,
) -> Range<usize> {
    let start = batches.len();
    for entry in entries {
        let index = if matches!(entry.draw, Draw::Glyph(_)) {
            let Shape::Glyph(Some(glyph)) = paints[entry.inst.payload as usize].shape else { unreachable!("resolved glyph") };
            instances.push_glyph(glyph, entry.inst)
        } else {
            instances.push_draw(entry.inst)
        };
        if let Some(last) = batches[start..].last_mut()
            && last.draw == entry.draw
        {
            last.instances.end = index + 1;
        } else {
            batches.push(PlannedBatch { draw: entry.draw, instances: index..index + 1 });
        }
    }
    start..batches.len()
}

pub(super) fn schedule_pass(
    pass: &Pass,
    target: Rect,
    clips: &ClipStack,
    instances: &mut Instances,
    batches: &mut Vec<PlannedBatch>,
) -> Vec<DrawSegment> {
    let draws: Vec<_> = pass.batches.iter().flat_map(|batch| batch.instances.clone().map(|_| batch.draw)).collect();
    let mut segments = Vec::new();
    let mut start = 0;
    while start < draws.len() {
        let limit = (start + MAX_DEPTH_STEPS).min(draws.len());
        let end = draws[start..limit].iter().position(|draw| matches!(draw, Draw::Backdrop(_))).map_or(limit, |pos| start + pos + 1);
        let segment = Segment { paints: &pass.paints[start..end], draws: &draws[start..end], target, clips };
        segments.push(segment.schedule(instances, batches));
        start = end;
    }
    segments
}

pub(super) fn fallback_draw(paint: Paint, target: Rect, depth: f32, clips: &ClipStack, instances: &mut Instances) -> u32 {
    let Geometry { vertex_bounds, clip, head, flags, .. } = geometry(paint, clips, target);
    let payload = instances.push_paint(paint, head);
    instances.push_draw(DrawInst { bounds: xywh(vertex_bounds), clip: xywh(clip), core: [0.0; 4], depth, payload, clip_head: head, flags })
}

struct Segment<'a> {
    paints: &'a [Paint],
    draws: &'a [Draw],
    target: Rect,
    clips: &'a ClipStack,
}

impl Segment<'_> {
    fn schedule(self, instances: &mut Instances, batches: &mut Vec<PlannedBatch>) -> DrawSegment {
        let mut cores = Vec::new();
        let mut blended = Vec::new();
        let mut backdrop = None;
        let mut order = DrawOrder::new(self.target);
        let cores_by_paint: Vec<_> =
            self.paints.iter().zip(self.draws).map(|(&paint, &draw)| opaque_core(paint, draw, self.clips, self.target)).collect();
        for (index, (&paint, &draw)) in self.paints.iter().zip(self.draws).enumerate() {
            let Geometry { bounds, vertex_bounds, clip, head, flags } = geometry(paint, self.clips, self.target);
            // A backdrop must preserve its receipt even if it has no visible pixels.
            if bounds.is_empty() && !matches!(draw, Draw::Backdrop(_)) {
                continue;
            }
            let depth = (index + 1) as f32 / (self.paints.len() + 1) as f32;
            let core = cores_by_paint[index];
            let inst = DrawInst {
                bounds: xywh(vertex_bounds),
                clip: xywh(clip),
                core: xywh(core),
                depth,
                payload: if matches!(draw, Draw::Glyph(_)) { index as u32 } else { instances.push_paint(paint, head) },
                clip_head: head,
                flags,
            };
            let entry = Entry { key: 0, draw, inst };
            if matches!(draw, Draw::Backdrop(_)) {
                backdrop = Some(entry);
                continue;
            }
            if !core.is_empty() {
                cores.push(entry);
            }
            let hole = match draw {
                Draw::Quad if !core.is_empty() => quad_hole(paint, core),
                Draw::Shadow => cores_by_paint.get(index + 1).copied().unwrap_or_default(),
                _ => core,
            };
            for strip in hollow_frame(bounds, hole).into_iter().filter(|r| !r.is_empty()) {
                let key = order.key(draw, strip);
                // A full draw carries its unclipped geometry. Ordered strip rectangles
                // expose that same draw's coverage to the painter-order scheduler.
                let geometry = if strip == bounds { vertex_bounds } else { strip };
                blended.push(Entry { key, inst: DrawInst { bounds: xywh(geometry), ..inst }, ..entry });
            }
        }
        let cores = append_batches(cores.into_iter().rev(), self.paints, instances, batches);
        radix_sort(&mut blended);
        let blended = append_batches(blended.into_iter().chain(backdrop), self.paints, instances, batches);
        DrawSegment { cores, blended }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_front_to_back_core_pass_leaves_four_disjoint_shadow_strips() {
        use octara_model::theme::Color;

        let target = Rect::new(0.0, 0.0, 500.0, 320.0);
        let mut pass = Pass::default();
        let card = Paint {
            rect: xywh(target),
            radius: [16.0; 4],
            shape: Shape::Quad { fill: Some(Color::WHITE), gradient: None, border_widths: [0.0; 4], border_color: Color::TRANSPARENT },
            ..Paint::default()
        };
        pass.push(Paint { shape: Shape::Shadow { color: Color::BLACK, sigma: 4.0 }, ..card }, Draw::Shadow);
        pass.push(card, Draw::Quad);
        pass.push(Paint { rect: [120.0, 80.0, 80.0, 80.0], ..card }, Draw::Quad);
        let mut instances = Instances::default();
        let mut batches = Vec::new();
        let segments = schedule_pass(&pass, target, &ClipStack::default(), &mut instances, &mut batches);
        let [segment] = &segments[..] else { panic!("one depth segment") };
        let [core] = &batches[segment.cores.clone()] else { panic!("one opaque quad batch") };
        let core_draws = &instances.draws[core.instances.start as usize..core.instances.end as usize];
        assert_eq!(core_draws.len(), 2);
        assert!(core_draws[0].depth > core_draws[1].depth);
        let hole = rect(core_draws[1].core);
        assert_eq!(hole, Rect::new(16.0, 16.0, 468.0, 288.0));

        let strips: Vec<_> = batches[segment.blended.clone()]
            .iter()
            .filter(|batch| batch.draw == Draw::Shadow)
            .flat_map(|batch| batch.instances.clone())
            .map(|id| rect(instances.draws[id as usize].bounds))
            .collect();
        assert_eq!(strips.len(), 4);
        let full = target.w * target.h;
        let area: f32 = strips.iter().map(|r| r.w * r.h).sum();
        assert_eq!(area, full - hole.w * hole.h);
        assert!(area < full * 0.2);
        for (i, a) in strips.iter().enumerate() {
            assert!(a.intersect(&hole).is_empty());
            assert!(strips[i + 1..].iter().all(|b| a.intersect(b).is_empty()));
        }
    }

    #[test]
    fn clip_fast_paths_keep_the_vertex_clamp_and_elide_safe_rounded_chains() {
        let target = Rect::new(0.0, 0.0, 100.0, 100.0);
        let mut clips = ClipStack::default();
        clips.push(Rect::new(5.0, 5.0, 60.0, 60.0), [0.0; 4], 1.0);
        clips.push(Rect::new(10.0, 0.0, 40.0, 80.0), [0.0; 4], 1.0);

        let state = clips.current();
        let paint = Paint { rect: [6.0, 8.0, 64.0, 64.0], clip: xywh(state.bounds.unwrap()), clip_head: state.head, ..Paint::default() };
        let clip = geometry(paint, &clips, target);
        assert_eq!((clip.head, clip.flags), (0, 0));
        assert_eq!(clip.vertex_bounds, Rect::new(6.0, 8.0, 64.0, 64.0));
        assert_eq!(clip.bounds, Rect::new(10.0, 8.0, 40.0, 57.0));

        clips.push(Rect::new(8.0, 6.0, 41.0, 50.0), [10.0; 4], 1.0);
        let state = clips.current();
        let paint = Paint { clip: xywh(state.bounds.unwrap()), clip_head: state.head, ..paint };
        assert_ne!(geometry(paint, &clips, target).head, 0);
        let inside = geometry(Paint { rect: [25.0, 21.0, 10.0, 11.0], ..paint }, &clips, target);
        assert_eq!((inside.head, inside.flags), (0, 0));
        assert_eq!(inside.bounds, inside.vertex_bounds);
    }

    #[test]
    fn radix_sort_is_stable_across_all_key_bytes() {
        let mut entries =
            [1 << 56, 7, 1 << 32, 7, 0, 1 << 56, 1 << 40].map(|key| Entry { key, draw: Draw::Quad, inst: bytemuck::Zeroable::zeroed() });
        for (i, entry) in entries.iter_mut().enumerate() {
            entry.inst.payload = i as u32;
        }
        radix_sort(&mut entries);
        assert_eq!(entries.map(|entry| entry.inst.payload), [4, 1, 3, 2, 6, 0, 5]);
    }

    #[test]
    fn sort_keys_group_disjoint_draws_and_preserve_overlapping_transparency() {
        let mut order = DrawOrder::new(Rect::new(0.0, 0.0, 80.0, 80.0));
        let a = order.key(Draw::Quad, Rect::new(1.0, 2.0, 5.0, 5.0));
        let b = order.key(Draw::Shadow, Rect::new(4.0, 2.0, 8.0, 5.0));
        let c = order.key(Draw::Quad, Rect::new(16.0, 2.0, 5.0, 5.0));
        let d = order.key(Draw::Quad, Rect::new(7.0, 2.0, 5.0, 5.0));
        assert_eq!(a, c);
        assert!(a < b && b < d);
    }
}
