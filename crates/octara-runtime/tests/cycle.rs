use octara_model::geometry::{LayoutBox, Point, WindowViewport};
use octara_model::input::{ContactTarget, EditorEvent, TextEdit};
use octara_model::style::State;
use octara_runtime::cycle::{AppError, WINDOW_ROUNDS};
use octara_runtime::{AppRuntime, CommitOutput, HandlerEvent, HostEngine, Input, ProjectError, ProjectFeedback, WindowOptions};
use octara_tree::editor::{EditorSession, EditorSlot};
use octara_tree::tables::HandlerKind;
use octara_tree::{NodeChange, NodeContent, NodeId, NodeSpec, RuntimeOp, Tree, TreeDelta};
use std::cell::Cell;
use std::ops::Range;
use std::sync::Arc;

#[derive(Default)]
struct HostState {
    projects: Cell<usize>,
    rollbacks: Cell<usize>,
    bounce: Cell<bool>,
    #[cfg(not(target_family = "wasm"))]
    publish_from_worker: Cell<Option<octara_runtime::task::Sender<()>>>,
    publish_locally: Cell<Option<octara_runtime::task::Sender<()>>>,
}

struct TestHost(Arc<HostState>);

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

fn app() -> (AppRuntime, Arc<HostState>) {
    #[expect(clippy::arc_with_non_send_sync, reason = "the test observes mock window state on the app thread")]
    let host = Arc::new(HostState::default());
    let engine = host.clone();
    (AppRuntime::with_namespace([1, 2], move |_, _| Box::new(TestHost(engine.clone()))), host)
}

#[test]
fn feedback_loop_rolls_back_at_the_causal_round_limit() {
    let (mut app, host) = app();
    host.bounce.set(true);
    assert_eq!(app.open_window(WindowOptions::default(), |_| {}), Err(AppError::LayoutLoop));
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
fn worker_wakes_during_projection_wait_for_next_cycle_and_notify_host() {
    use octara_runtime::task::channel;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    let (mut app, host) = app();
    let (local, mut local_rx) = channel();
    let (worker, mut worker_rx) = channel();
    let local_arrived = Arc::new(AtomicBool::new(false));
    let worker_arrived = Arc::new(AtomicBool::new(false));
    let saw_local = local_arrived.clone();
    let saw_worker = worker_arrived.clone();
    app.build(move |b| {
        b.spawn("local channel", move |task| async move {
            saw_local.store(task.recv(&mut local_rx).await.is_some(), Ordering::Relaxed);
        });
        b.spawn("external channel", move |task| async move {
            saw_worker.store(task.recv(&mut worker_rx).await.is_some(), Ordering::Relaxed);
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
    assert!(local_arrived.load(Ordering::Relaxed), "same-thread wakes settle in the current cycle");
    assert!(!worker_arrived.load(Ordering::Relaxed), "off-thread arrivals cannot extend the current cycle");
    assert_eq!(wakes.load(Ordering::Relaxed), 2);
    app.settle().unwrap();
    assert!(worker_arrived.load(Ordering::Relaxed));
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
        Input::Editor(EditorEvent::Edit {
            session,
            expected_rev: revision,
            edit: TextEdit::Mark {
                text: "界".into(),
                selection: Some(Range { start: 3, end: 0 }),
                replacement: None,
                clauses: Default::default(),
            },
        }),
    )
    .unwrap();

    let active = EditorSession::active(app.window(window).unwrap().tree()).unwrap();
    assert_eq!(active.editor().value(), "");
    assert_eq!(active.editor().composition().unwrap().text(), "界");
    let revision = active.editor().document_rev();
    app.dispatch(
        window,
        Input::Editor(EditorEvent::Edit {
            session,
            expected_rev: revision,
            edit: TextEdit::Insert { text: "世界".into(), replacement: None },
        }),
    )
    .unwrap();

    let active = EditorSession::active(app.window(window).unwrap().tree()).unwrap();
    assert_eq!(active.id(), session);
    assert_eq!(active.editor().value(), "世界");
    assert_eq!(active.editor().document_rev(), revision + 1);
    assert!(active.editor().composition().is_none());
    assert_eq!(app.update(None, |write| write.get(value)).unwrap(), "世界");
}

#[test]
fn active_composition_owns_editor_keys_before_handlers_submit_and_modal_dismissal() {
    use octara_model::input::{EditorCommand, HostReply, InteractionEvent, Key, KeyEvent, Modifiers};
    use octara_tree::{overlay::LayerPolicy, transaction::OverlaySpec};
    let (mut app, _) = app();
    let (keys, submissions, dismissals, menu_hits) = app.build(|cx| (cx.signal(0), cx.signal(0), cx.signal(0), cx.signal(0))).unwrap();
    let window = app
        .open_window(WindowOptions::default(), move |mount| {
            let modal = mount.create(NodeSpec::new(NodeContent::Box));
            mount.change(modal, NodeChange::Overlay(OverlaySpec { layer: Some(LayerPolicy::modal()), ..Default::default() }));
            mount.place(mount.overlay_root(), 0, modal);
            mount.on(modal, HandlerKind::Dismiss, move |write, _| write.update(dismissals, |count| *count += 1));
            mount.on_command(modal, "menu.escape", move |write| write.update(menu_hits, |count| *count += 1));
            let mut editor = EditorSlot::new("A");
            editor.submit_on_enter = true;
            let node = mount.create(NodeSpec::new(NodeContent::TextInput(Box::new(editor))));
            mount.change(node, NodeChange::TestId(Some("editor".into())));
            mount.place(modal, 0, node);
            mount.on(node, HandlerKind::Key, move |write, _| write.update(keys, |count| *count += 1));
            mount.on(node, HandlerKind::Submit, move |write, _| write.update(submissions, |count| *count += 1));
        })
        .unwrap();
    app.update(Some(window), |write| write.focus("editor")).unwrap();
    let editor = EditorSession::active(app.window(window).unwrap().tree()).unwrap();
    let session = editor.id();
    let expected_rev = editor.editor().document_rev();
    let mark =
        TextEdit::Mark { text: "漢".into(), selection: Some(Range { start: 3, end: 0 }), replacement: None, clauses: Default::default() };
    assert!(app.dispatch(window, Input::Editor(EditorEvent::Edit { session, expected_rev, edit: mark })).unwrap().handled);
    let expected_rev = expected_rev + 1;
    for key in [Key::Enter, Key::Escape, Key::Space, Key::Left, Key::Backspace, Key::Char('1')] {
        let key = KeyEvent::new(key, Modifiers::empty());
        assert!(!app.dispatch(window, Input::Interaction(InteractionEvent::Key(key))).unwrap().handled);
        assert!(!app.dispatch(window, Input::Editor(EditorEvent::KeyIntercept { session, expected_rev, key })).unwrap().handled);
    }
    let command = EditorEvent::Command { session, expected_rev, command: EditorCommand::SelectAll };
    assert!(!app.dispatch(window, Input::Editor(command)).unwrap().handled);
    assert_eq!(app.update(None, |read| (read.get(keys), read.get(submissions), read.get(dismissals))).unwrap(), (0, 0, 0));
    let menu = |chord| Input::Host(HostReply::MenuCommand { name: "menu.escape".into(), chord });
    let chord = Some("escape".parse().unwrap());
    assert!(app.dispatch(window, menu(chord)).unwrap().handled);
    assert_eq!(app.update(None, |read| read.get(menu_hits)).unwrap(), 0);
    assert!(app.dispatch(window, menu(None)).unwrap().handled);
    assert_eq!(app.update(None, |read| read.get(menu_hits)).unwrap(), 1);
    let active = EditorSession::active(app.window(window).unwrap().tree()).unwrap();
    assert_eq!(active.editor().document_rev(), expected_rev);
    assert_eq!(active.editor().composition().unwrap().text(), "漢A");

    let event = EditorEvent::Edit { session, expected_rev, edit: TextEdit::Cancel };
    assert!(app.dispatch(window, Input::Editor(event)).unwrap().handled);
    assert!(app.dispatch(window, menu(chord)).unwrap().handled);
    assert_eq!(app.update(None, |read| read.get(menu_hits)).unwrap(), 2);
    for key in [Key::Enter, Key::Escape] {
        let key = KeyEvent::new(key, Modifiers::empty());
        assert!(app.dispatch(window, Input::Interaction(InteractionEvent::Key(key))).unwrap().handled);
    }
    assert_eq!(app.update(None, |read| (read.get(keys), read.get(submissions), read.get(dismissals))).unwrap(), (2, 1, 1));
}

#[test]
fn a_panic_in_a_write_latches_publication_failure() {
    let (mut app, _) = app();
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _ = app.update(None, |_| panic!("test callback panic"));
    }));
    assert!(outcome.is_err());
    assert!(app.is_failed());
    assert_eq!(app.settle(), Err(AppError::AppFailed));
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
