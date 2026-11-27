use octara_model::geometry::{LayoutBox, Point, WindowViewport};
use octara_model::input::{ContactTarget, EditorEvent};
use octara_model::style::State;
use octara_runtime::turn::{TurnError, WINDOW_ROUNDS};
use octara_runtime::{AppRuntime, CommitOutput, HandlerEvent, HostEngine, Input, ProjectError, ProjectFeedback, WindowOptions};
use octara_tree::editor::{EditorSession, EditorSlot};
use octara_tree::tables::HandlerKind;
use octara_tree::{NodeChange, NodeContent, NodeId, NodeSpec, RuntimeOp, Tree, TreeDelta};
use std::cell::Cell;
use std::rc::Rc;

#[derive(Default)]
struct HostState {
    projects: Cell<usize>,
    rollbacks: Cell<usize>,
    bounce: Cell<bool>,
    #[cfg(not(target_family = "wasm"))]
    publish_from_worker: Cell<Option<octara_runtime::task::Sender<()>>>,
    publish_locally: Cell<Option<octara_runtime::task::Sender<()>>>,
}

struct TestHost(Rc<HostState>);

impl HostEngine for TestHost {
    fn project(&mut self, tree: &Tree, _: &TreeDelta, _: WindowViewport, _: f64) -> Result<ProjectFeedback, ProjectError> {
        self.0.projects.set(self.0.projects.get() + 1);
        if let Some(sender) = self.0.publish_locally.take() {
            sender.send(()).unwrap();
        }
        #[cfg(not(target_family = "wasm"))]
        if let Some(sender) = self.0.publish_from_worker.take() {
            std::thread::spawn(move || sender.send(()).unwrap()).join().unwrap();
        }
        let mut feedback = ProjectFeedback::default();
        if self.0.bounce.get() {
            let root = tree.root();
            feedback.updates.push(RuntimeOp::SetState {
                node: root,
                state: State::HOVER,
                on: !tree.node(root).unwrap().state.contains(State::HOVER),
            });
        }
        Ok(feedback)
    }

    fn commit(&mut self) -> CommitOutput {
        CommitOutput::default()
    }
    fn rollback(&mut self) {
        self.0.rollbacks.set(self.0.rollbacks.get() + 1);
    }
    fn hit_test(&self, _: &Tree, _: Point) -> Option<ContactTarget> {
        None
    }
    fn geometry(&self, _: NodeId) -> Option<LayoutBox> {
        None
    }
}

fn app() -> (AppRuntime, Rc<HostState>) {
    let host = Rc::new(HostState::default());
    let engine = host.clone();
    (AppRuntime::with_namespace([1, 2], move |_, _| Box::new(TestHost(engine.clone()))), host)
}

#[test]
fn feedback_loop_rolls_back_at_the_causal_round_limit() {
    let (mut app, host) = app();
    host.bounce.set(true);
    assert_eq!(app.open_window(WindowOptions::default(), |_| {}), Err(TurnError::LayoutLoop));
    assert_eq!(host.projects.get(), WINDOW_ROUNDS);
    assert_eq!(host.rollbacks.get(), 1);
    assert_eq!(app.windows().count(), 0);
    assert!(app.take_host_commands().is_empty());

    host.bounce.set(false);
    app.open_window(WindowOptions::default(), |_| {}).unwrap();
    assert_eq!(app.windows().count(), 1);
}

#[cfg(not(target_family = "wasm"))]
#[test]
fn worker_wakes_during_projection_wait_for_next_turn_and_notify_host() {
    use octara_runtime::task::channel;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    let (mut app, host) = app();
    let (local, mut local_rx) = channel();
    let (worker, mut worker_rx) = channel();
    let local_arrived = Rc::new(Cell::new(false));
    let worker_arrived = Rc::new(Cell::new(false));
    let saw_local = local_arrived.clone();
    let saw_worker = worker_arrived.clone();
    app.build(move |b| {
        b.spawn("local channel", move |task| async move {
            saw_local.set(task.recv(&mut local_rx).await.is_some());
        });
        b.spawn("external channel", move |task| async move {
            saw_worker.set(task.recv(&mut worker_rx).await.is_some());
        });
    })
    .unwrap();
    let wakes = Arc::new(AtomicUsize::new(0));
    let notified = wakes.clone();
    app.set_wake(Some(Arc::new(move || {
        notified.fetch_add(1, Ordering::Relaxed);
    })));
    host.publish_locally.set(Some(local));
    host.publish_from_worker.set(Some(worker));
    app.open_window(WindowOptions::default(), |_| {}).unwrap();
    assert!(local_arrived.get(), "same-thread wakes settle in the current turn");
    assert!(!worker_arrived.get(), "off-thread arrivals cannot extend the current turn");
    assert_eq!(wakes.load(Ordering::Relaxed), 2);
    app.settle().unwrap();
    assert!(worker_arrived.get());
}

#[test]
fn accepted_ime_commit_keeps_the_session_through_a_bound_value_write() {
    let (mut app, _) = app();
    let value = app.build(|cx| cx.signal(String::new())).unwrap();
    let window = app
        .open_window(WindowOptions::default(), move |mount| {
            let node = mount.create(NodeSpec::new(NodeContent::TextInput(Box::new(EditorSlot::new("")))));
            mount.change(node, NodeChange::TestId(Some("editor".into())));
            mount.place(mount.root(), 0, node);
            mount.bind(node, move |read| NodeChange::EditorValue(read.get(value)));
            mount.on(node, HandlerKind::Input, move |write, event| {
                if let HandlerEvent::EditorInput { value: input, .. } = event {
                    write.set(value, (*input).to_owned());
                }
            });
        })
        .unwrap();

    app.update(Some(window), |write| write.focus("editor")).unwrap();
    let active = EditorSession::active(app.window(window).unwrap().tree()).unwrap();
    let (session, revision) = (active.id(), active.editor().document_rev());
    app.dispatch(
        window,
        Input::Editor(EditorEvent::Composition {
            session,
            expected_rev: revision,
            text: "界".into(),
            selection: Some((3, 0)),
            replacement: None,
        }),
    )
    .unwrap();

    let active = EditorSession::active(app.window(window).unwrap().tree()).unwrap();
    assert_eq!(active.editor().value(), "");
    assert_eq!(active.editor().composition().unwrap().text(), "界");
    let revision = active.editor().document_rev();
    app.dispatch(window, Input::Editor(EditorEvent::Commit { session, expected_rev: revision, text: "世界".into() })).unwrap();

    let active = EditorSession::active(app.window(window).unwrap().tree()).unwrap();
    assert_eq!(active.id(), session);
    assert_eq!(active.editor().value(), "世界");
    assert_eq!(active.editor().document_rev(), revision + 1);
    assert!(active.editor().composition().is_none());
    assert_eq!(app.update(None, |write| write.get(value)).unwrap(), "世界");
}

#[test]
fn a_panic_in_a_write_latches_publication_failure() {
    let (mut app, _) = app();
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _ = app.update(None, |_| panic!("test callback panic"));
    }));
    assert!(outcome.is_err());
    assert!(app.is_failed());
    assert_eq!(app.settle(), Err(TurnError::AppFailed));
}

#[test]
fn a_back_gesture_requires_the_same_window_contact_at_completion() {
    use octara_model::input::{BackPhase, InteractionEvent};
    let (mut app, _) = app();
    let visits = app.build(|cx| cx.signal(0)).unwrap();
    let window = app
        .open_window(WindowOptions::default(), move |mount| {
            mount.on_command(mount.root(), "navigation.back", move |write| write.update(visits, |visits| *visits += 1));
        })
        .unwrap();
    let gesture = |id, phase| Input::Interaction(InteractionEvent::BackGesture { id, phase });
    assert!(app.dispatch(window, gesture(4, BackPhase::Started)).unwrap().handled);
    assert!(!app.dispatch(window, gesture(5, BackPhase::Completed)).unwrap().handled);
    assert!(app.dispatch(window, gesture(4, BackPhase::Progress(0.6))).unwrap().handled);
    assert!(app.dispatch(window, gesture(4, BackPhase::Completed)).unwrap().handled);
    assert!(!app.dispatch(window, gesture(4, BackPhase::Completed)).unwrap().handled);
    assert!(app.dispatch(window, gesture(7, BackPhase::Started)).unwrap().handled);
    app.dispatch(window, Input::Interaction(InteractionEvent::WindowActive(false))).unwrap();
    assert!(!app.dispatch(window, gesture(7, BackPhase::Completed)).unwrap().handled);
    assert_eq!(app.update(None, |write| write.get(visits)).unwrap(), 1);
}
