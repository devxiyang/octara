use std::{cell::Cell, rc::Rc};

use octara::model::geometry::{LayoutBox, Point, WindowViewport};
use octara::model::input::ContactTarget;
use octara::prelude::*;
use octara::tree::{NodeId, RuntimeOp, Tree, TreeDelta};
use octara::{BackPhase, CommitOutput, HostEngine, HostReply, Input, InteractionEvent, ProjectError, ProjectFeedback};

struct TestHost;

impl HostEngine for TestHost {
    fn project(&mut self, tree: &Tree, _: &TreeDelta, _: WindowViewport, _: f64) -> Result<ProjectFeedback, ProjectError> {
        let updates = tree
            .nodes()
            .filter_map(|(node, _)| {
                let (_, offset) = tree.scroll(node)?.requested?;
                Some(RuntimeOp::SetScroll { node, offset, clamped: false })
            })
            .collect();
        Ok(ProjectFeedback { updates, ..Default::default() })
    }

    fn commit(&mut self) -> CommitOutput {
        CommitOutput::default()
    }

    fn rollback(&mut self) {}

    fn hit_test(&self, _: &Tree, _: Point) -> Option<ContactTarget> {
        None
    }

    fn geometry(&self, _: NodeId) -> Option<LayoutBox> {
        None
    }
}

fn app() -> App {
    App::new(|_, _| Box::new(TestHost))
}

fn node(app: &App, window: WindowId, name: &str) -> NodeId {
    let [id] = app.window(window).unwrap().runtime().tree().test_ids(name) else { panic!("one {name} node") };
    *id
}

fn press(app: &mut App, window: WindowId, name: &str) {
    let id = node(app, window, name);
    let target = app.window(window).unwrap().runtime().tree().guard(id);
    assert!(app.dispatch(window, Input::Host(HostReply::AccessibilityAction { target, action: A11yAction::Press })).unwrap().handled);
}

#[test]
fn close_uses_the_nearest_live_provider_even_inside_a_portal() {
    let mut app = app();
    let (outer, inner) = app
        .build(|b| {
            assert!(b.close_handle().is_none());
            (b.signal(true), b.signal(true))
        })
        .unwrap();
    let saved = Rc::new(Cell::new(None));
    let window = app
        .open_window(WindowOptions::default(), {
            let saved = saved.clone();
            move |_| {
                show(
                    move |r| r.get(outer),
                    move |b| {
                        b.provide_close(move |w| w.set(outer, false));
                        col()
                            .child(scoped(move |b| {
                                let close = b.close_handle().unwrap();
                                el().test_id("close-outer").role(Role::Button).focusable().on_press(move |w| close.close(w))
                            }))
                            .child(el().portal(move |r| r.get(inner), {
                                let saved = saved.clone();
                                move |b| {
                                    let owner = b.scope();
                                    b.provide_close(move |w| {
                                        assert_eq!(w.scope(), owner);
                                        w.set(inner, false);
                                    });
                                    saved.set(b.close_handle());
                                    scoped(move |b| {
                                        let close = b.close_handle().unwrap();
                                        el().test_id("close-inner").role(Role::Button).focusable().on_press(move |w| close.close(w))
                                    })
                                }
                            }))
                    },
                )
            }
        })
        .unwrap();

    press(&mut app, window, "close-inner");
    assert_eq!(app.update(|w| (w.get(outer), w.get(inner))).unwrap(), (true, false));
    let retired = saved.get().unwrap();
    app.update(|w| w.set(inner, true)).unwrap();
    app.update(|w| retired.close(w)).unwrap();
    assert!(app.update(|w| w.get(inner)).unwrap());
    press(&mut app, window, "close-outer");
    assert!(!app.update(|w| w.get(outer)).unwrap());
}

#[test]
fn navigator_guards_all_mutations_and_persists_back_and_forward_history() {
    let mut app = app();
    let (nav, block) = app
        .build(|b| {
            let nav = b.persisted_navigator("nav", || "home".to_owned());
            let block = b.signal(false);
            b.on_before_leave(nav, move |r, from, to| !(from == "form" && from != to && r.get(block)));
            (nav, block)
        })
        .unwrap();
    app.update(|w| {
        assert!(nav.push(w, "search".into()));
        assert!(nav.push(w, "form".into()));
        w.set(block, true);
        assert!(!nav.push(w, "new".into()));
        assert!(!nav.replace(w, "replacement".into()));
        assert!(!nav.pop(w));
        assert!(!nav.go(w, -2));
        assert!(!nav.pop_to_root(w));
        assert!(!nav.reset(w, ["discarded".into()]));
        assert_eq!(w.read(|r| (nav.current(r), nav.depth(r))), ("form".into(), 3));
        w.set(block, false);
        assert!(nav.pop(w));
        assert_eq!(w.read(|r| nav.stack(r)), ["home", "search"]);
    })
    .unwrap();

    let mut restored = restored_app(app.export_persisted());
    let nav: Navigator<String> = restored.build(|b| b.persisted_navigator("nav", || panic!("restored route"))).unwrap();
    restored
        .update(|w| {
            let state = w.read(|r| (nav.stack(r), nav.can_go_back(r), nav.can_go_forward(r)));
            assert_eq!(state, (vec!["home".to_owned(), "search".to_owned()], true, true));
            assert!(!nav.go(w, isize::MAX));
            assert!(nav.go(w, 1));
            assert_eq!(w.read(|r| nav.current(r)), "form");
            assert!(nav.pop_to_root(w));
            assert!(nav.replace(w, "root".into()));
            assert!(nav.push(w, "branch".into()));
            assert!(!w.read(|r| nav.can_go_forward(r)));
            assert!(!nav.reset(w, []));
            assert!(nav.reset(w, ["fresh".into(), "detail".into()]));
            assert_eq!(w.read(|r| nav.stack(r)), ["fresh", "detail"]);
        })
        .unwrap();
}

fn restored_app(state: RestartState) -> App {
    let mut restored = app();
    assert!(restored.import_persisted(state));
    restored
}

#[test]
fn route_mount_modes_preserve_or_reset_page_identity_and_editor_state() {
    for mode in [RouteMount::KeepAlive, RouteMount::Unmount] {
        let mut app = app();
        let nav = app.build(|b| b.navigator("home")).unwrap();
        let draft = Rc::new(Cell::new(None));
        let cleanups = Rc::new(Cell::new(0));
        let window = app
            .open_window(WindowOptions::default(), {
                let (draft, cleanups) = (draft.clone(), cleanups.clone());
                move |_| {
                    route_stack(nav, mode, move |b, route| {
                        if *route != "home" {
                            return text(*route).test_id("second");
                        }
                        let value = b.signal(String::new());
                        draft.set(Some(value));
                        let cleanups = cleanups.clone();
                        b.on_cleanup(move |_| cleanups.set(cleanups.get() + 1));
                        col()
                            .overflow_y(Overflow::Scroll)
                            .test_id("scroll")
                            .child(input().test_id("draft").value(value))
                            .child(el().portal(|_| true, |_| text("portal").test_id("portal")))
                    })
                }
            })
            .unwrap();
        let original = node(&app, window, "draft");
        let value = draft.get().unwrap();
        let scroll = node(&app, window, "scroll");
        app.update_window(window, |w| {
            w.set(value, "draft text".to_owned());
            w.scroll_to(scroll, Vec2::new(0.0, 42.0));
        })
        .unwrap();
        app.update(|w| assert!(nav.push(w, "second"))).unwrap();
        let tree = app.window(window).unwrap().runtime().tree();
        if mode == RouteMount::KeepAlive {
            let [portal] = tree.test_ids("portal") else { panic!("retained page retains its portal") };
            assert!(!tree.is_shown(*portal));
            assert!(!tree.is_shown(original));
        } else {
            assert!(tree.node(original).is_none());
        }
        app.update(|w| assert!(nav.pop(w))).unwrap();

        let revisited = node(&app, window, "draft");
        let scroll = node(&app, window, "scroll");
        let tree = app.window(window).unwrap().runtime().tree();
        let editor = tree.editor(revisited).unwrap();
        if mode == RouteMount::KeepAlive {
            assert_eq!(revisited, original);
            assert_eq!(editor.value(), "draft text");
            assert_eq!(tree.scroll(scroll).unwrap().offset.y, 42.0);
            assert_eq!(cleanups.get(), 0);
            assert!(tree.is_shown(node(&app, window, "portal")));
        } else {
            assert_ne!(revisited, original);
            assert_eq!(editor.value(), "");
            assert_eq!(tree.scroll(scroll).unwrap().offset.y, 0.0);
            assert_eq!(cleanups.get(), 1);
        }
    }
}

#[test]
fn native_back_commits_once_and_uses_guards_for_the_active_page() {
    let mut app = app();
    let (nav, block) = app.build(|b| (b.navigator("root"), b.signal(true))).unwrap();
    let window = app
        .open_window(WindowOptions::default(), move |_| {
            route_stack(nav, RouteMount::KeepAlive, move |b, route| {
                if *route == "form" {
                    b.on_before_leave(nav, move |r, _, _| !r.get(block));
                }
                text(*route)
            })
        })
        .unwrap();
    let back = |app: &mut App, token, phase| {
        app.dispatch(window, Input::Interaction(InteractionEvent::BackGesture { id: token, phase })).unwrap().handled
    };

    assert!(app.window(window).unwrap().runtime().back_target().is_none());
    assert!(!back(&mut app, 1, BackPhase::Started));
    app.update(|w| assert!(nav.push(w, "form"))).unwrap();
    assert!(app.window(window).unwrap().runtime().back_target().is_some());
    assert!(back(&mut app, 2, BackPhase::Started));
    assert!(back(&mut app, 2, BackPhase::Progress(0.8)));
    assert!(!back(&mut app, 2, BackPhase::Completed));
    assert_eq!(app.update(|w| w.read(|r| nav.current(r))).unwrap(), "form");
    app.update(|w| w.set(block, false)).unwrap();
    assert!(back(&mut app, 3, BackPhase::Started));
    assert!(back(&mut app, 3, BackPhase::Cancelled));
    assert!(!back(&mut app, 3, BackPhase::Completed));
    assert!(back(&mut app, 4, BackPhase::Started));
    app.update(|w| assert!(nav.pop(w))).unwrap();
    app.update(|w| {
        w.set(block, true);
        assert!(nav.go(w, 1)); // The retained form's guard does not apply to root.
        w.set(block, false);
    })
    .unwrap();
    assert!(!back(&mut app, 4, BackPhase::Completed));
    assert!(back(&mut app, 5, BackPhase::Started));
    assert!(back(&mut app, 5, BackPhase::Completed));
    assert!(!back(&mut app, 5, BackPhase::Completed));
    assert!(app.window(window).unwrap().runtime().back_target().is_none());
    assert_eq!(app.update(|w| w.read(|r| nav.current(r))).unwrap(), "root");
}
