//! The snapshot requested for each continuous period of overlay occlusion.

use crate::{layout::LayoutKernel, paint_order};
use octara_model::geometry::Rect;
use octara_model::identity::{NativeId, NodeId, TreeId};
use octara_model::image::{Image, Sample};
use octara_model::native::{NativeSlot, NativeSnapshot, NativeSnapshotRequest};
use octara_runtime::module::{ModuleDispatcher, NativeViewMounts};
use octara_tree::Tree;
use rustc_hash::FxHashMap;

#[derive(Default)]
pub(crate) struct Airspace {
    serial: u64,
    captures: FxHashMap<NodeId, Capture>,
}

struct Capture {
    request: NativeSnapshotRequest,
    props_rev: Option<u64>,
    retained_for_presentation: bool,
    state: CaptureState,
}

enum CaptureState {
    Pending,
    Captured(Sample),
    Failed,
}

impl Capture {
    fn matches(&self, tree: TreeId, props_rev: u64) -> bool {
        self.request.tree == tree && self.props_rev.is_none_or(|rev| rev == props_rev)
    }
}

pub(crate) struct AirspaceFrame {
    tree: TreeId,
    candidates: Vec<(NodeId, u64)>,
    pub images: FxHashMap<NodeId, Sample>,
}

impl Airspace {
    pub fn project(&self, tree: &Tree, layout: &LayoutKernel) -> AirspaceFrame {
        let mut frame = AirspaceFrame { tree: tree.id(), candidates: Vec::new(), images: FxHashMap::default() };
        let root = tree.overlay_root();
        if tree.children(root).is_empty() || !tree.native_nodes().any(|node| snapshot_props(tree, node).is_some()) {
            return frame;
        }
        // Each overlay contributes its visible bounds in paint order. A view
        // inside an overlay can only be occluded by a later overlay.
        let overlays: Vec<_> = paint_order(tree, root).iter().map(|&node| (node, shown_bounds(tree, layout, node))).collect();
        if overlays.iter().all(|(_, bounds)| bounds.is_none()) {
            return frame;
        }
        for node in tree.native_nodes() {
            let Some(props_rev) = snapshot_props(tree, node) else { continue };
            let Some(bounds) = shown_bounds(tree, layout, node) else { continue };
            for (overlay, cover) in overlays.iter().rev() {
                if tree.is_within(node, *overlay) {
                    break;
                }
                if cover.is_some_and(|rect| !bounds.intersect(&rect).is_empty()) {
                    frame.candidates.push((node, props_rev));
                    if let Some(capture) = self.captures.get(&node).filter(|capture| capture.matches(frame.tree, props_rev))
                        && let CaptureState::Captured(image) = &capture.state
                    {
                        frame.images.insert(node, image.clone());
                    }
                    break;
                }
            }
        }
        frame.candidates.sort_unstable_by_key(|&(node, _)| node);
        frame
    }

    pub fn commit(&mut self, frame: &AirspaceFrame) {
        self.captures.retain(|node, capture| {
            let Ok(index) = frame.candidates.binary_search_by_key(node, |&(node, _)| node) else {
                // A skipped publication must not discard the image of a view
                // that a submitted or displayed frame still hides.
                return capture.request.tree == frame.tree && capture.retained_for_presentation;
            };
            if !capture.matches(frame.tree, frame.candidates[index].1) {
                return false;
            }
            if frame.images.contains_key(node) {
                // The first committed image stays frozen through ordinary view updates.
                capture.props_rev = None;
            }
            true
        });
    }

    pub fn retain_for_presentation(&mut self, id: NativeId) {
        if let Some(capture) = self.captures.get_mut(&id.node())
            && matches!(capture.state, CaptureState::Captured(_))
        {
            capture.retained_for_presentation = true;
        }
    }

    pub fn remove(&mut self, id: NativeId) {
        self.captures.remove(&id.node());
    }

    pub fn start(&mut self, frame: &AirspaceFrame, mounts: &mut NativeViewMounts, dispatcher: &ModuleDispatcher) -> bool {
        let mut started = false;
        for &(node, props_rev) in &frame.candidates {
            let id = NativeId::from(node);
            if self.captures.contains_key(&node) || !mounts.can_snapshot_on_occlude(id) {
                continue;
            }
            self.serial = self.serial.checked_add(1).expect("native snapshot identity exhausted");
            let request = NativeSnapshotRequest { tree: frame.tree, id, request: self.serial };
            self.captures.insert(
                node,
                Capture { request, props_rev: Some(props_rev), retained_for_presentation: false, state: CaptureState::Pending },
            );
            started |= mounts.snapshot(request, dispatcher);
        }
        started
    }

    pub fn resolve(&mut self, request: NativeSnapshotRequest, snapshot: &Result<NativeSnapshot, String>) {
        let Some(capture) = self.captures.get_mut(&request.id.node()) else { return };
        if capture.request != request || !matches!(capture.state, CaptureState::Pending) {
            return;
        }
        capture.state = match snapshot {
            Ok(snapshot) => {
                let picture = &snapshot.picture;
                CaptureState::Captured(Image::rgba(picture.width(), picture.height(), picture.rgba()).sample())
            }
            Err(_) => CaptureState::Failed,
        };
    }
}

fn snapshot_props(tree: &Tree, node: NodeId) -> Option<u64> {
    let native = tree.native(node)?;
    (native.slot == NativeSlot::Overlay && native.typed_props.is_some()).then_some(native.props_rev)
}

fn shown_bounds(tree: &Tree, layout: &LayoutKernel, node: NodeId) -> Option<Rect> {
    if !tree.is_shown(node) || tree.visual(node).opacity <= 0.0 {
        return None;
    }
    let rect = layout.bounds(node)?.intersect(&layout.clip(node)?);
    (!rect.is_empty()).then_some(rect)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn omitted_publication_keeps_the_image_that_still_hides_a_native_view() {
        let tree = TreeId::new([1, 2], 3);
        let id = NativeId(1);
        let request = NativeSnapshotRequest { tree, id, request: 1 };
        let captured = Image::rgba(1, 1, [10, 20, 30, 255]).sample();
        let mut airspace = Airspace::default();
        airspace.captures.insert(
            id.node(),
            Capture { request, props_rev: None, retained_for_presentation: false, state: CaptureState::Captured(captured) },
        );
        let omitted = AirspaceFrame { tree, candidates: Vec::new(), images: FxHashMap::default() };
        airspace.retain_for_presentation(id);
        airspace.commit(&omitted);
        assert!(airspace.captures.contains_key(&id.node()));
        airspace.remove(id);
        assert!(airspace.captures.is_empty());
    }
}
