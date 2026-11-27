use crate::wgpu;
use std::future::{Future, poll_fn};
use std::pin::pin;
use std::sync::{Mutex, OnceLock};
use std::task::{Poll, Waker};

/// The first confirmed loss of one device, owned by that device's generation.
#[derive(Default)]
pub struct DeviceLoss {
    evidence: OnceLock<(wgpu::DeviceLostReason, String)>,
    waiter: Mutex<Option<Waker>>,
}

impl DeviceLoss {
    pub fn record(&self, reason: wgpu::DeviceLostReason, message: String) -> bool {
        if self.evidence.set((reason, message)).is_err() {
            return false;
        }
        let waiter = self.waiter.lock().expect("device loss wait").take();
        if let Some(waiter) = waiter {
            waiter.wake();
        }
        true
    }

    pub fn get(&self) -> Option<&(wgpu::DeviceLostReason, String)> {
        self.evidence.get()
    }

    /// Wait for the device's single active job, or drop its future on confirmed loss.
    pub async fn until_lost<T>(&self, work: impl Future<Output = T>) -> Option<T> {
        let mut work = pin!(work);
        let _clear_waiter = std::mem::DropGuard::new(self, |loss| {
            let waiter = loss.waiter.lock().expect("device loss wait").take();
            drop(waiter);
        });
        poll_fn(|cx| {
            {
                let mut waiter = self.waiter.lock().expect("device loss wait");
                if self.get().is_some() {
                    return Poll::Ready(None);
                }
                if waiter.as_ref().is_none_or(|waker| !waker.will_wake(cx.waker())) {
                    *waiter = Some(cx.waker().clone());
                }
            }
            work.as_mut().poll(cx).map(|value| self.get().is_none().then_some(value))
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::future::{pending, ready};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::task::{Context, Wake};

    #[derive(Default)]
    struct Wakes(AtomicUsize);

    impl Wake for Wakes {
        fn wake(self: Arc<Self>) {
            self.0.fetch_add(1, Ordering::Relaxed);
        }
    }

    #[test]
    fn loss_wakes_pending_work_and_preserves_the_first_evidence() {
        let device = DeviceLoss::default();
        let replacement = DeviceLoss::default();
        let wakes = Arc::new(Wakes::default());
        let waker = Waker::from(wakes.clone());
        let mut cx = Context::from_waker(&waker);
        let mut work = pin!(device.until_lost(pending::<()>()));
        assert_eq!(work.as_mut().poll(&mut cx), Poll::Pending);
        assert!(device.record(wgpu::DeviceLostReason::Destroyed, "first".into()));
        assert!(!device.record(wgpu::DeviceLostReason::Unknown, "late".into()));
        assert_eq!(wakes.0.load(Ordering::Relaxed), 1);
        assert_eq!(work.as_mut().poll(&mut cx), Poll::Ready(None));
        assert_eq!(device.get().unwrap().1, "first");
        assert_eq!(pin!(replacement.until_lost(ready(42))).poll(&mut cx), Poll::Ready(Some(42)));
    }

    #[test]
    fn dropping_active_work_releases_its_waker() {
        let device = DeviceLoss::default();
        let wakes = Arc::new(Wakes::default());
        let waker = Waker::from(wakes.clone());
        let mut cx = Context::from_waker(&waker);
        {
            let mut work = pin!(device.until_lost(pending::<()>()));
            assert_eq!(work.as_mut().poll(&mut cx), Poll::Pending);
        }
        assert!(device.record(wgpu::DeviceLostReason::Destroyed, "retired".into()));
        assert_eq!(wakes.0.load(Ordering::Relaxed), 0);
    }
}
