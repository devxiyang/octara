use crate::ShellEvent;
use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::{NSObjectProtocol, ProtocolObject};
use objc2_foundation::{NSLocale, NSNotificationCenter, NSNotificationName};
use std::sync::Arc;
use winit::event_loop::EventLoopProxy;

pub(super) fn locale() -> Option<Arc<str>> {
    NSLocale::preferredLanguages().firstObject().map(|language| Arc::from(language.to_string()))
}

pub(super) struct Observer {
    center: Retained<NSNotificationCenter>,
    tokens: Vec<Retained<ProtocolObject<dyn NSObjectProtocol>>>,
}

impl Observer {
    pub(super) fn new(center: Retained<NSNotificationCenter>, names: &[&NSNotificationName], proxy: EventLoopProxy<ShellEvent>) -> Self {
        let callback = RcBlock::new(move |_| {
            let _ = proxy.send_event(ShellEvent::RefreshSystemPreferences);
        });
        let tokens = names
            .iter()
            .map(|&name| {
                // SAFETY: The notification callback only sends through the thread-safe event proxy.
                unsafe { center.addObserverForName_object_queue_usingBlock(Some(name), None, None, &callback) }
            })
            .collect();
        Self { center, tokens }
    }
}

impl Drop for Observer {
    fn drop(&mut self) {
        for token in &self.tokens {
            // SAFETY: Each token was returned by this notification center.
            unsafe { self.center.removeObserver((**token).as_ref()) };
        }
    }
}
