use super::super::test_support::{DroppedResult, WakeCounter};
use super::*;
use crate::app::App;
use crate::platform::HeadlessPlatform;
use std::sync::atomic::AtomicUsize;

#[test]
fn channel_close_releases_payloads_and_waker_while_senders_remain() {
    let dropped = Arc::new(AtomicUsize::new(0));
    let (tx, mut rx) = channel::<DroppedResult>();
    let wake = Arc::new(WakeCounter(AtomicUsize::new(0)));
    let waker = Waker::from(wake.clone());
    let mut receive = rx.recv();
    assert!(Pin::new(&mut receive).poll(&mut Context::from_waker(&waker)).is_pending());
    drop(waker);
    assert_eq!(Arc::strong_count(&wake), 2);
    drop(receive);
    drop(rx);
    assert_eq!(Arc::strong_count(&wake), 1);
    assert_eq!(wake.0.load(Ordering::Relaxed), 0);
    drop(tx.send(DroppedResult(dropped.clone())).err().unwrap());
    assert_eq!(dropped.load(Ordering::Relaxed), 1);

    let (tx, rx) = channel::<DroppedResult>();
    for _ in 0..32 {
        assert!(tx.send(DroppedResult(dropped.clone())).is_ok());
    }
    drop(rx);
    assert_eq!(dropped.load(Ordering::Relaxed), 33);
    assert!(tx.chan.state.lock().unwrap().queue.is_empty());
}

#[test]
fn cancelling_a_receive_releases_its_waker_and_preserves_fifo_for_the_next_wait() {
    let (tx, mut rx) = channel::<u32>();
    let old = Arc::new(WakeCounter(AtomicUsize::new(0)));
    let waker = Waker::from(old.clone());
    let mut receive = rx.recv();
    assert!(Pin::new(&mut receive).poll(&mut Context::from_waker(&waker)).is_pending());
    drop(waker);
    assert_eq!(Arc::strong_count(&old), 2);
    drop(receive);
    assert_eq!(Arc::strong_count(&old), 1);
    tx.send(7).unwrap();
    tx.send(8).unwrap();
    assert_eq!(old.0.load(Ordering::Relaxed), 0);
    assert_eq!(Pin::new(&mut rx.recv()).poll(&mut Context::from_waker(Waker::noop())), Poll::Ready(Some(7)));
    assert_eq!(Pin::new(&mut rx.recv()).poll(&mut Context::from_waker(Waker::noop())), Poll::Ready(Some(8)));

    let next = Arc::new(WakeCounter(AtomicUsize::new(0)));
    let waker = Waker::from(next.clone());
    let mut receive = rx.recv();
    assert!(Pin::new(&mut receive).poll(&mut Context::from_waker(&waker)).is_pending());
    drop(tx);
    assert_eq!(next.0.load(Ordering::Relaxed), 1, "last sender wakes the current waiter");
    assert_eq!(Pin::new(&mut receive).poll(&mut Context::from_waker(&waker)), Poll::Ready(None));
}

#[test]
fn cancelling_after_delivery_keeps_the_unconsumed_message() {
    let (tx, mut rx) = channel::<u32>();
    let wake = Arc::new(WakeCounter(AtomicUsize::new(0)));
    let waker = Waker::from(wake.clone());
    let mut receive = rx.recv();
    assert!(Pin::new(&mut receive).poll(&mut Context::from_waker(&waker)).is_pending());
    tx.send(42).unwrap();
    assert_eq!(wake.0.load(Ordering::Relaxed), 1);
    drop(receive);
    assert_eq!(Pin::new(&mut rx.recv()).poll(&mut Context::from_waker(Waker::noop())), Poll::Ready(Some(42)));
}

#[test]
fn send_racing_close_leaves_no_queued_values() {
    let dropped = Arc::new(AtomicUsize::new(0));
    for _ in 0..32 {
        let (tx, rx) = channel::<DroppedResult>();
        let gate = Arc::new(std::sync::Barrier::new(2));
        std::thread::scope(|scope| {
            let gate2 = gate.clone();
            let dropped = dropped.clone();
            let tx = &tx;
            scope.spawn(move || {
                gate2.wait();
                for _ in 0..32 {
                    drop(tx.send(DroppedResult(dropped.clone())));
                }
            });
            gate.wait();
            drop(rx);
        });
        assert!(tx.chan.state.lock().unwrap().queue.is_empty());
    }
    assert_eq!(dropped.load(Ordering::Relaxed), 32 * 32);
}

#[test]
fn named_channel_replacement_and_cross_thread_close_release_registrations() {
    let app = App::new(HeadlessPlatform::default());
    app.run(|| {
        let mut receivers = Vec::new();
        for _ in 0..32 {
            receivers.push(channel::<u32>().1.named("same"));
            assert_eq!(sched().names.borrow().len(), 1);
            rt(|r| assert_eq!(r.cleanup_count(r.current_scope().unwrap()), 1));
        }
        // Retiring an old receiver must not unregister its replacement.
        let last = receivers.pop().unwrap();
        drop(receivers);
        sched().retire_names();
        assert!(sched().send_named("same", "7").is_ok());
        assert_eq!(last.chan.state.lock().unwrap().queue.pop_front(), Some(7));
        std::thread::spawn(move || drop(last)).join().unwrap();
    });
    app.run(|| {
        assert!(sched().names.borrow().is_empty());
        rt(|r| assert_eq!(r.cleanup_count(r.current_scope().unwrap()), 0));
    });
    app.shutdown();
}

#[test]
fn renaming_and_scope_exit_release_the_named_sender_even_if_the_receiver_survives() {
    let app = App::new(HeadlessPlatform::default());
    app.run(|| {
        let scope = rt(|r| r.create_scope(r.current_scope(), "named"));
        let mut receiver = rt(|r| r.with_scope(Some(scope), || channel::<u32>().1.named("old").named("new")));
        assert!(sched().send_named("old", "1").is_err());
        rt(|r| assert_eq!(r.cleanup_count(scope), 1));
        rt(|r| r.dispose_scope(scope));
        assert!(sched().names.borrow().is_empty());
        assert_eq!(Pin::new(&mut receiver.recv()).poll(&mut Context::from_waker(Waker::noop())), Poll::Ready(None));
    });
    app.shutdown();
}

#[test]
fn closing_a_channel_drops_user_values_outside_its_lock() {
    struct Reentrant(Sender<Reentrant>, Arc<AtomicUsize>);
    impl Drop for Reentrant {
        fn drop(&mut self) {
            let state = self.0.chan.state.try_lock().expect("user destructor must be free to reenter the channel");
            assert!(!state.receiver_alive);
            self.1.fetch_add(1, Ordering::Relaxed);
        }
    }
    let (tx, rx) = channel::<Reentrant>();
    let dropped = Arc::new(AtomicUsize::new(0));
    assert!(tx.send(Reentrant(tx.clone(), dropped.clone())).is_ok());
    drop(rx);
    assert_eq!(dropped.load(Ordering::Relaxed), 1);
    assert!(tx.chan.state.lock().unwrap().queue.is_empty());
}
