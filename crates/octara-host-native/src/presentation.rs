//! Surface and frame identities carried across asynchronous presentation.

use octara_model::capture::PaintSample;
use octara_model::identity::{FrameStamp, TreeId};
use std::time::Duration;

#[derive(Default)]
pub(crate) struct Presentation {
    surface_generation: u64,
    last_sample: Option<(u64, Duration)>,
    accepted: u64,
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

    pub fn accepts(&self, stamp: FrameStamp, tree: TreeId, generation: u64, revision: u64, size: [u32; 2]) -> bool {
        self.ready(size)
            && stamp.tree == tree
            && stamp.frame > self.accepted
            && stamp.surface_generation == self.surface_generation
            && stamp.device_generation == generation
            && stamp.native_rev == revision
    }

    /// Records a successfully presented frame. Returns whether it is the window's first.
    pub fn did_present(&mut self, stamp: FrameStamp) -> bool {
        let first = self.accepted == 0;
        self.last_sample = Some((stamp.sample.revision, stamp.sample.sampled_at));
        self.accepted = stamp.frame;
        first
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_draws_from_superseded_surface_device_and_tree() {
        let mut present = Presentation::default();
        let tree = TreeId::new([1, 2], 3);
        present.configure([160, 120]);
        let frame = present.stamp(tree, 1, 5, Duration::from_millis(10));
        assert!(present.accepts(frame, tree, 1, 5, [160, 120]));
        assert!(!present.accepts(frame, tree, 2, 5, [160, 120]));
        assert!(!present.accepts(frame, TreeId::new([2, 3], 3), 1, 5, [160, 120]));
        present.invalidate(false);
        assert!(!present.accepts(frame, tree, 1, 5, [160, 120]));
        present.configure([160, 120]);
        assert!(!present.accepts(frame, tree, 1, 5, [160, 120]));
        let frame = present.stamp(tree, 1, 5, Duration::from_millis(10));
        assert!(present.did_present(frame));
        assert!(!present.accepts(frame, tree, 1, 5, [160, 120]));
        assert!(!present.needs_draw(5, Duration::from_millis(10)));
        present.invalidate(true);
        present.configure([120, 160]);
        let frame = present.stamp(tree, 2, 6, Duration::from_millis(11));
        assert!(present.accepts(frame, tree, 2, 6, [120, 160]));
        assert!(!present.did_present(frame));
    }
}
