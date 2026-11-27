use crate::ShellEvent;
use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::{NSObjectProtocol, ProtocolObject};
use objc2_app_kit::{NSWorkspace, NSWorkspaceAccessibilityDisplayOptionsDidChangeNotification};
use objc2_foundation::NSNotificationCenter;
use winit::event_loop::EventLoopProxy;

pub(super) struct Observer {
    center: Retained<NSNotificationCenter>,
    token: Retained<ProtocolObject<dyn NSObjectProtocol>>,
}

impl Observer {
    pub(super) fn new(proxy: EventLoopProxy<ShellEvent>) -> Self {
        let center = NSWorkspace::sharedWorkspace().notificationCenter();
        let callback = RcBlock::new(move |_| {
            let _ = proxy.send_event(ShellEvent::RefreshReducedMotion);
        });
        // SAFETY: The notification is posted by NSWorkspace and the callback only uses the thread-safe event proxy.
        let token = unsafe {
            center.addObserverForName_object_queue_usingBlock(
                Some(NSWorkspaceAccessibilityDisplayOptionsDidChangeNotification),
                None,
                None,
                &callback,
            )
        };
        Self { center, token }
    }
}

impl Drop for Observer {
    fn drop(&mut self) {
        // SAFETY: This is the token returned by the same notification center.
        unsafe { self.center.removeObserver((*self.token).as_ref()) };
    }
}
