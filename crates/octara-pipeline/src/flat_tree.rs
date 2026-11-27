//! Flat, paint-ordered geometry for viewport culling and native hit tests.

use crate::{LayoutEngine, TextEngine, paint_order};
use octara_model::geometry::{Point, Rect};
use octara_tree::{DirtyBitmap, NodeFlags, NodeId, NodeKind, Tree, TreeDelta};
use rustc_hash::FxHashMap;
use slotmap::SecondaryMap;
use std::sync::Arc;

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

type Cell = (i32, i32);

#[derive(Default)]
struct Grid {
    cells: FxHashMap<Cell, Vec<u32>>,
    large: Vec<u32>,
    scrollports: Vec<u32>,
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
        self.scrollports.clear();
        for candidates in self.cells.values_mut() {
            candidates.clear();
        }
    }
}

fn cell(point: Point) -> Cell {
    ((point.x / CELL_SIZE).floor() as i32, (point.y / CELL_SIZE).floor() as i32)
}

#[derive(Default)]
struct FlatFrame {
    topology: Arc<Topology>,
    bounds: Vec<Rect>,
    grid: Grid,
    scrolled: FxHashMap<NodeId, Grid>,
}

impl FlatFrame {
    fn rebuild_bounds(&mut self, layout: &LayoutEngine) {
        self.bounds.clear();
        self.grid.clear();
        self.scrolled.retain(|&node, grid| {
            grid.clear();
            self.topology.indices.contains_key(node) && layout.scroll_range(node).is_some()
        });
        for (index, entry) in self.topology.nodes.iter().enumerate() {
            let bounds = layout.paint_bounds(entry.node).unwrap_or_default();
            self.bounds.push(bounds);
            if entry.flags & OVERLAY_ROOT != 0 {
                continue;
            }
            let grid = if let Some(ancestor) = layout.scroll_ancestor(entry.node) {
                self.scrolled.entry(ancestor).or_default()
            } else {
                &mut self.grid
            };
            let index = flat_index(index);
            grid.insert(index, bounds);
            if layout.scroll_range(entry.node).is_some() {
                // The child clip can extend beyond the box on its non-clipping axis.
                grid.scrollports.push(index);
            }
        }
        for grid in std::iter::once(&mut self.grid).chain(self.scrolled.values_mut()) {
            grid.cells.retain(|_, entries| !entries.is_empty());
        }
    }

    fn hit(&self, tree: &Tree, layout: &LayoutEngine, text: &TextEngine, pos: Point) -> Option<NodeId> {
        let mut pending = smallvec::SmallVec::<[(&Grid, Point); 4]>::new();
        pending.push((&self.grid, pos));
        let mut top = None;
        while let Some((grid, local)) = pending.pop() {
            let candidates = grid.cells.get(&cell(local)).map_or(&[][..], Vec::as_slice);
            for &index in candidates.iter().rev().chain(grid.large.iter().rev()) {
                if top.is_none_or(|top| top < index)
                    && self.bounds[index as usize].contains(local)
                    && exact_hit(tree, layout, text, self.topology.nodes[index as usize], pos)
                {
                    top = Some(index);
                }
            }
            for &index in &grid.scrollports {
                let entry = self.topology.nodes[index as usize];
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
        top.map(|index| self.topology.nodes[index as usize].node)
    }
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
        text.link_contains(node, layout, pos)
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
        self.staging.rebuild_bounds(layout);
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

    pub(crate) fn hit(&self, tree: &Tree, layout: &LayoutEngine, text: &TextEngine, pos: Point) -> Option<NodeId> {
        let frame = if self.ready { &self.staging } else { &self.current };
        frame.hit(tree, layout, text, pos)
    }
}
