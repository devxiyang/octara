//! Inspector targets bind an observed allocation, including every phase of a delivered input.

use octara::app::Event;
use octara::inspect::{self, Delivery, Reply};
use octara::platform::{HeadlessPlatform, WindowOptions};
use octara::prelude::*;
use serde_json::{Value, json};
use std::cell::Cell;
use std::rc::Rc;

#[ui]
fn target_view() -> View {
    let replaced = state("replaced", || false);
    let count = state("count", || 0);
    let offset = state("offset", || 0.0);
    let value = state("value", String::new);
    col()
        .size_full()
        .gap_2()
        .child(el().id("replace").size_12().on_press(move || replaced.update(|v| *v = !*v)))
        .child(el().id("mutate").size_12().on_press(move || {
            count.update(|n| *n += 1);
            offset.update(|n| *n += 10.0);
        }))
        .child(keyed(
            move || replaced.get(),
            move |replacement| {
                el().id("target")
                    .label(if *replacement { "Second" } else { "First" })
                    .relative()
                    .style_with(move |p| p.left(offset.get()))
                    .size_12()
                    .on_press(move || count.update(|n| *n += 1))
            },
        ))
        .child(text(move || count.get().to_string()).id("count"))
        .child(input().id("field").w(240).value(move || value.get()).on_input(move |v| value.set(v)))
        .child(input().id("other").w(240))
        .into()
}

fn node_target(window: &Window, id: &str) -> String {
    window.inspect().find(id).unwrap().target.clone()
}

fn request(window: &mut Window, value: Value) -> Reply {
    inspect::handle_line(window, &value.to_string())
}

fn stale(reply: &Reply, delivery: Delivery) {
    assert!(!reply.ok, "{reply:?}");
    assert_eq!(reply.code.as_deref(), Some("STALE_TARGET"), "{reply:?}");
    assert_eq!(reply.delivery, Some(delivery), "{reply:?}");
}

#[test]
fn the_compatibility_parser_cannot_silently_discard_a_target_guard() {
    let mut window = Window::headless(target_view);
    let target = node_target(&window, "target");
    let line = json!({"op":"tap","id":"target","target":target}).to_string();
    let refusal = inspect::parse_request(&window, &line).err().expect("the legacy parser cannot retain an operation-local guard");
    assert_eq!(refusal.code.as_deref(), Some("BAD_REQUEST"));
    assert_eq!(refusal.delivery, Some(Delivery::Refused));
    assert_eq!(window.kernel().text_of("count").as_deref(), Some("0"));
    assert!(inspect::parse_request(&window, r#"{"op":"tap","id":"target"}"#).is_ok());
    assert!(inspect::handle_line(&mut window, &line).ok, "the guarded entry point still accepts this same target");
    assert_eq!(window.kernel().text_of("count").as_deref(), Some("1"));
}

#[test]
fn targets_survive_property_updates_but_not_same_id_replacement() {
    let mut window = Window::headless(target_view);
    let target = node_target(&window, "target");
    let before = window.inspect().find("target").unwrap().bounds;
    window.tap("mutate").unwrap();
    assert_eq!(node_target(&window, "target"), target);
    assert_ne!(window.inspect().find("target").unwrap().bounds, before);
    assert!(request(&mut window, json!({"op":"tap","id":"target","target":target})).ok);
    assert_eq!(window.kernel().text_of("count").as_deref(), Some("2"));
    window.tap("replace").unwrap();
    assert_ne!(node_target(&window, "target"), target);
    stale(&request(&mut window, json!({"op":"tap","id":"target","target":target})), Delivery::Refused);
    let wrong = node_target(&window, "mutate");
    stale(&request(&mut window, json!({"op":"tap","id":"target","target":wrong})), Delivery::Refused);
    assert_eq!(window.kernel().text_of("count").as_deref(), Some("2"), "neither stale nor unrelated live targets reached the replacement");
    let fresh = node_target(&window, "target");
    assert!(request(&mut window, json!({"op":"tap","id":"target","target":fresh})).ok);
    assert_eq!(window.kernel().text_of("count").as_deref(), Some("3"));
}

#[test]
fn reload_retires_window_and_node_targets_while_preserving_app_state() {
    let mut window = Window::headless(target_view);
    window.tap("mutate").unwrap();
    let old_window = window.inspect().target;
    let old_node = node_target(&window, "target");
    let reply = request(&mut window, json!({"op":"reload","target":old_window}));
    assert!(reply.ok, "{reply:?}");
    assert_eq!(reply.delivery, Some(Delivery::Delivered));
    assert_eq!(window.kernel().text_of("count").as_deref(), Some("1"));
    assert_ne!(window.target(), old_window);
    assert_ne!(node_target(&window, "target"), old_node);
    stale(&request(&mut window, json!({"op":"size","width":700,"height":500,"target":old_window})), Delivery::Refused);
    stale(&request(&mut window, json!({"op":"tap","id":"target","target":old_node})), Delivery::Refused);
    let fresh = window.target();
    assert!(request(&mut window, json!({"op":"size","width":700,"height":500,"target":fresh})).ok);
    assert_eq!(window.size(), (700.0, 500.0));
}

#[test]
fn window_generations_and_app_namespaces_reject_reused_numbers() {
    let app = App::new(HeadlessPlatform::default());
    let mut old = app.open_window(WindowOptions::new("Old"), target_view).unwrap();
    let old_window = old.target();
    let old_node = node_target(&old, "target");
    let number = old.id().number();
    let node = old.kernel().find("target").unwrap();
    app.close_window(old.id());
    let mut reopened = app.open_window(WindowOptions::new("New"), target_view).unwrap();
    assert_ne!(reopened.id().number(), number);
    stale(&request(&mut old, json!({"op":"size","width":700,"height":500,"target":old_window})), Delivery::Refused);
    stale(&request(&mut reopened, json!({"op":"size","window":number,"width":700,"height":500,"target":old_window})), Delivery::Refused);
    stale(&request(&mut reopened, json!({"op":"tap","id":"target","target":old_node})), Delivery::Refused);

    let mut other_app = Window::headless(target_view);
    assert_eq!(other_app.id().number(), number);
    assert_eq!(other_app.kernel().find("target"), Some(node), "allocation numbers alone repeat in a different app");
    stale(&request(&mut other_app, json!({"op":"size","width":700,"height":500,"target":old_window})), Delivery::Refused);
    stale(&request(&mut other_app, json!({"op":"tap","id":"target","target":old_node})), Delivery::Refused);
}

#[test]
fn picks_share_snapshot_targets_and_old_json_defaults_are_compatible() {
    let mut window = Window::headless(target_view);
    let snapshot = window.inspect();
    assert_eq!(snapshot.target, snapshot.windows[0].target);
    let field = snapshot.find("field").unwrap();
    let [x, y, w, h] = field.bounds;
    let reply = request(&mut window, json!({"op":"pick","x":x+w/2.0,"y":y+h/2.0,"target":snapshot.target}));
    assert!(reply.ok, "{reply:?}");
    assert!(reply.delivery.is_none(), "a read-only query does not report delivered input");
    assert_eq!(reply.picked.unwrap().node.target, field.target);
    fn strip(value: &mut Value) {
        match value {
            Value::Object(object) => {
                object.remove("target");
                for value in object.values_mut() {
                    strip(value);
                }
            }
            Value::Array(array) => {
                for value in array {
                    strip(value);
                }
            }
            _ => {}
        }
    }
    let mut json = serde_json::to_value(snapshot).unwrap();
    strip(&mut json);
    let old: inspect::Snapshot = serde_json::from_value(json).unwrap();
    assert!(old.target.is_empty());
    assert!(old.windows.iter().all(|w| w.target.is_empty()));
    assert!(old.root.walk().iter().all(|n| n.target.is_empty()));
}

#[test]
fn malformed_or_wrong_scope_guards_never_fall_back_to_unchecked_input() {
    let mut window = Window::headless(target_view);
    for target in [Value::Null, json!(false), json!(12), json!([]), json!({}), json!(""), json!("garbage")] {
        let reply = request(&mut window, json!({"op":"tap","id":"target","target":target}));
        assert_eq!(reply.code.as_deref(), Some("BAD_REQUEST"), "{reply:?}");
        assert_eq!(reply.delivery, Some(Delivery::Refused));
    }
    let window_target = window.target();
    let node = node_target(&window, "target");
    for request_value in [
        json!({"op":"tap","id":"target","target":window_target}),
        json!({"op":"size","width":700,"height":500,"target":node}),
        json!({"op":"select","from":["a",0],"to":["b",1],"target":node}),
    ] {
        let reply = request(&mut window, request_value);
        assert_eq!(reply.code.as_deref(), Some("BAD_REQUEST"));
        assert_eq!(reply.delivery, Some(Delivery::Refused));
    }
    let reply = request(&mut window, json!({"op":"theme","name":"invalid","target":window_target}));
    assert_eq!(reply.delivery, Some(Delivery::Refused));
    assert_eq!(window.kernel().text_of("count").as_deref(), Some("0"));
    assert!(request(&mut window, json!({"op":"tap","id":"target"})).ok, "omitting target retains legacy test-id requests");
}

fn replacing_field(on_focus: bool) -> (Window, Rc<Cell<usize>>) {
    let writes = Rc::new(Cell::new(0));
    let observed = writes.clone();
    let window = Window::headless(move || {
        let replaced = state("replaced", || false);
        let value = state("value", String::new);
        let writes = observed.clone();
        col().size_full().child(keyed(
            move || replaced.get(),
            move |replacement| {
                let writes = writes.clone();
                let field = input().id("field").w(240).value(move || value.get()).on_input(move |text| {
                    writes.set(writes.get() + 1);
                    value.set(text);
                });
                if *replacement {
                    field
                } else if on_focus {
                    field.on_focus(move || replaced.set(true))
                } else {
                    field.on_key(move |key| {
                        if key.key == Key::Char('x') {
                            replaced.set(true);
                        }
                        false
                    })
                }
            },
        ))
    });
    (window, writes)
}

#[test]
fn focus_handlers_cannot_retarget_a_guarded_type_to_a_same_id_replacement() {
    let (mut window, writes) = replacing_field(true);
    let target = node_target(&window, "field");
    stale(&request(&mut window, json!({"op":"type","id":"field","text":"wrong","target":target})), Delivery::Delivered);
    assert_ne!(node_target(&window, "field"), target);
    assert_eq!(window.kernel().value_of("field").as_deref(), Some(""));
    assert_eq!(writes.get(), 0, "the old focus phase ran, but no text reached the replacement");
}

#[test]
fn key_fallback_cannot_type_into_a_same_id_replacement() {
    let (mut window, writes) = replacing_field(false);
    window.dispatch(Event::Focus("field".into())).unwrap();
    let target = node_target(&window, "field");
    stale(&request(&mut window, json!({"op":"key","chord":"x","target":target})), Delivery::Delivered);
    assert_ne!(node_target(&window, "field"), target);
    assert_eq!(writes.get(), 0);
    assert_eq!(window.kernel().value_of("field").as_deref(), Some(""));
}

#[test]
fn a_final_handled_tab_may_move_focus_without_becoming_stale() {
    let mut window = Window::headless(target_view);
    window.dispatch(Event::Focus("field".into())).unwrap();
    let target = node_target(&window, "field");
    let reply = request(&mut window, json!({"op":"key","chord":"tab","target":target}));
    assert!(reply.ok, "{reply:?}");
    assert_eq!(reply.delivery, Some(Delivery::Delivered));
    assert_eq!(window.inspect().focus.as_deref(), Some("other"));
}

#[test]
fn duplicate_names_refuse_lookup_and_tokens_select_one_mounted_instance() {
    let mut window = Window::headless(|| {
        let hidden = state("hidden", || false);
        let removed = state("removed", || false);
        let pressed = state("pressed", || "none");
        on_command("hide-choice", move || hidden.set(true));
        on_command("remove-choice", move || removed.set(true));
        on_command("focus-choice", || focus("choice"));
        col()
            .id("choices")
            .role(Role::ListBox)
            .active_descendant(Some("choice".to_owned()))
            .child(el().id("choice").label("First").role(Role::ListBoxOption).focusable().size_12().on_press(move || pressed.set("first")))
            .child(show(
                move || !removed.get(),
                move || {
                    el().id("choice")
                        .label("Second")
                        .role(Role::ListBoxOption)
                        .focusable()
                        .style_with(move |p| if hidden.get() { p.hidden() } else { p })
                        .size_12()
                        .on_press(move || pressed.set("second"))
                },
            ))
            .child(text(move || pressed.get().to_owned()).id("pressed"))
    });
    let snapshot = window.inspect();
    assert!(snapshot.find("choice").is_none());
    assert!(snapshot.find("choices").unwrap().active_descendant.is_none());
    let token = |name| snapshot.root.walk().into_iter().find(|n| n.name.as_deref() == Some(name)).unwrap().target.clone();
    let first = token("First");
    let second = token("Second");
    assert!(window.kernel().find("choice").is_none());
    assert_eq!(window.kernel().find_all("choice").len(), 2);
    assert!(window.dispatch(Event::Focus("choice".into())).unwrap_err().contains("2 elements"));
    window.invoke("focus-choice").unwrap();
    assert!(window.inspect().focus.is_none());
    for step in [
        json!({"op":"focus","id":"choice"}),
        json!({"op":"tap","id":"choice"}),
        json!({"op":"expect","id":"choice","text":"First"}),
        json!({"op":"select","region":"choice"}),
    ] {
        let reply = request(&mut window, step);
        assert_eq!(reply.code.as_deref(), Some("AMBIGUOUS_TARGET"), "{reply:?}");
        assert_eq!(reply.delivery, Some(Delivery::Refused));
    }
    let reply = request(&mut window, json!({"op":"tap","target":second}));
    assert!(reply.ok, "{reply:?}");
    assert_eq!(window.kernel().text_of("pressed").as_deref(), Some("second"));
    assert!(request(&mut window, json!({"op":"expect","target":second,"text":"Second"})).ok);
    assert!(request(&mut window, json!({"op":"focus","target":first,"id":"choice"})).ok);
    assert!(request(&mut window, json!({"op":"expect","target":first,"state":"focused"})).ok);

    window.invoke("hide-choice").unwrap();
    let hidden = request(&mut window, json!({"op":"focus","target":second}));
    assert_eq!(hidden.code.as_deref(), Some("STEP_FAILED"), "{hidden:?}");
    assert_eq!(hidden.delivery, Some(Delivery::Refused));
    let ambiguous = request(&mut window, json!({"op":"focus","id":"choice"}));
    assert_eq!(ambiguous.code.as_deref(), Some("AMBIGUOUS_TARGET"));
    assert!(window.inspect().find("choices").unwrap().active_descendant.is_none());

    window.invoke("remove-choice").unwrap();
    stale(&request(&mut window, json!({"op":"focus","target":second})), Delivery::Refused);
    assert!(request(&mut window, json!({"op":"tap","id":"choice"})).ok);
    assert_eq!(window.kernel().text_of("pressed").as_deref(), Some("first"));
    assert_eq!(window.inspect().find("choices").unwrap().active_descendant.as_deref(), Some("choice"));
}

#[test]
fn unnamed_instances_can_receive_and_replay_observed_focus_and_text() {
    fn view() -> El {
        let one = state("one", String::new);
        let two = state("two", String::new);
        col()
            .child(input().label("One").w(200).value(one).on_input(move |value| one.set(value)))
            .child(input().label("Two").w(200).value(two).on_input(move |value| two.set(value)))
    }
    let mut live = Window::headless(view);
    let snapshot = live.inspect();
    let field = snapshot.root.walk().into_iter().find(|n| n.name.as_deref() == Some("Two")).unwrap();
    assert!(field.id.is_none());
    let token = &field.target;
    live.app().start_recording();
    assert!(request(&mut live, json!({"op":"focus","target":token})).ok);
    assert!(request(&mut live, json!({"op":"type","target":token,"text":"中文"})).ok);
    let recording = live.app().stop_recording();
    let encoded = serde_json::to_string(&recording).unwrap();
    let replay = Window::headless(view);
    replay.app().replay(&serde_json::from_str(&encoded).unwrap()).unwrap();
    for w in [&live, &replay] {
        let snapshot = w.inspect();
        let nodes = snapshot.root.walk();
        let field = |name| nodes.iter().find(|n| n.name.as_deref() == Some(name)).unwrap();
        assert_eq!(field("One").value.as_deref(), Some(""));
        assert_eq!(field("Two").value.as_deref(), Some("中文"));
    }
}

#[test]
fn observed_nodes_are_routed_to_the_named_window_only() {
    let app = App::new(HeadlessPlatform::default());
    let mut left = app.open_window(WindowOptions::new("Left"), target_view).unwrap();
    let right = app.open_window(WindowOptions::new("Right"), target_view).unwrap();
    let token = node_target(&right, "target");
    stale(&request(&mut left, json!({"op":"tap","target":token})), Delivery::Refused);
    let reply = request(&mut left, json!({"op":"tap","window":right.id().number(),"target":token}));
    assert!(reply.ok, "{reply:?}");
    assert_eq!(left.kernel().text_of("count").as_deref(), Some("0"));
    assert_eq!(right.kernel().text_of("count").as_deref(), Some("1"));
}
