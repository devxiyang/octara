use crate::ShellEvent;
use block2::RcBlock;
use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2::runtime::{NSObjectProtocol, ProtocolObject};
use objc2_app_kit::{NSApplication, NSApplicationDidBecomeActiveNotification, NSApplicationDidResignActiveNotification};
use objc2_foundation::NSNotificationCenter;
use octara::module::HostLifecycleEvent;
use winit::event_loop::EventLoopProxy;

pub(super) struct ActivationObserver {
    center: Retained<NSNotificationCenter>,
    tokens: [Retained<ProtocolObject<dyn NSObjectProtocol>>; 2],
}

impl ActivationObserver {
    pub(super) fn new(proxy: EventLoopProxy<ShellEvent>) -> Option<Self> {
        let app = NSApplication::sharedApplication(MainThreadMarker::new()?);
        // SAFETY: These are the system application and its static activation notification names.
        let (center, notifications) = unsafe {
            (
                NSNotificationCenter::defaultCenter(),
                [
                    (NSApplicationDidBecomeActiveNotification, HostLifecycleEvent::AppForeground),
                    (NSApplicationDidResignActiveNotification, HostLifecycleEvent::AppBackground),
                ],
            )
        };
        let tokens = notifications.map(|(name, event)| {
            let proxy = proxy.clone();
            let callback = RcBlock::new(move |_| {
                let _ = proxy.send_event(ShellEvent::Lifecycle(event.clone()));
            });
            // SAFETY: The block forwards through the thread-safe proxy and the observer is removed on drop.
            unsafe { center.addObserverForName_object_queue_usingBlock(Some(name), Some(&app), None, &callback) }
        });
        Some(Self { center, tokens })
    }
}

impl Drop for ActivationObserver {
    fn drop(&mut self) {
        for token in &self.tokens {
            // SAFETY: Each token was registered with this notification center.
            unsafe { self.center.removeObserver((**token).as_ref()) };
        }
    }
}
