//! Chooser policies consume keyed option snapshots.
use octara::app::{Event, InputEvent};
use octara::inspect::{self, Delivery};
use octara::prelude::*;
use octara::serde_json::json;
use octara_kit::*;

#[derive(Clone, Copy)]
enum Chooser {
    Select,
    Combo,
    Popup,
}

fn options() -> Vec<Choice> {
    vec![Choice::new("a", "Shared"), Choice::new("b", "Shared"), Choice::new("c", "Blocked").disabled(true)]
}

fn scene(kind: Chooser, reject: bool) -> View {
    let source = state("source", options);
    let selected = state("selected", || None::<String>);
    let attempts = state("attempts", || 0usize);
    let open = state("open", || false);
    let draft = state("draft", String::new);
    on_command("open", move || open.set(true));
    on_command("reverse", move || source.update(|items| items.reverse()));
    on_command("duplicate", move || source.update(|items| items.push(Choice::new("a", "Different"))));
    on_command("restore", move || source.set(options()));
    on_command("remove", move || source.update(|items| items.retain(|item| item.id != "b")));
    let pick = move |value: Option<String>| {
        attempts.update(|count| *count += 1);
        if !reject {
            selected.set(value);
        }
    };
    let control = match kind {
        Chooser::Select => select("pick", "Choices", (move || selected.get(), pick), move || source.get()),
        Chooser::Combo => combobox("pick", "Choices", (move || selected.get(), pick), move || source.get()),
        Chooser::Popup => popup_list(
            "pick",
            "Choices",
            open,
            text_field(draft).label("Query"),
            move || source.get().into_iter().map(|item| PopupItem::new(item.id, item.label).disabled(item.disabled)).collect(),
            move |id| pick(Some(id)),
        ),
    };
    col()
        .size_full()
        .p_4()
        .gap_2()
        .child(control)
        .child(text(move || selected.get().unwrap_or_default()).id("chosen"))
        .child(text(move || attempts.get().to_string()).id("attempts"))
        .into()
}

impl Chooser {
    fn focus_target(self) -> &'static str {
        match self {
            Self::Select => "pick-list",
            Self::Combo => "pick-search",
            Self::Popup => "pick",
        }
    }
    fn open(self, window: &mut Window) {
        match self {
            Self::Select | Self::Combo => window.tap("pick").unwrap(),
            Self::Popup => {
                window.tap("pick").unwrap();
                window.invoke("open").unwrap();
            }
        }
    }
}

fn target(window: &Window, id: &str) -> String {
    window.inspect().find(id).unwrap().target.clone()
}
fn stale(window: &mut Window, token: &str) {
    let reply = inspect::handle_line(window, &json!({"op":"tap", "id":"pick-option-b", "target":token}).to_string());
    assert_eq!(reply.code.as_deref(), Some("STALE_TARGET"), "{reply:?}");
    assert_eq!(reply.delivery, Some(Delivery::Refused));
}

#[test]
fn duplicate_ids_retire_rows_disable_keyboard_choices_and_recover_with_fresh_targets() {
    for kind in [Chooser::Select, Chooser::Combo, Chooser::Popup] {
        let mut window = Window::headless(move || scene(kind, false));
        kind.open(&mut window);
        if matches!(kind, Chooser::Combo) {
            window.type_text("Shared");
        }
        let focused = target(&window, kind.focus_target());
        let old = target(&window, "pick-option-b");
        window.invoke("duplicate").unwrap();
        assert!(!window.kernel().exists("pick-option-a"));
        assert!(!window.kernel().exists("pick-option-b"));
        assert_eq!(window.inspect().find(kind.focus_target()).unwrap().active_descendant, None);
        window.press_key("down").unwrap();
        window.press_key("enter").unwrap();
        assert_eq!(window.kernel().text_of("attempts").as_deref(), Some("0"));
        assert_eq!(target(&window, kind.focus_target()), focused);
        assert!(window.app().diagnostics().iter().any(|d| d.code.id == "DUPLICATE_KEY"));
        window.invoke("restore").unwrap();
        assert_ne!(target(&window, "pick-option-b"), old);
        stale(&mut window, &old);
        window.press_key("down").unwrap();
        window.press_key("enter").unwrap();
        assert_eq!(window.kernel().text_of("chosen").as_deref(), Some("b"));
        assert!(window.app().diagnostics().iter().all(|d| d.code.id == "DUPLICATE_KEY"));
    }
}

#[test]
fn retained_callbacks_choose_ids_after_reordering_and_recreated_rows_reject_old_targets() {
    for kind in [Chooser::Select, Chooser::Combo, Chooser::Popup] {
        let mut window = Window::headless(move || scene(kind, false));
        kind.open(&mut window);
        let focused = target(&window, kind.focus_target());
        let before = target(&window, "pick-option-b");
        window.invoke("reverse").unwrap();
        assert_eq!(target(&window, "pick-option-b"), before);
        window.tap("pick-option-b").unwrap();
        assert_eq!(window.kernel().text_of("chosen").as_deref(), Some("b"));
        kind.open(&mut window);
        let old = target(&window, "pick-option-b");
        let focus = target(&window, kind.focus_target());
        window.invoke("remove").unwrap();
        let snapshot = window.inspect();
        assert_eq!(snapshot.focus.as_deref(), Some(kind.focus_target()));
        assert_eq!(snapshot.find(kind.focus_target()).unwrap().target, focus);
        assert_eq!(snapshot.find(kind.focus_target()).unwrap().active_descendant.as_deref(), Some("pick-option-a"));
        window.invoke("restore").unwrap();
        stale(&mut window, &old);
        window.tap("pick-option-b").unwrap();
        assert_eq!(window.kernel().text_of("attempts").as_deref(), Some("2"));
        if matches!(kind, Chooser::Popup) {
            assert_eq!(target(&window, "pick"), focused);
        }
        assert!(window.app().diagnostics().is_empty(), "{:?}", window.app().diagnostics());
        assert!(window.layout_issues().is_empty(), "{:?}", window.layout_issues());
    }
}

#[test]
fn a_rejected_choice_does_not_become_the_selected_value() {
    for kind in [Chooser::Select, Chooser::Combo, Chooser::Popup] {
        let mut window = Window::headless(move || scene(kind, true));
        kind.open(&mut window);
        window.invoke("reverse").unwrap();
        window.press_key("down").unwrap();
        window.press_key("enter").unwrap();
        assert_eq!(window.kernel().text_of("chosen").as_deref(), Some(""));
        assert_eq!(window.kernel().text_of("attempts").as_deref(), Some("1"));
        assert!(window.app().diagnostics().is_empty());
    }
}

#[test]
fn a_choice_setter_can_close_the_window_from_every_input_route() {
    use std::cell::RefCell;
    use std::rc::Rc;

    for kind in [Chooser::Select, Chooser::Combo] {
        for route in ["keyboard", "pointer", "semantic"] {
            let picks = Rc::new(RefCell::new(Vec::<Option<String>>::new()));
            let log = picks.clone();
            let mut window = Window::headless(move || {
                let log = log.clone();
                let model = (
                    || None::<String>,
                    move |id| {
                        log.borrow_mut().push(id);
                        close_window(current_window());
                    },
                );
                match kind {
                    Chooser::Select => select("pick", "Choices", model, options),
                    Chooser::Combo => combobox("pick", "Choices", model, options),
                    Chooser::Popup => unreachable!(),
                }
            });
            kind.open(&mut window);
            match route {
                "keyboard" => {
                    window.press_key("down").unwrap();
                    window.press_key("enter").unwrap();
                }
                "pointer" => window.tap("pick-option-b").unwrap(),
                "semantic" => window.dispatch(Event::Press("pick-option-b".into())).unwrap(),
                _ => unreachable!(),
            }
            assert!(window.is_closed());
            assert_eq!(*picks.borrow(), [Some("b".to_owned())]);
            assert!(window.app().diagnostics().is_empty(), "{:?}", window.app().diagnostics());
            window.app().shutdown();
        }
    }
}

#[test]
fn popup_source_updates_preserve_composition_selection_and_editor_identity() {
    let mut window = Window::headless(|| scene(Chooser::Popup, false));
    Chooser::Popup.open(&mut window);
    window.type_text("Original");
    window.set_selection(1, 5);
    window.event(InputEvent::Preedit { text: "候选".into(), selection: Some((0, 6)) });
    let before = window.inspect();
    let editor = before.find("pick").unwrap().target.clone();
    assert_eq!(before.text_input.as_ref().unwrap().composition, "候选");
    for command in ["reverse", "remove", "restore"] {
        window.invoke(command).unwrap();
        assert_eq!(target(&window, "pick"), editor);
        assert_eq!(window.inspect().focus.as_deref(), Some("pick"));
        assert_eq!(window.inspect().text_input, before.text_input, "source update {command} preserves the input method");
        for key in ["enter", "escape"] {
            window.press_key(key).unwrap();
            assert_eq!(window.inspect().text_input, before.text_input, "the input method owns {key} after {command}");
            assert!(window.kernel().exists("pick-list"));
            assert_eq!(window.kernel().text_of("attempts").as_deref(), Some("0"));
        }
    }
    assert_eq!(window.kernel().text_of("chosen").as_deref(), Some(""));
    window.event(InputEvent::Preedit { text: String::new(), selection: None });
    assert_eq!(window.text_field().unwrap().selection, 1..5);
    window.press_key("enter").unwrap();
    assert_eq!(window.kernel().text_of("chosen").as_deref(), Some("a"));
    assert_eq!(window.kernel().text_of("attempts").as_deref(), Some("1"));
    assert!(window.app().diagnostics().is_empty(), "{:?}", window.app().diagnostics());
    assert!(window.layout_issues().is_empty(), "{:?}", window.layout_issues());
}
