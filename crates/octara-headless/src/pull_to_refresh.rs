//! Scope-owned state for pull-to-refresh on a vertical scrollport.

use octara::prelude::*;

#[derive(Clone, Copy)]
pub struct PullToRefresh {
    threshold: f32,
    distance: Signal<f32>,
    armed: Signal<bool>,
    refreshing: Signal<bool>,
}

impl PullToRefresh {
    /// Creates a controller with a threshold in logical pixels of resisted pull.
    pub fn new(build: &mut BuildCx<'_>, threshold: f32) -> Self {
        assert!(threshold.is_finite() && threshold > 0.0, "refresh threshold must be finite and positive");
        Self { threshold, distance: build.signal(0.0), armed: build.signal(false), refreshing: build.signal(false) }
    }

    /// The visible pull distance, including the rebound after release.
    pub fn distance(self, read: &mut ReadCx<'_>) -> f32 {
        read.get(self.distance).max(0.0)
    }

    pub fn threshold(self) -> f32 {
        self.threshold
    }

    pub fn is_refreshing(self, read: &mut ReadCx<'_>) -> bool {
        read.get(self.refreshing)
    }

    /// Ends the loading state when the caller's refresh work completes.
    pub fn finish(self, write: &mut WriteCx<'_>) {
        write.set_if_changed(self.refreshing, false);
    }

    pub fn attach<C>(self, scrollport: El<C>, refresh: impl Fn(&mut WriteCx<'_>) + 'static) -> El<C> {
        self.attach_with_haptic(scrollport, refresh, |_| {})
    }

    /// Calls `threshold_haptic` when a pull crosses the threshold upward.
    /// The caller can issue a platform haptics command from this hook.
    pub fn attach_with_haptic<C>(
        self,
        scrollport: El<C>,
        refresh: impl Fn(&mut WriteCx<'_>) + 'static,
        threshold_haptic: impl Fn(&mut WriteCx<'_>) + 'static,
    ) -> El<C> {
        scrollport.observe_overscroll(self.distance).on_overscroll(move |write, event| {
            let armed = !write.get(self.refreshing) && event.offset >= self.threshold;
            match event.phase {
                GesturePhase::Started | GesturePhase::Changed => {
                    if write.set_if_changed(self.armed, armed) && armed {
                        threshold_haptic(write);
                    }
                }
                GesturePhase::Ended => {
                    write.set_if_changed(self.armed, false);
                    if armed {
                        write.set(self.refreshing, true);
                        refresh(write);
                    }
                }
                GesturePhase::Cancelled => {
                    write.set_if_changed(self.armed, false);
                }
            }
        })
    }
}
