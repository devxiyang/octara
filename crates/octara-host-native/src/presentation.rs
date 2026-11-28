//! Surface and frame identities carried across asynchronous presentation.

use octara_model::capture::PaintSample;
use octara_model::identity::{FrameStamp, NativeId, TreeId};
use octara_model::native::NativePlacement;
use std::collections::{BTreeMap, btree_map::Entry};
use std::time::Duration;

#[derive(Default)]
pub(crate) struct Presentation {
    surface_generation: u64,
    last_sample: Option<(u64, Duration)>,
    accepted: u64,
    last_presented_rev: u64,
    surface_size: Option<[u32; 2]>,
    rebuild_surface: bool,
}

impl Presentation {
    pub fn configure(&mut self, size: [u32; 2]) {
        self.surface_generation = self.surface_generation.checked_add(1).expect("surface generation exhausted");
        self.surface_size = Some(size);
        self.rebuild_surface = false;
        self.last_sample = None;
    }

    pub fn invalidate(&mut self, rebuild: bool) {
        self.surface_size = None;
        self.rebuild_surface |= rebuild;
        self.last_sample = None;
    }

    pub fn ready(&self, size: [u32; 2]) -> bool {
        self.surface_size == Some(size)
    }
    pub fn rebuild(&self) -> bool {
        self.rebuild_surface
    }

    pub fn needs_draw(&self, revision: u64, at: Duration) -> bool {
        self.last_sample != Some((revision, at))
    }

    pub fn presented_revision(&self, revision: u64) -> bool {
        self.last_sample.is_some_and(|(presented, _)| presented == revision)
    }

    pub fn stamp(&self, tree: TreeId, generation: u64, revision: u64, at: Duration) -> FrameStamp {
        FrameStamp {
            tree,
            frame: self.accepted.checked_add(1).expect("presentation frame exhausted"),
            surface_generation: self.surface_generation,
            device_generation: generation,
            native_rev: revision,
            sample: PaintSample { revision, sampled_at: at },
        }
    }

    pub fn accepts(&self, stamp: FrameStamp, tree: TreeId, generation: u64, size: [u32; 2]) -> bool {
        self.ready(size)
            && stamp.tree == tree
            && stamp.frame > self.accepted
            && stamp.surface_generation == self.surface_generation
            && stamp.device_generation == generation
            && stamp.native_rev >= self.last_presented_rev
    }

    /// Records a successfully presented frame. Returns whether it is the window's first.
    pub fn did_present(&mut self, stamp: FrameStamp) -> bool {
        let first = self.accepted == 0;
        self.last_sample = Some((stamp.sample.revision, stamp.sample.sampled_at));
        self.accepted = stamp.frame;
        self.last_presented_rev = stamp.native_rev;
        first
    }
}

/// Coalesce placements and keep removed views alive while a frame can still show them.
/// Each window can have at most one frame in flight.
pub(crate) struct PendingNativeViews<View> {
    prepared_rev: Option<u64>,
    prepared: BTreeMap<NativeId, NativeViewChange>,
    next: BTreeMap<NativeId, NativeViewChange>,
    prepared_removals: Vec<View>,
    retiring: Vec<View>,
    retiring_changes: BTreeMap<NativeId, RetiringView<View>>,
}

#[derive(Default)]
pub(crate) struct NativeViewChange {
    pub placement: Option<NativePlacement>,
    pub snapshot_hidden: Option<bool>,
}

struct RetiringView<View> {
    view: View,
    placed: bool,
}

impl<View> Default for PendingNativeViews<View> {
    fn default() -> Self {
        Self {
            prepared_rev: None,
            prepared: BTreeMap::new(),
            next: BTreeMap::new(),
            prepared_removals: Vec::new(),
            retiring: Vec::new(),
            retiring_changes: BTreeMap::new(),
        }
    }
}

impl<View> PendingNativeViews<View> {
    pub fn insert(&mut self, placement: NativePlacement) {
        let change = self.next.entry(placement.id).or_default();
        change.placement = Some(placement);
    }

    pub fn set_snapshot_visibility(&mut self, id: NativeId, hidden: bool) {
        self.next.entry(id).or_default().snapshot_hidden = Some(hidden);
    }

    pub fn retire(&mut self, id: NativeId, view: View, placed: bool) {
        self.next.remove(&id);
        // The admitted frame may change this exact instance,
        // even if another view with the same id has since been created.
        if self.prepared.contains_key(&id)
            && let Entry::Vacant(entry) = self.retiring_changes.entry(id)
        {
            entry.insert(RetiringView { view, placed });
        } else if placed {
            self.retiring.push(view);
        }
    }

    /// Freeze placements through the synchronized revision. If a draw was
    /// abandoned, carry its unchanged placements and displayed removals forward.
    pub fn prepare(&mut self, revision: u64) -> bool {
        debug_assert!(self.prepared_rev.is_none_or(|previous| previous <= revision));
        self.prepared_rev = Some(revision);
        self.discard_retired_changes();
        self.merge_next();
        self.prepared_removals.append(&mut self.retiring);
        !self.prepared.is_empty() || !self.prepared_removals.is_empty()
    }

    pub fn abandon(&mut self) {
        if self.prepared_rev.take().is_none() {
            return;
        }
        self.discard_retired_changes();
        self.merge_next();
        self.next = std::mem::take(&mut self.prepared);
    }

    fn merge_next(&mut self) {
        if self.prepared.is_empty() {
            self.prepared = std::mem::take(&mut self.next);
            return;
        }
        for (id, next) in std::mem::take(&mut self.next) {
            let prepared = self.prepared.entry(id).or_default();
            if next.placement.is_some() {
                prepared.placement = next.placement;
            }
            if next.snapshot_hidden.is_some() {
                prepared.snapshot_hidden = next.snapshot_hidden;
            }
        }
    }

    fn discard_retired_changes(&mut self) {
        for (id, retired) in std::mem::take(&mut self.retiring_changes) {
            self.prepared.remove(&id);
            if retired.placed {
                self.retiring.push(retired.view);
            }
        }
    }

    pub fn commit(&mut self, revision: u64, mut apply: impl FnMut(NativeId, NativeViewChange, Option<&mut View>)) {
        if self.prepared_rev != Some(revision) {
            return;
        }
        self.prepared_rev = None;
        self.prepared_removals.clear();
        for (id, change) in std::mem::take(&mut self.prepared) {
            if let Some(mut retired) = self.retiring_changes.remove(&id) {
                apply(id, change, Some(&mut retired.view));
                self.retiring.push(retired.view);
            } else {
                apply(id, change, None);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use octara_model::geometry::Rect;
    use octara_model::native::NativeSlot;

    #[test]
    fn rejects_draws_from_superseded_surface_device_and_tree() {
        let mut present = Presentation::default();
        let tree = TreeId::new([1, 2], 3);
        present.configure([160, 120]);
        let frame = present.stamp(tree, 1, 5, Duration::from_millis(10));
        assert!(present.accepts(frame, tree, 1, [160, 120]));
        assert!(!present.accepts(frame, tree, 2, [160, 120]));
        assert!(!present.accepts(frame, TreeId::new([2, 3], 3), 1, [160, 120]));
        assert!(!present.accepts(frame, tree, 1, [120, 160]));
        present.invalidate(false);
        assert!(!present.accepts(frame, tree, 1, [160, 120]));
        present.configure([160, 120]);
        assert!(!present.accepts(frame, tree, 1, [160, 120]));
        let frame = present.stamp(tree, 1, 5, Duration::from_millis(10));
        assert!(present.did_present(frame));
        assert!(!present.accepts(frame, tree, 1, [160, 120]));
        assert!(!present.needs_draw(5, Duration::from_millis(10)));
        present.invalidate(true);
        present.configure([120, 160]);
        let frame = present.stamp(tree, 2, 6, Duration::from_millis(11));
        assert!(present.accepts(frame, tree, 2, [120, 160]));
        assert!(!present.did_present(frame));
    }

    #[test]
    fn revision_floor_survives_surface_changes_and_allows_resampling() {
        let mut present = Presentation::default();
        let tree = TreeId::new([1, 2], 3);
        let size = [160, 120];
        present.configure(size);
        let frame = present.stamp(tree, 1, 5, Duration::ZERO);
        assert!(present.did_present(frame));
        present.invalidate(false);
        present.configure(size);
        let older = present.stamp(tree, 2, 4, Duration::from_millis(8));
        assert!(!present.accepts(older, tree, 2, size));
        let resampled = present.stamp(tree, 2, 5, Duration::from_millis(8));
        assert!(present.accepts(resampled, tree, 2, size));
    }

    fn placement(id: u64, x: f32) -> NativePlacement {
        let rect = Rect::new(x, 0.0, 50.0, 50.0);
        NativePlacement {
            id: NativeId(id),
            slot: NativeSlot::Underlay,
            rect,
            radius: [0.0; 4],
            clip: rect,
            clip_radius: 0.0,
            opacity: 1.0,
            visible: true,
            covered: Vec::new(),
        }
    }

    #[test]
    fn applies_placements_only_for_the_presented_revision() {
        let mut pending = PendingNativeViews::<()>::default();
        let mut committed = Vec::new();
        pending.insert(placement(1, 4.0));
        pending.insert(placement(2, 2.0));
        assert!(pending.prepare(10));

        // Two publications settle during draw 10. Their deltas must survive
        // presenting 10 and coalesce for the next admitted revision.
        pending.insert(placement(1, 8.0));
        pending.insert(placement(3, 0.0));
        pending.insert(placement(1, 12.0));
        pending.commit(12, |_, change, _| committed.push(change.placement.unwrap()));
        assert!(committed.is_empty());
        pending.commit(10, |_, change, _| committed.push(change.placement.unwrap()));
        assert_eq!(std::mem::take(&mut committed), [placement(1, 4.0), placement(2, 2.0)]);
        assert!(pending.prepare(12));
        pending.commit(12, |_, change, _| committed.push(change.placement.unwrap()));
        assert_eq!(committed, [placement(1, 12.0), placement(3, 0.0)]);
        assert!(!pending.prepare(13));
    }

    #[test]
    fn two_underlays_remain_mounted_through_a_frame_preceding_removal() {
        use std::rc::Rc;

        let mut pending = PendingNativeViews::default();
        let view = Rc::new(NativeId(1));
        let lifetime = Rc::downgrade(&view);
        let mut committed = Vec::new();
        pending.insert(placement(1, 4.0));
        pending.insert(placement(2, 52.0));
        assert!(pending.prepare(10));
        pending.retire(NativeId(1), view, false);

        pending.commit(10, |id, change, retired| {
            assert_eq!(retired.map(|view| **view), (id == NativeId(1)).then_some(id));
            committed.push(change.placement.unwrap());
        });
        assert_eq!(committed, [placement(1, 4.0), placement(2, 52.0)]);
        assert!(lifetime.upgrade().is_some());
        assert!(pending.prepare(11));
        assert!(lifetime.upgrade().is_some());
        pending.commit(11, |_, _, _| panic!("expected only a removal"));
        assert!(lifetime.upgrade().is_none());
        assert!(!pending.prepare(12));
    }

    #[test]
    fn surface_reset_releases_unplaced_view_and_carries_displayed_removal_forward() {
        use std::rc::Rc;

        let mut pending = PendingNativeViews::default();
        let displayed = Rc::new(());
        let displayed_lifetime = Rc::downgrade(&displayed);
        let unplaced = Rc::new(());
        let unplaced_lifetime = Rc::downgrade(&unplaced);
        pending.insert(placement(1, 4.0));
        pending.insert(placement(2, 2.0));
        pending.insert(placement(4, 8.0));
        assert!(pending.prepare(10));
        pending.retire(NativeId(1), displayed, true);
        pending.retire(NativeId(2), unplaced, false);
        pending.insert(placement(3, 6.0));
        pending.abandon();
        pending.commit(10, |_, _, _| panic!("abandoned frame cannot apply placements"));
        assert!(unplaced_lifetime.upgrade().is_none());
        assert!(pending.prepare(11));
        assert!(displayed_lifetime.upgrade().is_some());
        let mut committed = Vec::new();
        pending.commit(11, |_, change, _| committed.push(change.placement.unwrap()));
        assert_eq!(committed, [placement(3, 6.0), placement(4, 8.0)]);
        assert!(displayed_lifetime.upgrade().is_none());
    }

    #[test]
    fn snapshot_visibility_commits_with_its_draw_and_survives_a_rejected_draw() {
        let mut pending = PendingNativeViews::<u32>::default();
        let id = NativeId(1);
        pending.set_snapshot_visibility(id, true);
        assert!(pending.prepare(10));
        pending.set_snapshot_visibility(id, false);
        pending.insert(placement(1, 20.0));
        pending.commit(10, |key, change, _| {
            assert_eq!(key, id);
            assert_eq!(change.snapshot_hidden, Some(true));
            assert_eq!(change.placement, None);
        });
        assert!(pending.prepare(11));
        pending.insert(placement(1, 24.0));
        pending.abandon();
        assert!(pending.prepare(12));
        pending.retire(id, 42, true);
        pending.commit(12, |key, change, retired| {
            assert_eq!(key, id);
            assert_eq!(retired.copied(), Some(42));
            assert_eq!(change.snapshot_hidden, Some(false));
            assert_eq!(change.placement, Some(placement(1, 24.0)));
        });
    }
}
