//! Direct traversal and stateless layout algorithms over the physical ECS tree.

use super::*;
use style::{OctaraStyleRef, StyleOverride};
use taffy::{
    CacheTree, CoreStyle, DetailedGridInfo, DetailedLayoutInfo, LayoutContainingBlock, LayoutFlexboxContainer, LayoutGridContainer,
    LayoutPartialTree, MaybeResolve, TraversePartialTree,
};

#[derive(Clone)]
pub(super) struct BoundaryState {
    pub(super) input: taffy::LayoutInput,
    pub(super) output: taffy::LayoutOutput,
}

#[derive(Clone, Copy)]
struct SizingProbe {
    input: taffy::LayoutInput,
    size: taffy::Size<f32>,
}

impl SizingProbe {
    fn matches(self, size: taffy::Size<f32>) -> bool {
        match self.input.axis {
            taffy::RequestedAxis::Horizontal => self.size.width == size.width,
            taffy::RequestedAxis::Vertical => self.size.height == size.height,
            taffy::RequestedAxis::Both => self.size == size,
        }
    }
}

#[derive(Default)]
pub(super) struct BoundaryProbes {
    entries: SmallVec<[SizingProbe; 4]>,
    incomplete: bool,
}

impl BoundaryProbes {
    fn record(&mut self, input: taffy::LayoutInput, size: taffy::Size<f32>) {
        if self.incomplete {
            return;
        }
        if let Some(probe) = self.entries.iter_mut().find(|probe| probe.input == input) {
            probe.size = size;
        } else if self.entries.len() < self.entries.inline_size() {
            self.entries.push(SizingProbe { input, size });
        } else {
            // A boundary without a complete record of its parent-facing probes
            // must defer to the enclosing layout.
            self.incomplete = true;
        }
    }
}

pub(super) struct Compute<'a> {
    pub(super) kernel: &'a mut LayoutKernel,
    tree: &'a Tree,
    text: &'a mut TextEngine,
    rechecking: Option<NodeId>,
    suppressed_children: Option<NodeId>,
}

pub(super) fn layout_style(tree: &Tree, node: NodeId) -> &LayoutStyle {
    static DEFAULT: std::sync::LazyLock<LayoutStyle> = std::sync::LazyLock::new(LayoutStyle::default);
    tree.style(node).map_or(&DEFAULT, |style| &style.layout)
}

impl<'a> Compute<'a> {
    pub(super) fn new(kernel: &'a mut LayoutKernel, tree: &'a Tree, text: &'a mut TextEngine) -> Self {
        Self { kernel, tree, text, rechecking: None, suppressed_children: None }
    }

    pub(super) fn root(&mut self, node: NodeId, extent: Size) {
        self.kernel.work.computes += 1;
        taffy::compute_root_layout(
            self,
            native_id(node),
            taffy::Size { width: AvailableSpace::Definite(extent.width), height: AvailableSpace::Definite(extent.height) },
        );
    }

    pub(super) fn boundary(&mut self, node: NodeId, input: taffy::LayoutInput) -> (taffy::LayoutOutput, bool) {
        self.kernel.work.computes += 1;
        let id = native_id(node);
        let mut probes_changed = false;
        if let Some(probes) = self.kernel.boundary_probes.get(node) {
            let entries = probes.entries.clone();
            // A definite CSS used size can still originate in an intrinsic probe,
            // such as the auto flex basis of an item in a definite container.
            self.rechecking = Some(node);
            probes_changed = entries.into_iter().any(|probe| !probe.matches(self.compute_child_layout(id, probe.input).size));
            self.rechecking = None;
        }
        let output = self.compute_child_layout(id, input);
        let mut layout = self.kernel.layouts[node];
        layout.size = output.size;
        layout.scrollable_overflow_rect = output.scrollable_overflow_rect;
        self.set_unrounded_layout(id, &layout);
        (output, probes_changed)
    }

    fn authored_children(&self, node: NodeId) -> &[NodeId] {
        if self.suppressed_children == Some(node) || self.tree.text(node).is_some() { &[] } else { self.tree.children(node) }
    }

    fn style(&self, node: taffy::NodeId) -> OctaraStyleRef<'_> {
        effective_style(self.tree, octara_id(node), &self.kernel.fits, self.kernel.window)
    }

    fn measure(&mut self, node: NodeId, known: taffy::Size<Option<f32>>, available: taffy::Size<AvailableSpace>) -> taffy::Size<f32> {
        let width = known.width.or(match available.width {
            AvailableSpace::Definite(width) => Some(width),
            AvailableSpace::MinContent => Some(0.0),
            AvailableSpace::MaxContent => None,
        });
        let tree = self.tree;
        let size = if let Some(size) = self.text.measure(tree, node, known.width, width) {
            size
        } else if let Some(&size) = self.kernel.native_sizes.get(node) {
            size
        } else if tree.icon(node).is_some() {
            let size = tree.visual(node).font_size;
            Size::new(size, size)
        } else if let Some((width, height)) = tree
            .image(node)
            .map(|slot| slot.image.size())
            .or_else(|| tree.media_source(node).map(|slot| slot.natural_size().unwrap_or((300, 150))))
        {
            let ratio = width as f32 / (height as f32).max(1.0);
            match (known.width, known.height) {
                (Some(w), None) => Size::new(w, w / ratio.max(f32::EPSILON)),
                (None, Some(h)) => Size::new(h * ratio, h),
                _ => Size::new(width as f32, height as f32),
            }
        } else {
            Size::new(300.0, 150.0)
        };
        taffy::Size { width: known.width.unwrap_or(size.width), height: known.height.unwrap_or(size.height) }
    }

    fn compute_visible(
        &mut self,
        id: taffy::NodeId,
        input: taffy::LayoutInput,
        display: Display,
        has_children: bool,
        measured: bool,
    ) -> taffy::LayoutOutput {
        let node = octara_id(id);
        let style = effective_style(self.tree, node, &self.kernel.fits, self.kernel.window);
        let contain = layout_style(self.tree, node).contain;
        let mut inner_input = input;
        if contain.contains(Contain::SIZE) && (has_children || measured) {
            // CSS size containment sizes the box as if it were empty. A grid's explicit
            // tracks and gaps still contribute, then its real descendants are laid out.
            let empty = if display == Display::Grid && has_children {
                let mut empty_input = input;
                if empty_input.run_mode == taffy::RunMode::PerformLayout {
                    empty_input.run_mode = taffy::RunMode::ComputeSize;
                    empty_input.axis = taffy::RequestedAxis::Both;
                }
                let suppressed = self.suppressed_children.replace(node);
                let output = taffy::compute_grid_layout(self, id, empty_input);
                self.suppressed_children = suppressed;
                output
            } else {
                empty_leaf(input, &style)
            };
            if !has_children || input.run_mode == taffy::RunMode::ComputeSize {
                return empty;
            }
            inner_input.known_dimensions = empty.size.map(Some);
            let authored_size = if input.sizing_mode == taffy::SizingMode::InherentSize {
                style.size().maybe_resolve(input.parent_size, |_, _| 0.0)
            } else {
                taffy::Size::NONE
            };
            inner_input.known_dimensions_are_definite = taffy::Size {
                width: input.known_dimensions.width.map_or(authored_size.width.is_some(), |_| input.known_dimensions_are_definite.width),
                height: input
                    .known_dimensions
                    .height
                    .map_or(authored_size.height.is_some(), |_| input.known_dimensions_are_definite.height),
            };
        }

        let mut output = match (display, has_children) {
            (Display::Flex, true) => taffy::compute_flexbox_layout(self, id, inner_input),
            (Display::Grid, _) if has_children || !measured => taffy::compute_grid_layout(self, id, inner_input),
            (_, false) if measured => {
                // Borrow the authored style and copy only the scalar overrides so measurement
                // can mutate the text engine independently of the layout tree.
                let style =
                    OctaraStyleRef::new(layout_style(self.tree, node), overrides(self.tree, node, &self.kernel.fits, self.kernel.window));
                taffy::compute_leaf_layout(inner_input, &style, |_, _| 0.0, |known, available| self.measure(node, known, available))
            }
            (_, false) => empty_leaf(inner_input, &style),
            _ => unreachable!("hidden nodes dispatch before visible layout"),
        };
        if input.run_mode == taffy::RunMode::PerformLayout {
            if output.oof_candidates.is_empty() {
                self.clear_hoisted_children(id);
            } else {
                taffy::compute_oof_layout(self, id, &mut output);
            }
        }
        output
    }

    fn record_size_probe(&mut self, node: NodeId, input: taffy::LayoutInput, size: taffy::Size<f32>) {
        if input.known_dimensions.width.is_some() && input.known_dimensions.height.is_some() {
            return;
        }
        let authored = layout_style(self.tree, node);
        if authored.contain.contains(Contain::SIZE)
            || authored.overflow_x == Overflow::Visible
            || authored.overflow_y == Overflow::Visible
            || self.is_inflexible_min_probe(node, input, authored)
        {
            return;
        }
        self.kernel.boundary_probes.entry(node).expect("layout probe key").or_default().record(input, size);
    }

    fn is_inflexible_min_probe(&self, node: NodeId, input: taffy::LayoutInput, authored: &LayoutStyle) -> bool {
        if input.sizing_mode != taffy::SizingMode::ContentSize
            || authored.flex_basis.is_some()
            || authored.flex_grow != 0.0
            || authored.flex_shrink != 0.0
            || matches!(authored.position, Position::Absolute | Position::Fixed)
            || self.tree.overlay(node).is_some_and(|slot| slot.target.is_some())
        {
            return false;
        }
        let Some(parent) = self.tree.physical_parent(node) else { return false };
        let parent_style = layout_style(self.tree, parent);
        if parent_style.display != Display::Flex {
            return false;
        }
        let direction = if parent == self.tree.root() || parent == self.tree.overlay_root() {
            FlexDirection::Column
        } else {
            parent_style.flex_direction
        };
        let (axis, size, space) = match direction {
            FlexDirection::Row | FlexDirection::RowReverse => {
                (taffy::RequestedAxis::Horizontal, authored.size.width, input.available_space.width)
            }
            FlexDirection::Column | FlexDirection::ColumnReverse => {
                (taffy::RequestedAxis::Vertical, authored.size.height, input.available_space.height)
            }
        };
        // Flexbox caps this automatic minimum at the explicitly authored main
        // size. The inflexible item already uses that size as its flex basis.
        input.axis == axis && matches!(size, Some(LayoutLength::Px(_))) && space == AvailableSpace::MinContent
    }

    fn record_layout(&mut self, node: NodeId, input: taffy::LayoutInput, output: &taffy::LayoutOutput) {
        let authored = layout_style(self.tree, node);
        let escaping = !output.oof_candidates.is_empty();
        let definite = input.known_dimensions.width.is_some()
            && input.known_dimensions.height.is_some()
            && input.known_dimensions_are_definite.width
            && input.known_dimensions_are_definite.height;
        let authored_definite =
            [authored.size.width, authored.size.height].into_iter().all(|size| matches!(size, Some(LayoutLength::Px(_))));
        let overflow = self.style(native_id(node)).overflow();
        let explicit =
            node == self.tree.root() || isolated_anchor(self.tree, node) || authored.contain.contains(Contain::LAYOUT | Contain::SIZE);
        let implicit = !escaping
            && self.kernel.boundary_probes.get(node).is_none_or(|probes| !probes.incomplete)
            && (definite || authored_definite)
            && overflow.x != taffy::Overflow::Visible
            && overflow.y != taffy::Overflow::Visible;
        let flags = &mut self.kernel.meta[node].flags;
        flags.remove(LayoutFlags::DIRTY | LayoutFlags::SUBTREE_DIRTY);
        flags.set(LayoutFlags::HAS_ESCAPING_OOF, escaping);
        flags.set(LayoutFlags::BOUNDARY_EXPLICIT, explicit);
        flags.set(LayoutFlags::BOUNDARY_IMPLICIT, implicit && !explicit);
        if (explicit || implicit) && node != self.tree.root() && !isolated_anchor(self.tree, node) {
            let state = BoundaryState { input, output: output.clone() };
            if let Some(current) = self.kernel.boundary_states.get_mut(node) {
                **current = state;
            } else {
                self.kernel.boundary_states.insert(node, Box::new(state));
            }
        } else {
            self.kernel.boundary_states.remove(node);
        }
    }
}

pub(super) fn is_measure_leaf(kind: NodeKind) -> bool {
    matches!(kind, NodeKind::Text | NodeKind::TextInput | NodeKind::Icon | NodeKind::Image | NodeKind::Media | NodeKind::Native)
}

fn empty_leaf(input: taffy::LayoutInput, style: &OctaraStyleRef<'_>) -> taffy::LayoutOutput {
    taffy::compute_leaf_layout(input, style, |_, _| 0.0, |known, _| known.map(|dimension| dimension.unwrap_or(0.0)))
}

type ChildIndices = SmallVec<[usize; 2]>;

pub(super) fn excluded_children(tree: &Tree, parent: NodeId) -> Option<ChildIndices> {
    if tree.text(parent).is_some() {
        return None;
    }
    let excluded: ChildIndices = tree
        .children(parent)
        .iter()
        .enumerate()
        .filter_map(|(index, &child)| (!tree.is_live(child) || tree.link(child).is_some() || isolated_anchor(tree, child)).then_some(index))
        .collect();
    (!excluded.is_empty()).then_some(excluded)
}

pub(super) fn isolated_anchor(tree: &Tree, node: NodeId) -> bool {
    tree.physical_parent(node) == Some(tree.overlay_root()) && tree.overlay(node).is_some_and(|slot| slot.target.is_some())
}

pub(super) struct Children<'a> {
    children: std::iter::Enumerate<std::slice::Iter<'a, NodeId>>,
    excluded: &'a [usize],
}

impl Iterator for Children<'_> {
    type Item = taffy::NodeId;

    fn next(&mut self) -> Option<Self::Item> {
        for (index, &child) in self.children.by_ref() {
            if self.excluded.first() == Some(&index) {
                self.excluded = &self.excluded[1..];
            } else {
                return Some(native_id(child));
            }
        }
        None
    }
}

impl TraversePartialTree for Compute<'_> {
    type ChildIter<'a>
        = Children<'a>
    where
        Self: 'a;

    fn child_ids(&self, node: taffy::NodeId) -> Children<'_> {
        let node = octara_id(node);
        let children = self.authored_children(node);
        let excluded: &[usize] =
            if children.is_empty() { &[] } else { self.kernel.excluded_children.get(node).map_or(&[], SmallVec::as_slice) };
        Children { children: children.iter().enumerate(), excluded }
    }

    fn child_count(&self, node: taffy::NodeId) -> usize {
        let node = octara_id(node);
        let length = self.authored_children(node).len();
        if length == 0 { 0 } else { length - self.kernel.excluded_children.get(node).map_or(0, SmallVec::len) }
    }

    fn get_child_id(&self, node: taffy::NodeId, mut index: usize) -> taffy::NodeId {
        let node = octara_id(node);
        if let Some(excluded) = self.kernel.excluded_children.get(node) {
            for &skip in excluded {
                if skip > index {
                    break;
                }
                index += 1;
            }
        }
        native_id(self.authored_children(node)[index])
    }
}

impl LayoutPartialTree for Compute<'_> {
    type CoreContainerStyle<'a>
        = OctaraStyleRef<'a>
    where
        Self: 'a;
    type CustomIdent = String;

    fn get_core_container_style(&self, node: taffy::NodeId) -> OctaraStyleRef<'_> {
        self.style(node)
    }

    fn set_unrounded_layout(&mut self, id: taffy::NodeId, layout: &taffy::Layout) {
        let node = octara_id(id);
        if self.kernel.layouts.get(node) != Some(layout) {
            self.kernel.layouts.insert(node, *layout);
            self.kernel.stamp(node);
        }
    }

    fn compute_child_layout(&mut self, id: taffy::NodeId, input: taffy::LayoutInput) -> taffy::LayoutOutput {
        let node = octara_id(id);
        let display = layout_style(self.tree, node).display;
        if display == Display::None || input.run_mode == taffy::RunMode::PerformHiddenLayout {
            self.clear_hoisted_children(id);
            self.kernel.grid_info.remove(node);
            self.kernel.boundary_states.remove(node);
            self.kernel.boundary_probes.remove(node);
            self.kernel.meta[node].flags = LayoutFlags::LIVE;
            // Hidden descendants keep their warm state. Their published frames are removed
            // by the visibility walk, and future edits still invalidate their caches.
            let suppressed = self.suppressed_children.replace(node);
            let output = taffy::compute_hidden_layout(self, id);
            self.suppressed_children = suppressed;
            return output;
        }
        let has_children = self.child_count(id) != 0;
        let measured = self.tree.node(node).is_some_and(|core| is_measure_leaf(core.kind));
        let output = if has_children || measured || display == Display::Grid {
            taffy::compute_cached_layout(self, id, input, |this, id, input| {
                if input.run_mode == taffy::RunMode::PerformLayout {
                    this.kernel.stamp(node);
                    this.kernel.grid_info.remove(node);
                }
                this.compute_visible(id, input, display, has_children, measured)
            })
        } else {
            if input.run_mode == taffy::RunMode::PerformLayout {
                self.cache_clear(id);
                self.kernel.boundary_probes.remove(node);
                self.clear_hoisted_children(id);
                self.kernel.grid_info.remove(node);
            }
            self.compute_visible(id, input, display, false, false)
        };
        if input.run_mode == taffy::RunMode::PerformLayout {
            self.record_layout(node, input, &output);
        } else if has_children && self.rechecking != Some(node) {
            self.record_size_probe(node, input, output.size);
        }
        output
    }
}

impl CacheTree for Compute<'_> {
    fn cache_get(&mut self, node: taffy::NodeId, input: &taffy::LayoutInput) -> Option<taffy::LayoutOutput> {
        self.kernel.caches.get_mut(octara_id(node))?.get(input)
    }

    fn cache_store(&mut self, node: taffy::NodeId, input: &taffy::LayoutInput, output: taffy::LayoutOutput) {
        self.kernel.caches.entry(octara_id(node)).expect("live layout key").or_default().store(input, output);
    }

    fn cache_clear(&mut self, node: taffy::NodeId) {
        self.kernel.caches.remove(octara_id(node));
    }
}

impl LayoutContainingBlock for Compute<'_> {
    type OofItemStyle<'a>
        = OctaraStyleRef<'a>
    where
        Self: 'a;

    fn get_oof_item_style(&self, node: taffy::NodeId) -> OctaraStyleRef<'_> {
        self.style(node)
    }

    fn clear_hoisted_children(&mut self, node: taffy::NodeId) {
        let owner = octara_id(node);
        for child in self.kernel.hoisted_oof.remove(owner).into_iter().flatten() {
            if self.kernel.hoisted_by.get(child) == Some(&owner) {
                self.kernel.hoisted_by.remove(child);
                self.kernel.stamp(child);
            }
        }
    }

    fn add_hoisted_children(&mut self, node: taffy::NodeId, hoisted: &[taffy::NodeId]) {
        if hoisted.is_empty() {
            return;
        }
        let owner = octara_id(node);
        for child in hoisted.iter().copied().map(octara_id) {
            self.kernel.hoisted_oof.entry(owner).expect("live containing block").or_default().push(child);
            if self.kernel.hoisted_by.insert(child, owner) != Some(owner) {
                self.kernel.stamp(child);
            }
        }
    }

    fn get_detailed_layout_info(&self, node: taffy::NodeId) -> &DetailedLayoutInfo<String> {
        self.kernel.grid_info.get(octara_id(node)).unwrap_or(&DetailedLayoutInfo::None)
    }
}

impl LayoutFlexboxContainer for Compute<'_> {
    type FlexboxContainerStyle<'a>
        = OctaraStyleRef<'a>
    where
        Self: 'a;
    type FlexboxItemStyle<'a>
        = OctaraStyleRef<'a>
    where
        Self: 'a;

    fn get_flexbox_container_style(&self, node: taffy::NodeId) -> OctaraStyleRef<'_> {
        self.style(node)
    }

    fn get_flexbox_child_style(&self, node: taffy::NodeId) -> OctaraStyleRef<'_> {
        self.style(node)
    }
}

impl LayoutGridContainer for Compute<'_> {
    type GridContainerStyle<'a>
        = OctaraStyleRef<'a>
    where
        Self: 'a;
    type GridItemStyle<'a>
        = OctaraStyleRef<'a>
    where
        Self: 'a;

    fn get_grid_container_style(&self, node: taffy::NodeId) -> OctaraStyleRef<'_> {
        self.style(node)
    }

    fn get_grid_child_style(&self, node: taffy::NodeId) -> OctaraStyleRef<'_> {
        self.style(node)
    }

    fn set_detailed_grid_info(&mut self, node: taffy::NodeId, info: DetailedGridInfo<String>) {
        self.kernel.grid_info.insert(octara_id(node), DetailedLayoutInfo::Grid(Box::new(info)));
    }
}

fn native_id(node: NodeId) -> taffy::NodeId {
    node.data().as_ffi().into()
}

fn octara_id(node: taffy::NodeId) -> NodeId {
    KeyData::from_ffi(node.into()).into()
}

pub(super) fn effective_style<'a>(tree: &'a Tree, node: NodeId, fits: &SecondaryMap<NodeId, Fit>, window: Rect) -> OctaraStyleRef<'a> {
    OctaraStyleRef::new(layout_style(tree, node), overrides(tree, node, fits, window))
}

fn overrides(tree: &Tree, node: NodeId, fits: &SecondaryMap<NodeId, Fit>, window: Rect) -> StyleOverride {
    let style = layout_style(tree, node);
    let mut adjusted = StyleOverride::for_style(style);
    let full = Some(LayoutLength::Pct(1.0));
    if node == tree.root() || node == tree.overlay_root() {
        adjusted.size = Size::new(full, full);
        adjusted.flex_direction = FlexDirection::Column;
    }
    if node == tree.overlay_root() || tree.overlay(node).is_some_and(|overlay| overlay.target.is_some()) {
        let zero = Some(LayoutLength::Px(0.0));
        adjusted.position = Some(Position::Absolute);
        adjusted.inset = Edges { left: zero, top: zero, ..Edges::default() };
    }
    if let Some(&fit) = fits.get(node) {
        if let Some(width) = fit.width {
            limit_axis(width, window.w, &mut adjusted.min_size.width, &mut adjusted.max_size.width);
            adjusted.overflow[0] = Overflow::Scroll;
        }
        if let Some(height) = fit.height {
            limit_axis(height, window.h, &mut adjusted.min_size.height, &mut adjusted.max_size.height);
            adjusted.overflow[1] = Overflow::Scroll;
        }
    }
    adjusted
}

fn limit_axis(limit: f32, available: f32, min: &mut Option<LayoutLength>, max: &mut Option<LayoutLength>) {
    let resolve = |length| match length {
        LayoutLength::Px(px) => px,
        LayoutLength::Pct(fraction) => fraction * available,
    };
    if min.is_some_and(|min| resolve(min) > limit) {
        *min = Some(LayoutLength::Px(limit));
    }
    if max.is_none_or(|max| resolve(max) > limit) {
        *max = Some(LayoutLength::Px(limit));
    }
}
