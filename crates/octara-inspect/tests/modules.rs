#![cfg(all(feature = "native", not(target_arch = "wasm32")))]

use std::any::Any;
use std::cell::{Cell, RefCell};
use std::future::{Future, poll_fn};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::task::Poll;
use std::time::Duration;

use octara::{
    HostReply, Input,
    module::{
        CallTicket, HostLifecycleEvent, ModuleCall, ModuleCallCx, ModuleDispatcher, ModuleError, ModuleEvent, ModuleId, ModuleInstallCx,
        ModuleLifecycleCx, ModuleObservationKind, OctaraModule,
    },
    prelude::*,
};
use octara_inspect::{HeadlessDriver, Inspector, replay::Replay};
use serde_json::{Value, json};

const MODULE: ModuleId = ModuleId("test.module");

struct ReadCount(u32);

impl ModuleCall for ReadCount {
    const MODULE: ModuleId = MODULE;
    const METHOD: &'static str = "read";
    type Output = u32;
}

struct ReadLabel(Rc<str>);

impl ModuleCall for ReadLabel {
    const MODULE: ModuleId = MODULE;
    const METHOD: &'static str = "read";
    type Output = Rc<str>;
}

#[derive(Clone, Default)]
struct TypedModule {
    installs: Rc<Cell<usize>>,
}

impl OctaraModule for TypedModule {
    fn id(&self) -> ModuleId {
        MODULE
    }

    fn install(&mut self, _: &mut ModuleInstallCx<'_>) -> Result<(), ModuleError> {
        self.installs.set(self.installs.get() + 1);
        Ok(())
    }

    fn call(&mut self, _: CallTicket, request: Box<dyn Any>, cx: &mut ModuleCallCx<'_>) {
        if request.is::<ReadCount>() {
            let ReadCount(count) = cx.downcast::<ReadCount>(request).unwrap();
            assert!(matches!(cx.reply::<ReadLabel>(Ok("wrong call type".into())), Err(ModuleError::InvalidArgs(_))));
            cx.reply::<ReadCount>(Ok(count + 1)).unwrap();
        } else {
            let ReadLabel(label) = cx.downcast::<ReadLabel>(request).unwrap();
            cx.reply::<ReadLabel>(Ok(label)).unwrap();
        }
    }
}

#[test]
fn sync_calls_dispatch_by_type_even_when_method_names_match() {
    let mut driver = HeadlessDriver::default();
    let module = TypedModule::default();
    driver.launch(AppDefinition::new().with_module(module.clone())).unwrap();
    let label: Rc<str> = Rc::from("local label");
    let retained_label = label.clone();
    let (count, received) = driver.app_mut().build(|b| (b.signal(None), b.signal(None))).unwrap();
    let (count_ticket, label_ticket) = driver
        .app_mut()
        .update(|w| {
            let count_ticket = w.call_module(ReadCount(41), move |w, result| w.set(count, Some(result.unwrap())));
            let label_ticket = w.call_module(ReadLabel(label), move |w, result| w.set(received, Some(result.unwrap())));
            (count_ticket, label_ticket)
        })
        .unwrap();
    assert_ne!(count_ticket, label_ticket);
    assert_eq!(module.installs.get(), 1);
    let (count, received) = driver.app_mut().update(|w| (w.get(count), w.get(received).unwrap())).unwrap();
    assert_eq!(count, Some(42));
    assert!(Rc::ptr_eq(&retained_label, &received));
}

#[test]
fn duplicate_module_is_rejected_before_installation() {
    let mut driver = HeadlessDriver::default();
    let module = TypedModule::default();
    let error = driver.launch(AppDefinition::new().with_module(module.clone()).with_module(module.clone())).unwrap_err();
    assert_eq!(error, AppError::DuplicateModule(MODULE));
    assert_eq!(module.installs.get(), 0);
}

struct Deferred;

impl ModuleCall for Deferred {
    const MODULE: ModuleId = MODULE;
    const METHOD: &'static str = "deferred";
    type Output = u32;
}

struct Notify(u32);

impl ModuleCall for Notify {
    const MODULE: ModuleId = MODULE;
    const METHOD: &'static str = "notify";
    type Output = ();
}

#[derive(Clone, Default)]
struct DeferredModule {
    calls: Rc<RefCell<Vec<(CallTicket, ModuleDispatcher)>>>,
    cancelled: Rc<RefCell<Vec<CallTicket>>>,
    notified: Rc<Cell<Option<u32>>>,
}

impl OctaraModule for DeferredModule {
    fn id(&self) -> ModuleId {
        MODULE
    }

    fn call(&mut self, ticket: CallTicket, request: Box<dyn Any>, cx: &mut ModuleCallCx<'_>) {
        if request.is::<Notify>() {
            let Notify(value) = cx.downcast::<Notify>(request).unwrap();
            self.notified.set(Some(value));
            cx.reply::<Notify>(Ok(())).unwrap();
        } else {
            cx.downcast::<Deferred>(request).unwrap();
            self.calls.borrow_mut().push((ticket, cx.dispatcher()));
        }
    }

    fn cancel(&mut self, ticket: CallTicket) {
        self.cancelled.borrow_mut().push(ticket);
    }
}

fn press(driver: &mut HeadlessDriver, window: WindowId) {
    let tree = driver.app().runtime().window(window).unwrap().tree();
    let [node] = tree.test_ids("call-module") else { panic!("expected one module action") };
    let target = tree.guard(*node);
    let receipt = driver.dispatch(window, Input::Host(HostReply::AccessibilityAction { target, action: A11yAction::Press })).unwrap();
    assert!(receipt.delivered && receipt.handled, "{receipt:?}");
}

#[test]
fn ui_action_notifies_module_and_receives_worker_reply_on_ingress() {
    let mut driver = HeadlessDriver::default();
    let module = DeferredModule::default();
    let ticket = Rc::new(Cell::new(None));
    let reply = Rc::new(Cell::new(None));
    let ui_ticket = ticket.clone();
    let ui_reply = reply.clone();
    let window = driver
        .launch(AppDefinition::new().with_module(module.clone()).window(WindowDefinition::new(WindowOptions::default(), move |_| {
            el().w(100).h(32).role(Role::Button).test_id("call-module").on_press(move |w| {
                w.notify_module(Notify(7));
                let reply = ui_reply.clone();
                let window = w.window_id();
                let ticket = w.call_module(Deferred, move |w, result| {
                    assert_eq!(w.window_id(), window);
                    reply.set(Some(result.unwrap()));
                });
                ui_ticket.set(Some(ticket));
            })
        })))
        .unwrap()[0];

    press(&mut driver, window);
    assert_eq!(module.notified.get(), Some(7));
    let [(pending, dispatcher)] = module.calls.take().try_into().ok().expect("expected one deferred call");
    assert_eq!(ticket.get(), Some(pending));

    let wake_count = Arc::new(AtomicUsize::new(0));
    let host_wakes = wake_count.clone();
    driver.app_mut().runtime_mut().set_wake(Some(Arc::new(move || {
        host_wakes.fetch_add(1, Ordering::Relaxed);
    })));
    let wakes_before_reply = wake_count.load(Ordering::Relaxed);
    std::thread::spawn(move || {
        dispatcher.resolve::<ReadCount>(pending, Ok(1)).unwrap();
        dispatcher.resolve_erased(pending, Ok(Box::new("mismatched output"))).unwrap();
        dispatcher.resolve::<Deferred>(pending, Ok(23)).unwrap();
    })
    .join()
    .unwrap();
    assert!(wake_count.load(Ordering::Relaxed) > wakes_before_reply);
    assert_eq!(reply.get(), None);
    driver.app_mut().runtime_mut().settle().unwrap();
    assert_eq!(reply.get(), Some(23));
    assert!(module.cancelled.borrow().is_empty());
}

#[test]
fn removing_view_and_closing_window_cancel_only_their_owned_calls() {
    let mut driver = HeadlessDriver::default();
    let module = DeferredModule::default();
    let visible = driver.app_mut().build(|b| b.signal(true)).unwrap();
    let window = driver
        .launch(AppDefinition::new().with_module(module.clone()).window(WindowDefinition::new(WindowOptions::default(), move |_| {
            show(
                move |r| r.get(visible),
                |_| {
                    el().w(100).h(32).role(Role::Button).test_id("call-module").on_press(|w| {
                        w.call_module(Deferred, |_, _| panic!("retired view received a late reply"));
                    })
                },
            )
        })))
        .unwrap()[0];
    press(&mut driver, window);
    let window_ticket =
        driver.app_mut().update_window(window, |w| w.call_module(Deferred, |_, _| panic!("closed window received a late reply"))).unwrap();
    let app_reply = Rc::new(Cell::new(None));
    let received = app_reply.clone();
    let app_ticket = driver.app_mut().update(|w| w.call_module(Deferred, move |_, result| received.set(Some(result.unwrap())))).unwrap();
    let [(view_ticket, dispatcher), (actual_window_ticket, _), (actual_app_ticket, _)] =
        module.calls.take().try_into().ok().expect("expected one call from each scope");
    assert_eq!((actual_window_ticket, actual_app_ticket), (window_ticket, app_ticket));
    assert!(module.cancelled.borrow().is_empty());

    driver.app_mut().update(|w| w.set(visible, false)).unwrap();
    assert_eq!(*module.cancelled.borrow(), [view_ticket]);
    dispatcher.resolve::<Deferred>(view_ticket, Ok(1)).unwrap();
    driver.app_mut().runtime_mut().settle().unwrap();

    driver.app_mut().close_window(window).unwrap();
    assert_eq!(*module.cancelled.borrow(), [view_ticket, window_ticket]);
    assert_eq!(app_reply.get(), None);
    dispatcher.resolve::<Deferred>(window_ticket, Ok(2)).unwrap();
    dispatcher.resolve::<Deferred>(app_ticket, Ok(3)).unwrap();
    driver.app_mut().runtime_mut().settle().unwrap();
    assert_eq!(app_reply.get(), Some(3));
    assert_eq!(*module.cancelled.borrow(), [view_ticket, window_ticket]);
}

#[test]
fn dropping_pending_task_call_cancels_while_its_task_is_still_live() {
    let mut driver = HeadlessDriver::default();
    let module = DeferredModule::default();
    driver.launch(AppDefinition::new().with_module(module.clone())).unwrap();
    let finished = Rc::new(Cell::new(None));
    let task_finished = finished.clone();
    let interval = Duration::from_millis(5);
    driver
        .app_mut()
        .build(|b| {
            b.spawn("drop module call", move |task| async move {
                {
                    let mut call = std::pin::pin!(task.call_module(Deferred));
                    poll_fn(|cx| {
                        assert!(call.as_mut().poll(cx).is_pending());
                        Poll::Ready(())
                    })
                    .await;
                    task.sleep(interval).await;
                }
                task_finished.set(Some(task.call_module(Deferred).await.unwrap()));
            });
        })
        .unwrap();
    let [(ticket, dispatcher)] = module.calls.take().try_into().ok().expect("expected one pending task call");
    assert!(module.cancelled.borrow().is_empty());
    driver.advance_to(interval).unwrap();
    assert_eq!(*module.cancelled.borrow(), [ticket]);
    let [(next_ticket, _)] = module.calls.take().try_into().ok().expect("expected the task to start a second call");
    assert_ne!(ticket, next_ticket);
    dispatcher.resolve::<Deferred>(ticket, Ok(4)).unwrap();
    driver.app_mut().runtime_mut().settle().unwrap();
    assert_eq!(finished.get(), None);
    dispatcher.resolve::<Deferred>(next_ticket, Ok(5)).unwrap();
    driver.app_mut().runtime_mut().settle().unwrap();
    assert_eq!(finished.get(), Some(5));
    assert_eq!(*module.cancelled.borrow(), [ticket]);
}

struct InspectedCall<const PRIVATE: bool>(&'static str);

impl<const PRIVATE: bool> ModuleCall for InspectedCall<PRIVATE> {
    const MODULE: ModuleId = MODULE;
    const METHOD: &'static str = "inspect";
    const SENSITIVE: bool = PRIVATE;
    type Output = &'static str;

    fn inspect_args(&self) -> Option<Value> {
        assert!(!PRIVATE, "sensitive call arguments were inspected");
        Some(json!(self.0))
    }

    fn inspect_output(value: &Self::Output) -> Option<Value> {
        assert!(!PRIVATE, "sensitive call output was inspected");
        Some(json!(value))
    }
}

#[derive(Clone)]
struct InspectedEvent<const PRIVATE: bool>(Rc<str>);

impl<const PRIVATE: bool> ModuleEvent for InspectedEvent<PRIVATE> {
    const MODULE: ModuleId = MODULE;
    const EVENT: &'static str = "changed";
    const SENSITIVE: bool = PRIVATE;

    fn inspect_payload(&self) -> Option<Value> {
        assert!(!PRIVATE, "sensitive event payload was inspected");
        Some(json!(self.0.as_ref()))
    }
}

#[test]
fn replacing_the_inspector_detaches_its_previous_trace() {
    let recording = Rc::new(RefCell::new(Replay::new([11, 29])));
    let mut driver = HeadlessDriver::default().with_inspector(Inspector::default().with_trace_recorder(recording.clone()));
    driver.emit_module_event(InspectedEvent::<false>("observed".into())).unwrap();
    assert_eq!(recording.borrow().entries().len(), 1);

    let mut driver = driver.with_inspector(Inspector::default());
    driver.emit_module_event(InspectedEvent::<false>("no longer observed".into())).unwrap();
    assert_eq!(recording.borrow().entries().len(), 1);
}

#[test]
fn host_lifecycle_drains_even_when_background_ingress_is_full() {
    let mut driver = HeadlessDriver::default();
    let visits = driver
        .app_mut()
        .build(|build| {
            let visits = build.signal(0_u32);
            build.on_lifecycle(move |write, event| {
                if *event == HostLifecycleEvent::MemoryWarning {
                    write.update(visits, |visits| *visits += 1);
                }
            });
            visits
        })
        .unwrap();
    let dispatcher = driver.app().runtime().module_dispatcher();
    let saturated = (0..4096).any(|_| dispatcher.dispatch_lifecycle(HostLifecycleEvent::AppForeground) == Err(ModuleError::Busy));
    assert!(saturated);
    assert!(driver.dispatch_lifecycle(HostLifecycleEvent::MemoryWarning).unwrap().delivered);
    assert_eq!(driver.app_mut().update(|write| write.get(visits)).unwrap(), 1);
}

#[test]
fn inspection_redacts_sensitive_calls_replies_and_events() {
    let mut driver = HeadlessDriver::default();
    driver.stub_module_call::<InspectedCall<false>>(|request| {
        assert_eq!(request.0, "public argument");
        Ok("public result")
    });
    driver.stub_module_call::<InspectedCall<true>>(|request| {
        assert_eq!(request.0, "private argument");
        Ok("private result")
    });
    let delivered = driver
        .app_mut()
        .build(|b| {
            let delivered = b.signal(Vec::new());
            b.on_module_event::<InspectedEvent<false>>(move |w, event| w.update(delivered, |seen| seen.push(event.0.to_string())));
            b.on_module_event::<InspectedEvent<true>>(move |w, event| w.update(delivered, |seen| seen.push(event.0.to_string())));
            delivered
        })
        .unwrap();

    let observations = Rc::new(RefCell::new(Vec::new()));
    let observed = observations.clone();
    driver.app_mut().runtime_mut().set_module_observer(Some(Box::new(move |entry| observed.borrow_mut().push(entry))));
    let at = Duration::from_millis(42);
    driver.advance_to(at).unwrap();
    let public = driver
        .app_mut()
        .update(|w| w.call_module(InspectedCall::<false>("public argument"), |_, result| assert_eq!(result, Ok("public result"))))
        .unwrap();
    let private = driver
        .app_mut()
        .update(|w| w.call_module(InspectedCall::<true>("private argument"), |_, result| assert_eq!(result, Ok("private result"))))
        .unwrap();
    driver.emit_module_event(InspectedEvent::<false>("public event".into())).unwrap();
    driver.emit_module_event(InspectedEvent::<true>("private event".into())).unwrap();
    assert_eq!(driver.app_mut().update(|w| w.get(delivered)).unwrap(), ["public event", "private event"]);

    use ModuleObservationKind::{Call, Event, Reply};
    let expected = [
        (Call, "inspect", Some(public), "public argument"),
        (Reply, "inspect", Some(public), "public result"),
        (Call, "inspect", Some(private), "<redacted>"),
        (Reply, "inspect", Some(private), "<redacted>"),
        (Event, "changed", None, "public event"),
        (Event, "changed", None, "<redacted>"),
    ];
    let observations = observations.borrow();
    assert_eq!(observations.len(), expected.len());
    for (entry, (kind, name, ticket, payload)) in observations.iter().zip(expected) {
        assert_eq!((entry.at, entry.kind, entry.module.as_str(), entry.name.as_str(), entry.ticket), (at, kind, MODULE.0, name, ticket));
        assert_eq!(entry.payload, Some(json!(payload)));
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct LifecycleVisit {
    listener: &'static str,
    window: Option<WindowId>,
    event: HostLifecycleEvent,
}

type LifecycleLog = Signal<Vec<LifecycleVisit>>;

fn log_lifecycle(w: &mut WriteCx<'_>, log: LifecycleLog, listener: &'static str, event: &HostLifecycleEvent) {
    if matches!(event, HostLifecycleEvent::OpenUrls(_)) {
        let visit = LifecycleVisit { listener, window: w.window_id(), event: event.clone() };
        w.update(log, |log| log.push(visit));
    }
}

struct LifecycleModule {
    id: ModuleId,
    log: LifecycleLog,
}

impl OctaraModule for LifecycleModule {
    fn id(&self) -> ModuleId {
        self.id
    }

    fn install(&mut self, cx: &mut ModuleInstallCx<'_>) -> Result<(), ModuleError> {
        if self.id.0 == "test.alpha" {
            let log = self.log;
            cx.build().on_lifecycle(move |w, event| log_lifecycle(w, log, "installed", event));
        }
        Ok(())
    }

    fn call(&mut self, _: CallTicket, _: Box<dyn Any>, _: &mut ModuleCallCx<'_>) {
        panic!("lifecycle module received a call");
    }

    fn on_lifecycle(&mut self, event: &HostLifecycleEvent, cx: &mut ModuleLifecycleCx<'_>) {
        log_lifecycle(cx.write(), self.log, self.id.0, event);
    }
}

#[test]
fn lifecycle_fans_out_deterministically_to_modules_and_live_scopes() {
    let mut driver = HeadlessDriver::default();
    let (log, visible) = driver
        .app_mut()
        .build(|b| {
            let log = b.signal(Vec::new());
            b.on_lifecycle(move |w, event| log_lifecycle(w, log, "app", event));
            (log, b.signal(true))
        })
        .unwrap();
    let windows = driver
        .launch(
            AppDefinition::new()
                .with_module(LifecycleModule { id: ModuleId("test.zeta"), log })
                .with_module(LifecycleModule { id: ModuleId("test.alpha"), log })
                .window(WindowDefinition::new(WindowOptions::default(), move |b| {
                    b.on_lifecycle(move |w, event| log_lifecycle(w, log, "first window", event));
                    show(
                        move |r| r.get(visible),
                        move |b| {
                            b.on_lifecycle(move |w, event| log_lifecycle(w, log, "child", event));
                            el()
                        },
                    )
                }))
                .window(WindowDefinition::new(WindowOptions::default(), move |b| {
                    b.on_lifecycle(move |w, event| log_lifecycle(w, log, "second window", event));
                    el()
                })),
        )
        .unwrap();
    let [first, second] = windows[..] else { panic!("expected two windows") };
    let event = HostLifecycleEvent::OpenUrls(vec!["test:open".into()]);
    let dispatch = |driver: &mut HeadlessDriver| {
        driver.dispatch_lifecycle(event.clone()).unwrap();
        driver.app_mut().update(|w| w.update(log, std::mem::take)).unwrap()
    };
    let first_delivery = dispatch(&mut driver);
    assert_eq!(dispatch(&mut driver), first_delivery);
    assert!(first_delivery.iter().all(|visit| visit.event == event));
    let mut listeners: Vec<_> = first_delivery.iter().map(|visit| (visit.listener, visit.window)).collect();
    listeners.sort_unstable_by_key(|&(listener, _)| listener);
    assert_eq!(
        listeners,
        [
            ("app", None),
            ("child", Some(first)),
            ("first window", Some(first)),
            ("installed", None),
            ("second window", Some(second)),
            ("test.alpha", None),
            ("test.zeta", None),
        ]
    );

    driver.app_mut().update(|w| w.set(visible, false)).unwrap();
    driver.app_mut().close_window(second).unwrap();
    let surviving: Vec<_> = first_delivery.into_iter().filter(|visit| !matches!(visit.listener, "child" | "second window")).collect();
    assert_eq!(dispatch(&mut driver), surviving);
}
