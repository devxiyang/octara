use octara::model::geometry::{LayoutBox, Rect};
use octara::model::input::{ContactTarget, InteractionEvent, PointerId, PointerKind};
use octara::prelude::*;
use octara::{CommitOutput, HostEngine, HostReply, Input, NodeId, ProjectError, ProjectFeedback, Tree};
use std::cell::RefCell;
use std::rc::Rc;

const POSITIONS: [(&str, Rect); 3] = [
    ("source", Rect::new(0.0, 0.0, 30.0, 30.0)),
    ("receiver", Rect::new(30.0, 0.0, 60.0, 30.0)),
    ("other", Rect::new(90.0, 0.0, 60.0, 30.0)),
];

struct Engine;

impl HostEngine for Engine {
    fn project(&mut self, _: &Tree, _: &octara::tree::TreeDelta, _: WindowViewport, _: f64) -> Result<ProjectFeedback, ProjectError> {
        Ok(ProjectFeedback::default())
    }

    fn commit(&mut self) -> CommitOutput {
        CommitOutput::default()
    }

    fn rollback(&mut self) {}

    fn hit_test(&self, tree: &Tree, point: Point) -> Option<ContactTarget> {
        let (id, _) = POSITIONS.iter().find(|(_, bounds)| bounds.contains(point))?;
        let node = *tree.test_ids(id).first()?;
        Some(ContactTarget { hit: tree.guard(node), action: (*id == "source").then_some(tree.guard(node)) })
    }

    fn geometry(&self, _: NodeId) -> Option<LayoutBox> {
        None
    }

    fn interaction_bounds(&self, _: NodeId) -> Option<Rect> {
        Some(POSITIONS[1].1)
    }
}

#[derive(Default)]
struct Seen {
    hover: Vec<Option<DropPosition>>,
    drops: Vec<DropEvent>,
    presses: usize,
}

struct Fixture {
    app: App,
    window: WindowId,
    seen: Rc<RefCell<Seen>>,
    disabled: Signal<bool>,
}

impl Fixture {
    fn new() -> Self {
        let seen = Rc::new(RefCell::new(Seen::default()));
        let mut app = App::new(|_, _| Box::new(Engine));
        let disabled = app.build(|b| b.signal(false)).unwrap();
        let window = app
            .open_window(WindowOptions::default(), {
                let seen = seen.clone();
                move |_| {
                    let hovered = seen.clone();
                    let dropped = seen.clone();
                    let pressed = seen.clone();
                    col()
                        .child(
                            el().test_id("source")
                                .draggable(DragPayload::new("row", 42).preview_label("A row"))
                                .reorder_keys()
                                .on_press(move |_| pressed.borrow_mut().presses += 1),
                        )
                        .child(
                            el().test_id("receiver")
                                .disabled(disabled)
                                .drop_target(["row"], DropAxis::Horizontal, move |_, event| dropped.borrow_mut().drops.push(event.clone()))
                                .on_drop_hover(move |_, event| {
                                    assert_eq!(event.payload.preview_label.as_deref(), Some("A row"));
                                    hovered.borrow_mut().hover.push(event.position);
                                }),
                        )
                        .child(el().test_id("other").drop_target(["image"], DropAxis::Horizontal, |_, _| panic!("mismatched kind")))
                }
            })
            .unwrap();
        Self { app, window, seen, disabled }
    }

    fn tree(&self) -> &Tree {
        self.app.runtime().window(self.window).unwrap().tree()
    }

    fn has(&self, name: &str, state: State) -> bool {
        self.tree().node(self.tree().test_ids(name)[0]).unwrap().state.contains(state)
    }

    fn send(&mut self, event: InteractionEvent) {
        self.app.dispatch(self.window, Input::Interaction(event)).unwrap();
    }

    fn down(&mut self, kind: PointerKind) {
        let pos = Point::new(2.0, 5.0);
        let target = Engine.hit_test(self.tree(), pos).unwrap();
        self.send(InteractionEvent::PointerDown { id: PointerId(1), kind, target, pos, button: Button::Primary, mods: Modifiers::empty() });
    }

    fn moved(&mut self, x: f32) {
        self.send(InteractionEvent::PointerMove { id: PointerId(1), pos: Point::new(x, 5.0), mods: Modifiers::empty() });
    }

    fn release(&mut self, x: f32) {
        let pos = Point::new(x, 5.0);
        let target = Engine.hit_test(self.tree(), pos);
        self.send(InteractionEvent::PointerUp { id: PointerId(1), target, pos, mods: Modifiers::SHIFT });
    }

    fn key(&mut self, key: Key) {
        self.send(InteractionEvent::Key(KeyEvent::new(key, Modifiers::empty())));
    }
}

#[test]
fn pointer_transfer_delivers_live_payload_and_axis_position_once() {
    let mut f = Fixture::new();
    f.down(PointerKind::Mouse);
    f.moved(5.0);
    assert!(!f.has("source", State::DRAGGING));
    f.moved(35.0);
    assert!(f.has("source", State::DRAGGING));
    assert!(f.has("receiver", State::DROP_HOVER));
    f.moved(65.0);
    f.moved(100.0);
    assert!(!f.has("receiver", State::DROP_HOVER));
    f.release(88.0);
    assert!(!f.has("source", State::DRAGGING));
    assert!(!f.has("receiver", State::DROP_HOVER));
    f.release(88.0);

    let seen = f.seen.borrow();
    assert_eq!(seen.hover, [Some(DropPosition::Before), Some(DropPosition::Inside), None, Some(DropPosition::After), None]);
    let [drop] = seen.drops.as_slice() else { panic!("expected exactly one drop") };
    assert_eq!(drop.position, DropPosition::After);
    assert_eq!(*drop.data, 42);
    assert_eq!(drop.modifiers, Modifiers::SHIFT);
    assert_eq!(drop.source.node(), Some(f.tree().test_ids("source")[0]));
}

#[test]
fn escape_and_receiver_disable_cancel_touch_transfer() {
    let mut f = Fixture::new();
    f.down(PointerKind::Touch);
    f.moved(8.0);
    assert!(!f.has("source", State::DRAGGING));
    f.moved(65.0);
    f.key(Key::Escape);
    f.release(65.0);
    assert!(!f.has("receiver", State::DROP_HOVER));
    assert!(!f.has("source", State::DRAGGING));

    f.down(PointerKind::Touch);
    f.moved(65.0);
    let disabled = f.disabled;
    f.app.update(|w| w.set(disabled, true)).unwrap();
    assert!(!f.has("receiver", State::DROP_HOVER));
    f.release(65.0);
    assert!(f.seen.borrow().drops.is_empty());
}

#[test]
fn keyboard_reorder_keeps_focus_on_source() {
    let mut f = Fixture::new();
    let source = f.tree().test_ids("source")[0];
    let target = Some(f.tree().guard(source));
    f.app.dispatch(f.window, Input::Host(HostReply::FocusChanged { target, visible: true })).unwrap();
    f.key(Key::Space);
    f.key(Key::Right);
    assert_eq!(f.tree().focus(), Some(source));
    assert!(f.has("receiver", State::DROP_HOVER));
    f.key(Key::Escape);
    assert!(f.seen.borrow().drops.is_empty());
    f.key(Key::Space);
    f.key(Key::Right);
    f.key(Key::Enter);
    assert_eq!(f.seen.borrow().drops[0].position, DropPosition::After);
    assert_eq!(f.tree().focus(), Some(source));
    f.send(InteractionEvent::Key(KeyEvent { repeat: true, ..KeyEvent::new(Key::Enter, Modifiers::empty()) }));
    assert_eq!(f.seen.borrow().drops.len(), 1);
    assert_eq!(f.seen.borrow().presses, 0);
}

#[test]
fn a_foreign_release_cannot_deliver_to_a_matching_slot() {
    let mut f = Fixture::new();
    f.down(PointerKind::Mouse);
    f.moved(65.0);
    assert!(f.has("receiver", State::DROP_HOVER));
    let receiver = f.tree().test_ids("receiver")[0];
    let foreign = octara::TargetGuard::for_node(octara::tree::TreeId::new([33, 77], 1), receiver);
    f.send(InteractionEvent::PointerUp {
        id: PointerId(1),
        target: Some(ContactTarget { hit: foreign, action: Some(foreign) }),
        pos: Point::new(65.0, 5.0),
        mods: Modifiers::empty(),
    });
    assert!(f.seen.borrow().drops.is_empty());
    assert!(!f.has("source", State::DRAGGING));
    assert!(!f.has("receiver", State::DROP_HOVER));
}
