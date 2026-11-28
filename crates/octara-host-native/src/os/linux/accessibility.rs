use crate::os::{PlatformEvent, PlatformSender, WindowAccessibility};
use accesskit::{ActionHandler, ActionRequest, ActivationHandler, DeactivationHandler, TreeUpdate};
use octara::WindowId;
use octara_model::{geometry::Rect, identity::TargetGuard, semantics::A11yAction};
use octara_pipeline::NativePublication;

#[derive(Clone)]
struct Handler {
    window: WindowId,
    sender: PlatformSender,
}

impl Handler {
    fn enabled(&self, enabled: bool) {
        self.sender.send_event(PlatformEvent::AccessibilityEnabledChanged { window: self.window, enabled });
    }
}

impl ActivationHandler for Handler {
    fn request_initial_tree(&mut self) -> Option<TreeUpdate> {
        self.enabled(true);
        None
    }
}

impl ActionHandler for Handler {
    fn do_action(&mut self, request: ActionRequest) {
        self.sender.send_event(PlatformEvent::AccessKitAction { window: self.window, request });
    }
}

impl DeactivationHandler for Handler {
    fn deactivate_accessibility(&mut self) {
        self.enabled(false);
    }
}

pub(super) struct Accessibility {
    adapter: accesskit_unix::Adapter,
    published: Option<u64>,
}

impl Accessibility {
    pub(super) fn new(window: WindowId, sender: PlatformSender) -> Self {
        let handler = Handler { window, sender };
        Self { adapter: accesskit_unix::Adapter::new(handler.clone(), handler.clone(), handler), published: None }
    }

    pub(super) fn set_bounds(&mut self, outer: Rect, inner: Rect) {
        let convert = |rect: Rect| accesskit::Rect::new(rect.x.into(), rect.y.into(), (rect.x + rect.w).into(), (rect.y + rect.h).into());
        self.adapter.set_root_window_bounds(convert(outer), convert(inner));
    }

    pub(super) fn set_focused(&mut self, focused: bool) {
        self.adapter.update_window_focus_state(focused);
    }
}

impl WindowAccessibility for Accessibility {
    fn synchronize(&mut self, publication: &NativePublication) {
        if let Some(previous) = self.published {
            for (version, update) in publication.take_accessibility_updates() {
                if version > previous {
                    self.adapter.update_if_active(|| update.as_ref().clone());
                    self.published = Some(version);
                }
            }
        } else if let Some(frame) = publication.latest()
            && let Some(snapshot) = publication.snapshot_full()
        {
            self.adapter.update_if_active(|| {
                publication.take_accessibility_updates();
                self.published = Some(frame.number);
                snapshot
            });
        }
    }

    fn clear(&mut self) {
        self.published = None;
    }

    fn accepts_action(&self, _: &NativePublication, _: TargetGuard, _: &A11yAction) -> bool {
        // The shell resolves AccessKit node IDs against the current publication.
        false
    }
}
