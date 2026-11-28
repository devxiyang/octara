//! Flat, paint-ordered geometry for viewport culling and native hit tests.

use crate::{LayoutKernel, TextEngine, paint_order};
use octara_model::geometry::{Point, Rect};
use octara_model::paint::{DisplayList, Item};
use octara_model::style::PlaneInverse;
use octara_tree::{DirtyBitmap, NodeFlags, NodeId, NodeKind, Tree, TreeDelta};
use rustc_hash::FxHashMap;
use slotmap::SecondaryMap;
use smallvec::SmallVec;
use std::cell::{OnceCell, RefCell};
use std::sync::Arc;

const LINK: u16 = 1;
const OVERLAY_ROOT: u16 = 2;
const CELL_SIZE: f32 = 128.0;
const MAX_CELLS: i64 = 16;
const SMALL_GRID_THRESHOLD: usize = 24;

/// Cumulative work performed by the spatial index.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FlatTreeMetrics {
    /// Full spatial index materializations.
    pub grid_rebuilds: u64,
    /// Flat indices refreshed by incremental patches.
    pub nodes_patched: u64,
    /// Committed frames that left spatial work deferred.
    pub lazy_skips: u64,
}

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

    fn is_within(&self, node: u32, ancestor: u32) -> bool {
        ancestor <= node && node < self.nodes[ancestor as usize].subtree_end
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

type Cell = (i32, i32);

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum Cells {
    #[default]
    Empty,
    Span {
        min: Cell,
        max: Cell,
    },
    Large,
}

impl Cells {
    fn for_bounds(rect: Rect) -> Self {
        if rect.is_empty() {
            return Self::Empty;
        }
        let min @ (x, y) = cell(Point::new(rect.x, rect.y));
        let max @ (right, bottom) = cell(Point::new(rect.right(), rect.bottom()));
        let columns = i64::from(right) - i64::from(x) + 1;
        let rows = i64::from(bottom) - i64::from(y) + 1;
        if columns <= 0 || rows <= 0 {
            Self::Empty
        } else if columns.saturating_mul(rows) > MAX_CELLS {
            Self::Large
        } else {
            Self::Span { min, max }
        }
    }
}

fn cell(point: Point) -> Cell {
    ((point.x / CELL_SIZE).floor() as i32, (point.y / CELL_SIZE).floor() as i32)
}

fn span_cells((x, y): Cell, (right, bottom): Cell) -> impl Iterator<Item = Cell> {
    (y..=bottom).flat_map(move |row| (x..=right).map(move |column| (column, row)))
}

fn remove_index(indices: &mut Vec<u32>, index: u32) {
    let slot = indices.iter().position(|&entry| entry == index).expect("indexed spatial entry");
    indices.swap_remove(slot);
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
struct CellFootprint {
    parent: Option<NodeId>,
    cells: Cells,
    scrollport: bool,
}

#[derive(Clone, Default)]
struct HashGrid {
    cells: FxHashMap<Cell, Vec<u32>>,
    large: Vec<u32>,
    len: usize,
}

impl HashGrid {
    fn insert(&mut self, index: u32, footprint: Cells) {
        match footprint {
            Cells::Empty => return,
            Cells::Span { min, max } => {
                for cell in span_cells(min, max) {
                    self.cells.entry(cell).or_default().push(index);
                }
            }
            Cells::Large => self.large.push(index),
        }
        self.len += 1;
    }

    fn remove(&mut self, index: u32, footprint: Cells) {
        match footprint {
            Cells::Empty => return,
            Cells::Span { min, max } => {
                for cell in span_cells(min, max) {
                    let entries = self.cells.get_mut(&cell).expect("indexed spatial cell");
                    remove_index(entries, index);
                    if entries.is_empty() {
                        self.cells.remove(&cell);
                    }
                }
            }
            Cells::Large => remove_index(&mut self.large, index),
        }
        self.len -= 1;
    }

    fn take_indices(&mut self, footprints: &[CellFootprint]) -> Vec<u32> {
        let mut indices = std::mem::take(&mut self.large);
        for (&cell, entries) in &self.cells {
            // A span contributes one entry, at the first cell in its footprint.
            indices.extend(
                entries.iter().copied().filter(|&index| matches!(footprints[index as usize].cells, Cells::Span { min, .. } if min == cell)),
            );
        }
        indices
    }
}

#[derive(Clone)]
enum Bucket {
    Linear(Vec<u32>),
    Hashed(HashGrid),
}

impl Default for Bucket {
    fn default() -> Self {
        Self::Linear(Vec::new())
    }
}

#[derive(Clone, Default)]
struct Grid {
    candidates: Bucket,
    scrollports: Vec<u32>,
}

impl Grid {
    fn patch(&mut self, index: u32, before: CellFootprint, after: CellFootprint, footprints: &[CellFootprint]) {
        // The child clip can extend beyond the box on its non-clipping axis.
        match (before.scrollport, after.scrollport) {
            (false, true) => self.scrollports.push(index),
            (true, false) => remove_index(&mut self.scrollports, index),
            _ => {}
        }
        if before.cells == after.cells {
            return;
        }
        match &mut self.candidates {
            Bucket::Linear(entries) => {
                match (before.cells, after.cells) {
                    (Cells::Empty, _) => entries.push(index),
                    (_, Cells::Empty) => remove_index(entries, index),
                    _ => {}
                }
                if entries.len() > SMALL_GRID_THRESHOLD {
                    let mut grid = HashGrid::default();
                    for &entry in entries.iter() {
                        grid.insert(entry, footprints[entry as usize].cells);
                    }
                    self.candidates = Bucket::Hashed(grid);
                }
            }
            Bucket::Hashed(grid) => {
                grid.remove(index, before.cells);
                grid.insert(index, after.cells);
                if grid.len <= SMALL_GRID_THRESHOLD {
                    self.candidates = Bucket::Linear(grid.take_indices(footprints));
                }
            }
        }
    }

    fn is_empty(&self) -> bool {
        self.scrollports.is_empty() && matches!(&self.candidates, Bucket::Linear(entries) if entries.is_empty())
    }

    fn at(&self, pos: Point) -> impl Iterator<Item = &u32> {
        let (nearby, large) = match &self.candidates {
            Bucket::Linear(entries) => (entries.as_slice(), &[][..]),
            Bucket::Hashed(grid) => (grid.cells.get(&cell(pos)).map_or(&[][..], Vec::as_slice), grid.large.as_slice()),
        };
        nearby.iter().rev().chain(large.iter().rev())
    }
}

#[derive(Clone, Default)]
struct SpatialIndex {
    // A cached index can lag the committed topology or reflect a staged hit.
    indexed_topology: Option<Arc<Topology>>,
    // Committed geometry changes that the cached index has yet to read.
    pending_patch: DirtyBitmap,
    staged_patch: DirtyBitmap,
    // Changes materialized against staged layout need to be read again after rollback.
    staged_materialized: DirtyBitmap,
    bounds: Vec<Rect>,
    footprints: Vec<CellFootprint>,
    root: Grid,
    scrolled: FxHashMap<NodeId, Grid>,
    metrics: FlatTreeMetrics,
}

impl SpatialIndex {
    fn matches(&self, topology: &Arc<Topology>) -> bool {
        self.indexed_topology.as_ref().is_some_and(|indexed| Arc::ptr_eq(indexed, topology))
    }

    fn materialize(&mut self, topology: &Arc<Topology>, layout: &LayoutKernel, topology_is_staged: bool) {
        let matches = self.matches(topology);
        if matches && self.pending_patch.is_empty() && self.staged_patch.is_empty() {
            return;
        }
        if !topology_is_staged {
            self.staged_materialized.union_with(&self.staged_patch);
        }
        self.pending_patch.union_with(&self.staged_patch);
        self.staged_patch.clear();

        let mut pending = std::mem::take(&mut self.pending_patch);
        {
            let changes = pending.iter().filter_map(|node| topology.indices.get(node).copied());
            let patch_limit = topology.nodes.len().div_ceil(4);
            if matches && changes.clone().take(patch_limit).count() < patch_limit {
                for index in changes {
                    self.update(topology, layout, index);
                    self.metrics.nodes_patched += 1;
                }
            } else {
                self.rebuild(topology, layout);
            }
        }
        pending.clear();
        self.pending_patch = pending;
    }

    fn rebuild(&mut self, topology: &Arc<Topology>, layout: &LayoutKernel) {
        self.indexed_topology = Some(Arc::clone(topology));
        self.root = Grid::default();
        self.scrolled.clear();
        self.bounds.resize(topology.nodes.len(), Rect::default());
        self.footprints.clear();
        self.footprints.resize(topology.nodes.len(), CellFootprint::default());
        for index in 0..topology.nodes.len() {
            self.update(topology, layout, flat_index(index));
        }
        self.metrics.grid_rebuilds += 1;
    }

    fn update(&mut self, topology: &Topology, layout: &LayoutKernel, index: u32) {
        let slot = index as usize;
        let entry = topology.nodes[slot];
        let bounds = layout.paint_bounds(entry.node).unwrap_or_default();
        self.bounds[slot] = bounds;
        let empty = CellFootprint::default();
        let mut after = empty;
        if entry.flags & OVERLAY_ROOT == 0 {
            after.cells = Cells::for_bounds(bounds);
            after.scrollport = layout.scroll_range(entry.node).is_some();
            if after.cells != Cells::Empty || after.scrollport {
                after.parent = layout.scroll_ancestor(entry.node);
            }
        }
        let before = std::mem::replace(&mut self.footprints[slot], after);
        if before.parent == after.parent {
            self.patch_grid(index, before.parent, before, after);
        } else {
            self.patch_grid(index, before.parent, before, empty);
            self.patch_grid(index, after.parent, empty, after);
        }
    }

    fn patch_grid(&mut self, index: u32, parent: Option<NodeId>, before: CellFootprint, after: CellFootprint) {
        if before.cells == after.cells && before.scrollport == after.scrollport {
            return;
        }
        if let Some(parent) = parent {
            let grid = self.scrolled.entry(parent).or_default();
            grid.patch(index, before, after, &self.footprints);
            if grid.is_empty() {
                self.scrolled.remove(&parent);
            }
        } else {
            self.root.patch(index, before, after, &self.footprints);
        }
    }

    fn hit(
        &self,
        topology: &Topology,
        tree: &Tree,
        layout: &LayoutKernel,
        text: &TextEngine,
        pos: Point,
        projected: Option<&ProjectedLayers>,
    ) -> Option<u32> {
        let mut pending = SmallVec::<[(&Grid, Point); 4]>::new();
        pending.push((&self.root, pos));
        let overlay = topology.indices.get(tree.overlay_root()).copied();
        let mut top = None;
        while let Some((grid, local)) = pending.pop() {
            for &index in grid.at(local) {
                if top.is_none_or(|top| top < index)
                    && self.bounds[index as usize].contains(local)
                    && projected.is_none_or(|layers| layers.containing(index).is_none())
                    && exact_hit(
                        tree,
                        layout,
                        text,
                        topology.nodes[index as usize],
                        pos,
                        overlay.is_some_and(|root| topology.is_within(index, root)),
                    )
                {
                    top = Some(index);
                }
            }
            for &index in &grid.scrollports {
                let entry = topology.nodes[index as usize];
                // The Euler interval covers every descendant in this scroll bucket.
                if top.is_some_and(|top| top >= entry.subtree_end)
                    || !layout.child_clip(entry.node).or_else(|| layout.clip(entry.node)).is_some_and(|clip| clip.contains(pos))
                {
                    continue;
                }
                if let Some(children) = self.scrolled.get(&entry.node) {
                    let shift = layout.scroll_translation(entry.node);
                    let offset = layout.scroll_offset(entry.node);
                    pending.push((children, Point::new(pos.x - shift.x + offset.x, pos.y - shift.y + offset.y)));
                }
            }
        }
        top
    }
}

fn exact_hit(tree: &Tree, layout: &LayoutKernel, text: &TextEngine, entry: FlatNode, pos: Point, overlay: bool) -> bool {
    let node = entry.node;
    let Some(core) = tree.node(node) else { return false };
    if core.flags.contains(NodeFlags::POINTER_EVENTS_NONE)
        || layout.clip(node).is_some_and(|clip| !clip.contains(pos))
        || !tree.is_shown(node)
        || tree.is_inert(node)
    {
        return false;
    }
    let rounded_clip = |node| layout.rounded_child_clip(node).is_none_or(|clip| clip.contains_rounded(pos, radii(tree, node)));
    contains_node(tree, layout, text, entry, pos)
        && std::iter::successors(layout.nearest_rounded_clip(node), |&node| layout.nearest_rounded_clip(node)).all(rounded_clip)
        // Overlay layout starts a new clip chain. Hit testing also respects the two physical roots.
        && (!overlay
            || [tree.overlay_root(), tree.root()]
                .into_iter()
                .all(|node| layout.child_clip(node).is_none_or(|clip| clip.contains(pos)) && rounded_clip(node)))
}

fn radii(tree: &Tree, node: NodeId) -> [f32; 4] {
    tree.style(node).map_or([0.0; 4], |style| style.visual.radius)
}

fn contains_node(tree: &Tree, layout: &LayoutKernel, text: &TextEngine, entry: FlatNode, pos: Point) -> bool {
    if entry.flags & LINK != 0 {
        text.link_contains(entry.node, layout, pos)
    } else {
        layout.bounds(entry.node).is_some_and(|rect| rect.contains_rounded(pos, radii(tree, entry.node)))
    }
}

#[derive(Clone, Copy)]
struct PlaneClip {
    rect: Rect,
    axes: [bool; 2],
    radius: [f32; 4],
}

impl PlaneClip {
    fn contains(self, pos: Point) -> bool {
        match self.axes {
            [true, true] => self.rect.contains_rounded(pos, self.radius),
            [true, false] => self.rect.x <= pos.x && pos.x < self.rect.right(),
            [false, true] => self.rect.y <= pos.y && pos.y < self.rect.bottom(),
            [false, false] => true,
        }
    }
}

#[derive(Clone)]
struct ProjectedLayer {
    start: u32,
    end: u32,
    parent: Option<usize>,
    inverse: Option<PlaneInverse>,
    clips: SmallVec<[PlaneClip; 2]>,
}

#[derive(Clone)]
struct ProjectedLayers {
    layers: Vec<ProjectedLayer>,
    scale: f32,
}

impl ProjectedLayers {
    fn collect(topology: &Topology, draw: &DisplayList, scale: f32) -> Self {
        let mut layers = Vec::new();
        let mut clips = SmallVec::<[PlaneClip; 8]>::new();
        let mut scopes = SmallVec::<[Option<usize>; 8]>::new();
        let mut clip_start = 0;
        for placed in draw.placed_items_unculled_at(scale) {
            let shifted = |rect: Rect| Rect::new(rect.x + placed.offset.x, rect.y + placed.offset.y, rect.w, rect.h);
            match *placed.item {
                Item::PushClip { rect, radius, .. } => clips.push(PlaneClip { rect: shifted(rect), axes: [true; 2], radius }),
                Item::PushAxisClip { rect, horizontal, .. } => {
                    clips.push(PlaneClip { rect: shifted(rect), axes: [horizontal, !horizontal], radius: [0.0; 4] });
                }
                Item::PopClip => {
                    clips.pop();
                }
                Item::PushLayer { .. } | Item::PushTransition(_) => scopes.push(None),
                Item::PushTransform { source, rect, transform } => {
                    if let Some(&start) = topology.indices.get(source) {
                        layers.push(ProjectedLayer {
                            start,
                            end: topology.nodes[start as usize].subtree_end,
                            parent: None,
                            inverse: PlaneInverse::new(&transform.device_matrix(shifted(rect), 1.0)),
                            clips: SmallVec::from_slice(&clips[clip_start..]),
                        });
                    }
                    // The renderer applies inherited clips to the composite in the parent plane.
                    scopes.push(Some(std::mem::replace(&mut clip_start, clips.len())));
                }
                Item::PopLayer => {
                    if let Some(Some(start)) = scopes.pop() {
                        clip_start = start;
                    }
                }
                _ => {}
            }
        }
        layers.sort_by_key(|layer| layer.start);
        layers.dedup_by_key(|layer| layer.start);
        let mut parent = None::<usize>;
        for index in 0..layers.len() {
            while let Some(outer) = parent.filter(|&outer| layers[outer].end <= layers[index].start) {
                parent = layers[outer].parent;
            }
            layers[index].parent = parent;
            parent = Some(index);
        }
        Self { layers, scale }
    }

    fn containing(&self, index: u32) -> Option<usize> {
        let mut at = self.layers.partition_point(|layer| layer.start <= index).checked_sub(1);
        while let Some(layer) = at {
            if index < self.layers[layer].end {
                return at;
            }
            at = self.layers[layer].parent;
        }
        None
    }

    fn source_point(&self, layer: usize, pos: Point, clipped: bool) -> Option<Point> {
        let layer = &self.layers[layer];
        // Each layer flattens onto its parent. Invert one source plane at a time.
        let pos = if let Some(parent) = layer.parent { self.source_point(parent, pos, clipped)? } else { pos };
        if clipped && !layer.clips.iter().all(|clip| clip.contains(pos)) {
            return None;
        }
        layer.inverse?.source_point(pos)
    }

    fn hit(&self, topology: &Topology, tree: &Tree, layout: &LayoutKernel, text: &TextEngine, pos: Point, top: Option<u32>) -> Option<u32> {
        if !layout.viewport_bounds().contains(pos) {
            return top;
        }
        let mut points = SmallVec::<[Option<Point>; 8]>::new();
        for layer in &self.layers {
            let point =
                layer.parent.map_or(Some(pos), |parent| points[parent]).filter(|&pos| layer.clips.iter().all(|clip| clip.contains(pos)));
            points.push(point.and_then(|pos| layer.inverse?.source_point(pos)));
        }
        let first = top.map_or(0, |top| top + 1);
        for (root_index, root) in self.layers.iter().enumerate().rev().filter(|(_, layer)| layer.parent.is_none()) {
            if points[root_index].is_none() {
                continue;
            }
            let mut end = root.end;
            while end > root.start.max(first) {
                end -= 1;
                let index = end;
                let layer = self.containing(index).expect("projected subtree");
                let Some(pos) = points[layer] else {
                    end = self.layers[layer].start;
                    continue;
                };
                let entry = topology.nodes[index as usize];
                if entry.flags & OVERLAY_ROOT == 0
                    && tree.node(entry.node).is_some_and(|core| !core.flags.contains(NodeFlags::POINTER_EVENTS_NONE))
                    && contains_node(tree, layout, text, entry, self.layout_point(layout, entry.node, pos))
                    && tree.is_shown(entry.node)
                    && !tree.is_inert(entry.node)
                    && self.source_clips_contain(topology, tree, layout, layer, entry.node, pos)
                {
                    return Some(index);
                }
            }
        }
        top
    }

    fn source_clips_contain(
        &self,
        topology: &Topology,
        tree: &Tree,
        layout: &LayoutKernel,
        layer: usize,
        node: NodeId,
        pos: Point,
    ) -> bool {
        let start = self.layers[layer].start;
        let inside_layer = |node| topology.indices.get(node).is_some_and(|&index| topology.is_within(index, start));
        let contains = |node| self.source_child_clip_contains(tree, layout, node, pos);
        std::iter::successors(layout.clip_ancestor(node), |&node| layout.clip_ancestor(node))
            .take_while(|&node| inside_layer(node))
            .all(contains)
            // Overlay layout restarts the clip chain, but paint wraps it in the physical root clips.
            && [tree.overlay_root(), tree.root()]
                .into_iter()
                .all(|root| root == node || !inside_layer(root) || !tree.is_within(node, root) || contains(root))
    }

    fn source_child_clip_contains(&self, tree: &Tree, layout: &LayoutKernel, node: NodeId, pos: Point) -> bool {
        let axes = layout.clip_axes(node);
        axes == [false; 2]
            || layout
                .world_client(node)
                .is_none_or(|rect| PlaneClip { rect, axes, radius: radii(tree, node) }.contains(self.layout_point(layout, node, pos)))
    }

    fn layout_point(&self, layout: &LayoutKernel, node: NodeId, pos: Point) -> Point {
        // Draw placement pixel-aligns cumulative vertical scroll. Layout queries use its unsnapped value.
        let scroll = layout.scroll_translation(node).y;
        let snapped = (scroll * self.scale).round() / self.scale;
        Point::new(pos.x, pos.y + (scroll - snapped))
    }
}

#[derive(Clone)]
struct ProjectedFrame {
    draw: Arc<DisplayList>,
    scale: f32,
    layers: OnceCell<ProjectedLayers>,
}

#[derive(Clone, Default)]
struct FlatFrame {
    topology: Arc<Topology>,
    spatial: RefCell<SpatialIndex>,
    projected: Option<ProjectedFrame>,
}

#[derive(Default)]
pub(crate) struct FlatTree {
    current: FlatFrame,
    staged_topology: Option<Arc<Topology>>,
    // A staged draw can remove all projected layers.
    staged_projected: Option<Option<ProjectedFrame>>,
}

impl FlatTree {
    pub(crate) fn project(&mut self, tree: &Tree, delta: &TreeDelta, layout: &LayoutKernel) {
        let previous = self.staged_topology.as_ref().unwrap_or(&self.current.topology);
        let spatial = self.current.spatial.get_mut();
        if previous.changed(tree, delta) {
            self.staged_topology = Some(Arc::new(previous.rebuild(tree, delta)));
            spatial.staged_patch.clear();
        } else {
            spatial.staged_patch.union_with(layout.changed());
        }
    }

    pub(crate) fn project_draw(&mut self, draw: Option<(&Arc<DisplayList>, f32)>) {
        if self.staged_topology.is_none()
            && draw
                .zip(self.current.projected.as_ref())
                .is_some_and(|((draw, scale), current)| scale == current.scale && Arc::ptr_eq(draw, &current.draw))
        {
            self.staged_projected = None;
            return;
        }
        self.staged_projected = Some(draw.map(|(draw, scale)| ProjectedFrame { draw: draw.clone(), scale, layers: OnceCell::new() }));
    }

    pub(crate) fn commit(&mut self) {
        if let Some(topology) = self.staged_topology.take() {
            self.current.topology = topology;
        }
        if let Some(projected) = self.staged_projected.take() {
            self.current.projected = projected;
        }
        let spatial = self.current.spatial.get_mut();
        let matches = spatial.matches(&self.current.topology);
        if matches {
            spatial.pending_patch.union_with(&spatial.staged_patch);
        } else {
            spatial.pending_patch.clear();
        }
        spatial.staged_patch.clear();
        spatial.staged_materialized.clear();
        if !matches || !spatial.pending_patch.is_empty() {
            spatial.metrics.lazy_skips += 1;
        }
    }

    pub(crate) fn rollback(&mut self) {
        self.staged_topology = None;
        self.staged_projected = None;
        let spatial = self.current.spatial.get_mut();
        if spatial.matches(&self.current.topology) {
            spatial.pending_patch.union_with(&spatial.staged_materialized);
        }
        spatial.staged_patch.clear();
        spatial.staged_materialized.clear();
    }

    pub(crate) fn metrics(&self) -> FlatTreeMetrics {
        self.current.spatial.borrow().metrics
    }

    /// Snapshot the index so oracle hits can materialize it independently.
    pub(crate) fn fork_for_layout_check(&self) -> Self {
        Self {
            current: self.current.clone(),
            staged_topology: self.staged_topology.clone(),
            staged_projected: self.staged_projected.clone(),
        }
    }

    fn projected_layers(&self, topology: &Topology) -> Option<&ProjectedLayers> {
        let frame = self.staged_projected.as_ref().unwrap_or(&self.current.projected).as_ref()?;
        let layers = frame.layers.get_or_init(|| ProjectedLayers::collect(topology, &frame.draw, frame.scale));
        (!layers.layers.is_empty()).then_some(layers)
    }

    pub(crate) fn source_point(&self, layout: &LayoutKernel, node: NodeId, pos: Point) -> Option<Point> {
        match self.layer_for(node) {
            Some((_, layers, layer)) => Some(layers.layout_point(layout, node, layers.source_point(layer, pos, false)?)),
            None => Some(pos),
        }
    }

    pub(crate) fn scrollbar_point(&self, tree: &Tree, layout: &LayoutKernel, node: NodeId, pos: Point) -> Option<Point> {
        let Some((topology, layers, layer)) = self.layer_for(node) else {
            return layout.child_clip(node).is_none_or(|clip| clip.contains(pos)).then_some(pos);
        };
        let pos = layers.source_point(layer, pos, true)?;
        (layers.source_clips_contain(topology, tree, layout, layer, node, pos)
            && layers.source_child_clip_contains(tree, layout, node, pos))
        .then(|| layers.layout_point(layout, node, pos))
    }

    fn layer_for(&self, node: NodeId) -> Option<(&Topology, &ProjectedLayers, usize)> {
        let topology = self.staged_topology.as_ref().unwrap_or(&self.current.topology);
        let layers = self.projected_layers(topology)?;
        let layer = layers.containing(*topology.indices.get(node)?)?;
        Some((topology, layers, layer))
    }

    pub(crate) fn hit(&self, tree: &Tree, layout: &LayoutKernel, text: &TextEngine, pos: Point) -> Option<NodeId> {
        let topology = self.staged_topology.as_ref().unwrap_or(&self.current.topology);
        if topology.nodes.is_empty() {
            return None;
        }
        let mut spatial = self.current.spatial.borrow_mut();
        spatial.materialize(topology, layout, self.staged_topology.is_some());
        let projected = self.projected_layers(topology);
        let top = spatial.hit(topology, tree, layout, text, pos, projected);
        let top = projected.map_or(top, |layers| layers.hit(topology, tree, layout, text, pos, top));
        top.map(|index| topology.nodes[index as usize].node)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use octara_model::geometry::{Size, Vec2, WindowViewport};
    use octara_model::style::{Overflow, Position, Radius, StyleProp};
    use octara_tree::tables::AuthoredStyle;
    use octara_tree::{NodeChange, NodeContent, NodeSpec, RuntimeOp, TreeId};

    struct Projection {
        flat: FlatTree,
        layout: LayoutKernel,
        text: TextEngine,
    }

    impl Projection {
        fn new() -> Self {
            Self { flat: FlatTree::default(), layout: LayoutKernel::default(), text: TextEngine::new(false) }
        }

        fn project(&mut self, tree: &Tree, delta: &TreeDelta) {
            let viewport = WindowViewport { size: Size::new(1024.0, 256.0), ..Default::default() };
            self.text.begin_stage();
            self.layout.project(tree, delta, viewport, &mut self.text);
            self.flat.project(tree, delta, &self.layout);
        }

        fn commit(&mut self) {
            self.layout.commit_stage();
            self.text.commit_stage();
            self.flat.commit();
        }

        fn rollback(&mut self) {
            self.layout.rollback_stage();
            self.text.rollback_stage();
            self.flat.rollback();
        }

        fn hit(&self, tree: &Tree, x: f32, y: f32) -> Option<NodeId> {
            self.flat.hit(tree, &self.layout, &self.text, Point::new(x, y))
        }
    }

    fn style(props: impl IntoIterator<Item = StyleProp>) -> NodeChange {
        NodeChange::Style(Box::new(AuthoredStyle { base: props.into_iter().collect(), ..Default::default() }))
    }

    fn bounds(x: i32, y: i32, size: i32) -> [StyleProp; 5] {
        [
            StyleProp::Position(Position::Absolute),
            StyleProp::Left(x.into()),
            StyleProp::Top(y.into()),
            StyleProp::Width(size.into()),
            StyleProp::Height(size.into()),
        ]
    }

    fn siblings() -> (Tree, TreeDelta, Vec<NodeId>) {
        let mut tree = Tree::new(TreeId::new([31, 71], 1), Default::default());
        let mut tx = tree.transaction();
        let nodes: Vec<_> = (0..30)
            .map(|index| {
                let node = tx.create(NodeSpec::new(NodeContent::Box));
                tx.change(node, style(bounds(index * 16, 0, 12)));
                tx.place(tree.node_ref(tree.root()), index as usize, node);
                node
            })
            .collect();
        let (receipt, mounted) = tree.apply(tx.finish()).unwrap();
        let nodes = nodes.into_iter().map(|node| receipt.node(node).unwrap()).collect();
        let mut initial = tree.initial_delta();
        initial.merge(mounted);
        (tree, initial, nodes)
    }

    fn move_nodes(tree: &mut Tree, nodes: &[NodeId], start: i32) -> TreeDelta {
        let mut tx = tree.transaction();
        for (index, &node) in nodes.iter().enumerate() {
            tx.change(tree.node_ref(node), style(bounds(start + index as i32 * 16, 0, 12)));
        }
        tree.apply(tx.finish()).unwrap().1
    }

    #[test]
    fn rollback_after_a_staged_hit_restores_committed_geometry_with_a_patch() {
        let (mut tree, initial, nodes) = siblings();
        let mut projection = Projection::new();
        projection.project(&tree, &initial);
        assert_eq!(projection.flat.metrics(), FlatTreeMetrics::default());
        projection.commit();
        assert_eq!(projection.flat.metrics(), FlatTreeMetrics { lazy_skips: 1, ..Default::default() });
        assert_eq!(projection.hit(&tree, 2.0, 2.0), Some(nodes[0]));

        let delta = move_nodes(&mut tree, &nodes[..1], 640);
        projection.project(&tree, &delta);
        projection.commit();
        assert_eq!(projection.flat.metrics(), FlatTreeMetrics { grid_rebuilds: 1, nodes_patched: 0, lazy_skips: 2 });

        let delta = move_nodes(&mut tree, &nodes[1..2], 680);
        projection.project(&tree, &delta);
        let oracle = projection.flat.fork_for_layout_check();
        assert_eq!(oracle.hit(&tree, &projection.layout, &projection.text, Point::new(682.0, 2.0)), Some(nodes[1]));
        assert_eq!(oracle.metrics().nodes_patched, 2);
        assert_eq!(projection.flat.metrics().nodes_patched, 0);
        assert_eq!(projection.hit(&tree, 642.0, 2.0), Some(nodes[0]));
        assert_eq!(projection.hit(&tree, 682.0, 2.0), Some(nodes[1]));
        assert_eq!(projection.flat.metrics().nodes_patched, 2);

        let delta = move_nodes(&mut tree, &nodes[2..3], 720);
        projection.project(&tree, &delta);
        projection.rollback();
        assert_eq!(projection.hit(&tree, 642.0, 2.0), Some(nodes[0]));
        assert_eq!(projection.hit(&tree, 18.0, 2.0), Some(nodes[1]));
        assert_eq!(projection.hit(&tree, 34.0, 2.0), Some(nodes[2]));
        assert_eq!(projection.hit(&tree, 682.0, 2.0), Some(tree.root()));
        assert_eq!(projection.flat.metrics(), FlatTreeMetrics { grid_rebuilds: 1, nodes_patched: 3, lazy_skips: 2 });
    }

    #[test]
    fn rollback_of_an_indexed_topology_keeps_the_committed_pending_geometry() {
        let (mut tree, initial, nodes) = siblings();
        let mut projection = Projection::new();
        projection.project(&tree, &initial);
        projection.commit();
        assert_eq!(projection.hit(&tree, 2.0, 2.0), Some(nodes[0]));

        let delta = move_nodes(&mut tree, &nodes[..1], 640);
        projection.project(&tree, &delta);
        projection.commit();

        let mut tx = tree.transaction();
        let overlay = tx.create(NodeSpec::new(NodeContent::Box));
        tx.change(overlay, style(bounds(640, 0, 12)));
        tx.place(tree.node_ref(tree.overlay_root()), 0, overlay);
        let (receipt, delta) = tree.apply(tx.finish()).unwrap();
        projection.project(&tree, &delta);
        assert_eq!(projection.hit(&tree, 642.0, 2.0), receipt.node(overlay));

        projection.rollback();
        assert_eq!(projection.hit(&tree, 642.0, 2.0), Some(nodes[0]));
        assert_eq!(projection.hit(&tree, 2.0, 2.0), Some(tree.root()));
        assert_eq!(projection.flat.metrics(), FlatTreeMetrics { grid_rebuilds: 3, nodes_patched: 0, lazy_skips: 2 });
    }

    #[test]
    fn a_quarter_of_the_topology_requires_a_lazy_rebuild() {
        let (mut tree, initial, nodes) = siblings();
        let mut projection = Projection::new();
        projection.project(&tree, &initial);
        projection.commit();
        projection.hit(&tree, 2.0, 2.0);
        let delta = move_nodes(&mut tree, &nodes[..8], 640);
        projection.project(&tree, &delta);
        projection.commit();
        assert_eq!(projection.flat.metrics(), FlatTreeMetrics { grid_rebuilds: 1, nodes_patched: 0, lazy_skips: 2 });
        assert_eq!(projection.hit(&tree, 642.0, 2.0), Some(nodes[0]));
        assert_eq!(projection.flat.metrics(), FlatTreeMetrics { grid_rebuilds: 2, nodes_patched: 0, lazy_skips: 2 });
    }

    #[test]
    fn bucket_switches_at_twenty_four_candidates_without_duplicates() {
        let span = CellFootprint { cells: Cells::Span { min: (0, 0), max: (1, 1) }, ..Default::default() };
        let large = CellFootprint { cells: Cells::Large, ..Default::default() };
        let empty = CellFootprint::default();
        let mut grid = Grid::default();
        let mut footprints = vec![span; SMALL_GRID_THRESHOLD + 1];
        for index in 0..SMALL_GRID_THRESHOLD {
            grid.patch(flat_index(index), empty, span, &footprints);
        }
        assert!(matches!(&grid.candidates, Bucket::Linear(entries) if entries.len() == SMALL_GRID_THRESHOLD));

        let last = flat_index(SMALL_GRID_THRESHOLD);
        footprints[last as usize] = large;
        grid.patch(last, empty, large, &footprints);
        assert!(matches!(&grid.candidates, Bucket::Hashed(grid) if grid.large == [last] && grid.cells.len() == 4));
        footprints[1] = empty;
        grid.patch(1, span, empty, &footprints);
        assert!(matches!(&grid.candidates, Bucket::Linear(entries) if entries.len() == SMALL_GRID_THRESHOLD));
        let mut actual: Vec<_> = grid.at(Point::new(12.0, 12.0)).copied().collect();
        actual.sort_unstable();
        assert!(actual.into_iter().eq((0..=last).filter(|&index| index != 1)));
    }

    #[test]
    fn a_scroll_bucket_can_hit_outside_the_scrollport_on_the_visible_axis() {
        let mut tree = Tree::new(TreeId::new([31, 71], 2), Default::default());
        let mut tx = tree.transaction();
        let port = tx.create(NodeSpec::new(NodeContent::Box));
        let child = tx.create(NodeSpec::new(NodeContent::Box));
        tx.change(port, style(bounds(0, 0, 40).into_iter().chain([StyleProp::OverflowY(Overflow::Scroll)])));
        tx.change(child, style(bounds(130, 60, 20)));
        tx.place(tree.node_ref(tree.root()), 0, port);
        tx.place(port, 0, child);
        let (receipt, mounted) = tree.apply(tx.finish()).unwrap();
        let port = receipt.node(port).unwrap();
        let child = receipt.node(child).unwrap();
        let mut initial = tree.initial_delta();
        initial.merge(mounted);
        let mut projection = Projection::new();
        projection.project(&tree, &initial);
        projection.commit();
        assert_eq!(projection.hit(&tree, 135.0, 35.0), Some(tree.root()));

        let delta = tree.apply_runtime(RuntimeOp::RequestScroll { node: port, target: Vec2::new(0.0, 40.0) });
        projection.project(&tree, &delta);
        projection.commit();
        assert_eq!(projection.hit(&tree, 135.0, 35.0), Some(child));
        assert_eq!(projection.hit(&tree, 135.0, 45.0), Some(tree.root()));
        assert_eq!(projection.flat.metrics(), FlatTreeMetrics { grid_rebuilds: 1, nodes_patched: 0, lazy_skips: 1 });
    }

    #[test]
    fn overlay_hits_respect_the_root_rounded_clip() {
        let mut tree = Tree::new(TreeId::new([31, 71], 3), Default::default());
        let mut tx = tree.transaction();
        let child = tx.create(NodeSpec::new(NodeContent::Box));
        tx.change(
            tree.node_ref(tree.root()),
            style([
                StyleProp::OverflowX(Overflow::Hidden),
                StyleProp::OverflowY(Overflow::Hidden),
                StyleProp::RadiusTopLeft(Radius::Px(50.0)),
            ]),
        );
        tx.change(child, style(bounds(0, 0, 80)));
        tx.place(tree.node_ref(tree.overlay_root()), 0, child);
        let (receipt, mounted) = tree.apply(tx.finish()).unwrap();
        let mut initial = tree.initial_delta();
        initial.merge(mounted);
        let mut projection = Projection::new();
        projection.project(&tree, &initial);
        projection.commit();

        assert_eq!(projection.hit(&tree, 2.0, 2.0), None);
        assert_eq!(projection.hit(&tree, 35.0, 35.0), receipt.node(child));
    }
}
