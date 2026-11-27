//! Flat, paint-ordered geometry for viewport culling and native hit tests.

use crate::{LayoutEngine, TextEngine, frame_arena::FrameArena, paint_order};
use octara_model::geometry::{Point, Rect};
use octara_tree::{DirtyBitmap, NodeFlags, NodeId, NodeKind, Tree, TreeDelta};
use rustc_hash::FxHashMap;
use slotmap::SecondaryMap;
use std::sync::Arc;

const WIDTH: usize = 4;
const LINK: u16 = 1;
const OVERLAY_ROOT: u16 = 2;
const CELL_SIZE: f32 = 128.0;
const MAX_CELLS: i64 = 16;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct FlatNode {
    pub node: NodeId,
    pub subtree_end: u32,
    pub depth: u16,
    pub flags: u16,
}

const _: () = assert!(std::mem::size_of::<FlatNode>() == 16);

#[derive(Default)]
struct Topology {
    nodes: Vec<FlatNode>,
    indices: SecondaryMap<NodeId, u32>,
    z_indices: Vec<i32>,
}

impl Topology {
    fn rebuild(&self, tree: &Tree, delta: &TreeDelta) -> Self {
        let mut branches = DirtyBitmap::default();
        for node in delta.topology_dirty.iter().chain(self.reordered_parents(tree, delta)) {
            let mut next = Some(node);
            while let Some(node) = next.filter(|&node| branches.insert(node)) {
                next = tree.physical_parent(node);
            }
        }
        let mut result = Self { nodes: Vec::with_capacity(self.nodes.len() + delta.created.len()), ..Self::default() };
        let mut pending = vec![(tree.root(), 0usize)];
        let mut open: Vec<usize> = Vec::new();
        while let Some((node, depth)) = pending.pop() {
            let index = flat_index(result.nodes.len());
            while open.len() > depth {
                result.nodes[open.pop().expect("open subtree")].subtree_end = index;
            }
            if !branches.contains(&node)
                && let Some(&start) = self.indices.get(node)
            {
                result.copy_subtree(self, start, index, depth);
                result.z_indices[index as usize] = tree.stacking_key(node).1;
                continue;
            }
            open.push(index as usize);
            let flags = if node == tree.overlay_root() {
                OVERLAY_ROOT
            } else if tree.node(node).is_some_and(|core| core.kind == NodeKind::Link) {
                LINK
            } else {
                0
            };
            result.nodes.push(FlatNode { node, subtree_end: 0, depth: depth.min(u16::MAX as usize) as u16, flags });
            result.indices.insert(node, index);
            result.z_indices.push(tree.stacking_key(node).1);
            pending.extend(paint_order(tree, node).iter().rev().map(|&child| (child, depth + 1)));
        }
        let end = flat_index(result.nodes.len());
        for index in open {
            result.nodes[index].subtree_end = end;
        }
        result
    }

    fn copy_subtree(&mut self, previous: &Self, start: u32, destination: u32, depth: usize) {
        let root = previous.nodes[start as usize];
        let range = start as usize..root.subtree_end as usize;
        for (&entry, &z) in previous.nodes[range.clone()].iter().zip(&previous.z_indices[range]) {
            let depth = depth + usize::from(entry.depth - root.depth);
            self.indices.insert(entry.node, flat_index(self.nodes.len()));
            self.nodes.push(FlatNode {
                subtree_end: flat_index(destination as usize + (entry.subtree_end - start) as usize),
                depth: depth.min(u16::MAX as usize) as u16,
                ..entry
            });
            self.z_indices.push(z);
        }
    }

    fn reordered_parents<'a>(&'a self, tree: &'a Tree, delta: &'a TreeDelta) -> impl Iterator<Item = NodeId> + 'a {
        delta.paint_dirty.iter().filter_map(move |node| {
            let index = *self.indices.get(node)? as usize;
            if self.z_indices[index] != tree.stacking_key(node).1 { tree.physical_parent(node) } else { None }
        })
    }

    fn changed(&self, tree: &Tree, delta: &TreeDelta) -> bool {
        self.nodes.first().is_none_or(|root| root.node != tree.root())
            || !delta.topology_dirty.is_empty()
            || self.reordered_parents(tree, delta).next().is_some()
    }
}

#[inline]
fn flat_index(index: usize) -> u32 {
    u32::try_from(index).unwrap_or_else(|_| index_exhausted())
}

#[cold]
#[inline(never)]
fn index_exhausted() -> ! {
    panic!("flat tree exceeds the slot index range")
}

#[repr(C, align(16))]
#[derive(Clone, Copy, Default)]
struct Lanes([f32; WIDTH]);

#[derive(Default)]
struct Bounds {
    x: Vec<Lanes>,
    y: Vec<Lanes>,
    w: Vec<Lanes>,
    h: Vec<Lanes>,
}

impl Bounds {
    fn reset(&mut self, len: usize) {
        let blocks = len.div_ceil(WIDTH);
        for axis in [&mut self.x, &mut self.y, &mut self.w, &mut self.h] {
            axis.resize(blocks, Lanes::default());
            axis.fill(Lanes::default());
        }
    }

    fn get(&self, index: u32) -> Rect {
        let (block, lane) = (index as usize / WIDTH, index as usize % WIDTH);
        Rect::new(self.x[block].0[lane], self.y[block].0[lane], self.w[block].0[lane], self.h[block].0[lane])
    }

    fn store_batch(&mut self, batch: ClipBatch, flat: &[FlatNode], visible: &mut FrameArena<u32>) {
        let block = batch.block;
        let [x, y, w, h] = batch.clipped();
        [self.x[block], self.y[block], self.w[block], self.h[block]] = [x, y, w, h];
        let start = block * WIDTH;
        for (offset, entry) in flat[start..(start + WIDTH).min(flat.len())].iter().enumerate() {
            if entry.flags & OVERLAY_ROOT == 0 && w.0[offset] > 0.0 && h.0[offset] > 0.0 {
                visible.push(flat_index(start + offset));
            }
        }
    }
}

type Cell = (i32, i32);

#[derive(Default)]
struct Grid {
    cells: FxHashMap<Cell, Vec<u32>>,
    large: Vec<u32>,
}

impl Grid {
    fn insert(&mut self, index: u32, rect: Rect) {
        if rect.is_empty() {
            return;
        }
        let (x, y) = cell(Point::new(rect.x, rect.y));
        let (right, bottom) = cell(Point::new(rect.right(), rect.bottom()));
        let columns = i64::from(right) - i64::from(x) + 1;
        let rows = i64::from(bottom) - i64::from(y) + 1;
        if columns.saturating_mul(rows) > MAX_CELLS {
            self.large.push(index);
        } else {
            for row in y..=bottom {
                for column in x..=right {
                    self.cells.entry((column, row)).or_default().push(index);
                }
            }
        }
    }

    fn clear(&mut self) {
        self.large.clear();
        for candidates in self.cells.values_mut() {
            candidates.clear();
        }
    }
}

fn cell(point: Point) -> Cell {
    ((point.x / CELL_SIZE).floor() as i32, (point.y / CELL_SIZE).floor() as i32)
}

#[derive(Clone, Copy)]
struct ClipBatch {
    block: usize,
    rects: [Rect; WIDTH],
    clips: [Rect; WIDTH],
}

impl ClipBatch {
    fn new(block: usize) -> Self {
        Self { block, rects: [Rect::default(); WIDTH], clips: [Rect::default(); WIDTH] }
    }

    fn clipped(self) -> [Lanes; 4] {
        clip_four(edges(self.rects), edges(self.clips))
    }
}

fn edges(rects: [Rect; WIDTH]) -> [Lanes; 4] {
    [lanes(rects, |r| r.x), lanes(rects, |r| r.y), lanes(rects, |r| r.right()), lanes(rects, |r| r.bottom())]
}

fn lanes(rects: [Rect; WIDTH], get: impl Fn(Rect) -> f32) -> Lanes {
    Lanes(rects.map(get))
}

#[cfg(target_arch = "aarch64")]
#[inline]
fn clip_four([x, y, r, b]: [Lanes; 4], [cx, cy, cr, cb]: [Lanes; 4]) -> [Lanes; 4] {
    use std::arch::aarch64::*;
    let mut result = [Lanes::default(); 4];
    // AArch64 guarantees NEON. Each load and store spans one aligned four-lane array.
    unsafe {
        let load = |value: &Lanes| vld1q_f32(value.0.as_ptr());
        let left = vmaxnmq_f32(load(&x), load(&cx));
        let top = vmaxnmq_f32(load(&y), load(&cy));
        let zero = vdupq_n_f32(0.0);
        let width = vmaxnmq_f32(zero, vsubq_f32(vminnmq_f32(load(&r), load(&cr)), left));
        let height = vmaxnmq_f32(zero, vsubq_f32(vminnmq_f32(load(&b), load(&cb)), top));
        for (output, value) in result.iter_mut().zip([left, top, width, height]) {
            vst1q_f32(output.0.as_mut_ptr(), value);
        }
    }
    result
}

#[cfg(not(target_arch = "aarch64"))]
#[inline]
fn clip_four([x, y, r, b]: [Lanes; 4], [cx, cy, cr, cb]: [Lanes; 4]) -> [Lanes; 4] {
    let left = std::array::from_fn(|i| x.0[i].max(cx.0[i]));
    let top = std::array::from_fn(|i| y.0[i].max(cy.0[i]));
    let width = std::array::from_fn(|i| (r.0[i].min(cr.0[i]) - left[i]).max(0.0));
    let height = std::array::from_fn(|i| (b.0[i].min(cb.0[i]) - top[i]).max(0.0));
    [Lanes(left), Lanes(top), Lanes(width), Lanes(height)]
}

#[derive(Default)]
struct FlatFrame {
    topology: Arc<Topology>,
    bounds: Bounds,
    grid: Grid,
}

impl FlatFrame {
    fn rebuild_bounds(&mut self, layout: &LayoutEngine, scratch: &mut FrameArena<u32>) {
        let flat = &self.topology.nodes;
        self.bounds.reset(flat.len());
        self.grid.clear();
        let mut index = 0;
        let mut batch = ClipBatch::new(0);
        while let Some(entry) = flat.get(index) {
            if let Some((bounds, clip)) = layout.bounds(entry.node).zip(layout.clip(entry.node)).filter(|(_, clip)| !clip.is_empty()) {
                let block = index / WIDTH;
                if block != batch.block {
                    self.bounds.store_batch(batch, flat, scratch);
                    batch = ClipBatch::new(block);
                }
                batch.rects[index % WIDTH] = bounds;
                batch.clips[index % WIDTH] = clip;
                let clips_subtree =
                    index != 0 && entry.flags & OVERLAY_ROOT == 0 && layout.child_clip(entry.node).is_some_and(|clip| clip.is_empty());
                index = if clips_subtree { entry.subtree_end as usize } else { index + 1 };
            } else if entry.flags & OVERLAY_ROOT != 0 {
                // Overlay children use the viewport clip, even when their parent's clip is empty.
                index += 1;
            } else {
                index = entry.subtree_end as usize;
            }
        }
        self.bounds.store_batch(batch, flat, scratch);
        for &index in scratch.used().iter() {
            self.grid.insert(index, self.bounds.get(index));
        }
        self.grid.cells.retain(|_, candidates| !candidates.is_empty());
        scratch.finish();
    }

    fn hit(&self, tree: &Tree, layout: &LayoutEngine, text: &TextEngine, pos: Point) -> Option<NodeId> {
        let candidates = self.grid.cells.get(&cell(pos)).map_or(&[][..], Vec::as_slice);
        let hit = |candidates: &[u32]| {
            candidates.rchunks(WIDTH).find_map(|chunk| {
                let mut rects = [Rect::default(); WIDTH];
                for (rect, &index) in rects.iter_mut().zip(chunk) {
                    *rect = self.bounds.get(index);
                }
                let mut mask = point_mask(rects, pos) & ((1 << chunk.len()) - 1);
                while mask != 0 {
                    let lane = (u8::BITS - 1 - mask.leading_zeros()) as usize;
                    let index = chunk[lane];
                    let entry = self.topology.nodes[index as usize];
                    if exact_hit(tree, layout, text, entry, pos) {
                        return Some(index);
                    }
                    mask &= !(1 << lane);
                }
                None
            })
        };
        let index = hit(candidates).into_iter().chain(hit(&self.grid.large)).max()?;
        Some(self.topology.nodes[index as usize].node)
    }
}

#[cfg(target_arch = "aarch64")]
#[inline]
fn point_mask(rects: [Rect; WIDTH], point: Point) -> u8 {
    use std::arch::aarch64::*;
    let [x, y, right, bottom] = edges(rects);
    // AArch64 guarantees NEON and all four lane loads are in bounds.
    unsafe {
        let px = vdupq_n_f32(point.x);
        let py = vdupq_n_f32(point.y);
        let load = |lane: &Lanes| vld1q_f32(lane.0.as_ptr());
        let horizontal = vandq_u32(vcgeq_f32(px, load(&x)), vcleq_f32(px, load(&right)));
        let vertical = vandq_u32(vcgeq_f32(py, load(&y)), vcleq_f32(py, load(&bottom)));
        let flags = vld1q_u32([1, 2, 4, 8].as_ptr());
        vaddvq_u32(vandq_u32(vandq_u32(horizontal, vertical), flags)) as u8
    }
}

#[cfg(not(target_arch = "aarch64"))]
#[inline]
fn point_mask(rects: [Rect; WIDTH], point: Point) -> u8 {
    let inside = rects.map(|r| (point.x >= r.x) & (point.x <= r.right()) & (point.y >= r.y) & (point.y <= r.bottom()));
    let [a, b, c, d] = inside.map(u8::from);
    a | (b << 1) | (c << 2) | (d << 3)
}

fn exact_hit(tree: &Tree, layout: &LayoutEngine, text: &TextEngine, entry: FlatNode, pos: Point) -> bool {
    let node = entry.node;
    let Some(core) = tree.node(node) else { return false };
    if core.flags.contains(NodeFlags::POINTER_EVENTS_NONE)
        || layout.clip(node).is_some_and(|clip| !clip.contains(pos))
        || !tree.is_shown(node)
        || tree.is_inert(node)
    {
        return false;
    }
    let radii = |node| tree.style(node).map_or([0.0; 4], |style| style.visual.radius);
    let hit = if entry.flags & LINK != 0 {
        text.link_rects(node).iter().any(|rect| rect.contains(pos))
    } else {
        layout.bounds(node).is_some_and(|rect| rect.contains_rounded(pos, radii(node)))
    };
    hit && std::iter::successors(tree.physical_parent(node), |&node| tree.physical_parent(node)).all(|node| {
        layout.clip(node).is_none_or(|clip| clip.contains(pos))
            && layout.child_clip(node).is_none_or(|clip| clip.contains(pos))
            && layout.rounded_child_clip(node).is_none_or(|clip| clip.contains_rounded(pos, radii(node)))
    })
}

#[derive(Default)]
pub(crate) struct FlatTree {
    current: FlatFrame,
    staging: FlatFrame,
    ready: bool,
    scratch: FrameArena<u32>,
}

impl FlatTree {
    pub(crate) fn project(&mut self, tree: &Tree, delta: &TreeDelta, layout: &LayoutEngine) {
        let previous = if self.ready { &self.staging } else { &self.current };
        let topology = if previous.topology.changed(tree, delta) {
            Arc::new(previous.topology.rebuild(tree, delta))
        } else if !layout.changed().is_empty() {
            previous.topology.clone()
        } else {
            return;
        };
        self.staging.topology = topology;
        self.staging.rebuild_bounds(layout, &mut self.scratch);
        self.ready = true;
    }

    pub(crate) fn commit(&mut self) {
        if std::mem::take(&mut self.ready) {
            std::mem::swap(&mut self.current, &mut self.staging);
        }
    }

    pub(crate) fn rollback(&mut self) {
        self.ready = false;
    }

    pub(crate) fn viewport_visible(&self, node: NodeId) -> Option<bool> {
        let frame = if self.ready { &self.staging } else { &self.current };
        let index = *frame.topology.indices.get(node)?;
        Some(!frame.bounds.get(index).is_empty())
    }

    pub(crate) fn hit(&self, tree: &Tree, layout: &LayoutEngine, text: &TextEngine, pos: Point) -> Option<NodeId> {
        let frame = if self.ready { &self.staging } else { &self.current };
        frame.hit(tree, layout, text, pos)
    }
}
