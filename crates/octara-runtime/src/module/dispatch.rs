//! Cross-thread module messages. The runtime owns the single receiving end.

use std::any::{Any, TypeId};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex, Weak};

use octara_model::identity::{NativeId, WindowId};
use octara_model::native::{NativeSnapshot, NativeSnapshotRequest, TypedNativeEvent};

use super::{CallTicket, EventMetadata, HostLifecycleEvent, ModuleCall, ModuleError, ModuleEvent};

const CAPACITY: usize = 1024;

pub(crate) type HostWake = Arc<dyn Fn() + Send + Sync>;

pub(super) enum Message {
    Reply { ticket: CallTicket, call: Option<TypeId>, result: Result<Box<dyn Any + Send>, ModuleError> },
    Event { metadata: EventMetadata, payload: Box<dyn Any + Send> },
    Lifecycle(HostLifecycleEvent),
    NativeEvent { window: WindowId, id: NativeId, event: TypedNativeEvent, mount: Option<Weak<()>> },
    NativeSnapshot { request: NativeSnapshotRequest, snapshot: NativeSnapshot },
}

#[derive(Default)]
struct Inbox {
    pending: VecDeque<Message>,
    wake: Option<HostWake>,
    closed: bool,
}

/// Bounded, thread-safe ingress for platform callbacks. A full queue returns
/// [`ModuleError::Busy`]. A dispatcher whose app shut down returns
/// [`ModuleError::Cancelled`].
#[derive(Clone, Default)]
pub struct ModuleDispatcher {
    inbox: Arc<Mutex<Inbox>>,
}

impl ModuleDispatcher {
    pub fn resolve<C: ModuleCall>(&self, ticket: CallTicket, result: Result<C::Output, ModuleError>) -> Result<(), ModuleError>
    where
        C::Output: Send,
    {
        self.push(Message::Reply {
            ticket,
            call: Some(TypeId::of::<C>()),
            result: result.map(|value| Box::new(value) as Box<dyn Any + Send>),
        })
    }

    pub fn resolve_erased(&self, ticket: CallTicket, result: Result<Box<dyn Any + Send>, ModuleError>) -> Result<(), ModuleError> {
        self.push(Message::Reply { ticket, call: None, result })
    }

    pub fn emit<E: ModuleEvent + Send>(&self, event: E) -> Result<(), ModuleError> {
        self.push(Message::Event { metadata: EventMetadata::of::<E>(), payload: Box::new(event) })
    }

    pub fn dispatch_lifecycle(&self, event: HostLifecycleEvent) -> Result<(), ModuleError> {
        self.push(Message::Lifecycle(event))
    }

    pub fn emit_native_event(&self, window: WindowId, id: NativeId, event: TypedNativeEvent) -> Result<(), ModuleError> {
        self.push(Message::NativeEvent { window, id, event, mount: None })
    }

    pub(super) fn emit_mounted_event(
        &self,
        window: WindowId,
        id: NativeId,
        event: TypedNativeEvent,
        mount: Weak<()>,
    ) -> Result<(), ModuleError> {
        if mount.strong_count() == 0 {
            return Err(ModuleError::Cancelled);
        }
        self.push(Message::NativeEvent { window, id, event, mount: Some(mount) })
    }

    pub fn resolve_native_snapshot(&self, request: NativeSnapshotRequest, snapshot: NativeSnapshot) -> Result<(), ModuleError> {
        self.push(Message::NativeSnapshot { request, snapshot })
    }

    fn push(&self, message: Message) -> Result<(), ModuleError> {
        let wake = {
            let mut inbox = self.inbox.lock().expect("module inbox");
            if inbox.closed {
                return Err(ModuleError::Cancelled);
            }
            if inbox.pending.len() == CAPACITY {
                return Err(ModuleError::Busy);
            }
            let wake = inbox.pending.is_empty().then(|| inbox.wake.clone()).flatten();
            inbox.pending.push_back(message);
            wake
        };
        if let Some(wake) = wake {
            wake();
        }
        Ok(())
    }

    pub(super) fn drain(&self) -> VecDeque<Message> {
        std::mem::take(&mut self.inbox.lock().expect("module inbox").pending)
    }

    pub(crate) fn set_wake(&self, wake: Option<HostWake>) {
        let (retired, notify) = {
            let mut inbox = self.inbox.lock().expect("module inbox");
            if inbox.closed {
                return;
            }
            let notify = (!inbox.pending.is_empty()).then(|| wake.clone()).flatten();
            (std::mem::replace(&mut inbox.wake, wake), notify)
        };
        drop(retired);
        if let Some(notify) = notify {
            notify();
        }
    }

    pub(super) fn wake(&self) {
        let wake = self.inbox.lock().expect("module inbox").wake.clone();
        if let Some(wake) = wake {
            wake();
        }
    }

    pub(super) fn close(&self) {
        let retired = {
            let mut inbox = self.inbox.lock().expect("module inbox");
            inbox.closed = true;
            (inbox.wake.take(), std::mem::take(&mut inbox.pending))
        };
        drop(retired);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::module::ModuleId;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[derive(Clone)]
    struct Ping(usize);

    impl ModuleEvent for Ping {
        const MODULE: ModuleId = ModuleId("test.ping");
        const EVENT: &'static str = "ping";
    }

    #[test]
    fn bounded_fifo_wakes_a_consumer_and_refuses_work_after_close() {
        fn assert_thread_safe<T: Clone + Send + Sync>() {}
        assert_thread_safe::<ModuleDispatcher>();

        let dispatcher = ModuleDispatcher::default();
        let wakes = Arc::new(AtomicUsize::new(0));
        let wake_count = wakes.clone();
        dispatcher.set_wake(Some(Arc::new(move || {
            wake_count.fetch_add(1, Ordering::Relaxed);
        })));

        for number in 0..CAPACITY {
            dispatcher.emit(Ping(number)).unwrap();
        }
        assert_eq!(dispatcher.emit(Ping(CAPACITY)), Err(ModuleError::Busy));
        assert_eq!(wakes.load(Ordering::Relaxed), 1);
        let drained = dispatcher.drain();
        assert_eq!(drained.len(), CAPACITY);
        for (number, message) in drained.into_iter().enumerate() {
            let Message::Event { payload, .. } = message else { panic!("expected a ping") };
            assert_eq!(payload.downcast::<Ping>().ok().unwrap().0, number);
        }
        dispatcher.emit(Ping(0)).unwrap();
        assert_eq!(wakes.load(Ordering::Relaxed), 2);
        dispatcher.close();
        assert_eq!(dispatcher.emit(Ping(1)), Err(ModuleError::Cancelled));
        assert!(dispatcher.drain().is_empty());
    }
}
