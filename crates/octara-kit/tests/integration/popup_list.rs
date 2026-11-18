//! Existing editors own suggestion focus, text, selection and input-method state.

use octara::app::PointerKind;
use octara::kernel::a11y::a11y_id;
use octara::prelude::*;
use octara::serde_json::json;
use octara_kit::*;

fn clean(window: &Window) {
    assert!(window.layout_issues().is_empty(), "{:?}", window.layout_issues());
    assert!(window.kernel().accessibility_issues().is_empty(), "{:?}", window.kernel().accessibility_issues());
    assert!(window.app().diagnostics().is_empty(), "{:?}", window.app().diagnostics());
}

fn choices() -> Vec<PopupItem> {
    vec![
        PopupItem::new("one", "Shared label").description("First suggestion"),
        PopupItem::new("blocked", "Unavailable").disabled(true),
        PopupItem::new("two", "Shared label").description("Second suggestion"),
        PopupItem::new("three", "Third suggestion"),
    ]
}

#[ui]
fn scene() -> View {
    let draft = state("text", || "Original".to_owned());
    let open = state("open", || false);
    let items = state("items", choices);
    let chosen = state("chosen", String::new);
    let picks = state("picks", || 0);
    let submits = state("submits", || 0);
    on_command("popup.open", move || open.set(true));
    on_command("popup.close", move || open.set(false));
    on_command("popup.reverse", move || items.update(|items| items.reverse()));
    on_command("popup.rename", move || {
        items.update(|items| items.iter_mut().find(|item| item.id == "two").unwrap().label = "Renamed".into())
    });
    on_command("popup.remove-two", move || items.update(|items| items.retain(|item| item.id != "two")));
    on_command("popup.disable-three", move || {
        items.update(|items| items.iter_mut().find(|item| item.id == "three").unwrap().disabled = true)
    });
    on_command("popup.disable-all", move || items.update(|items| items.iter_mut().for_each(|item| item.disabled = true)));
    on_command("popup.empty", move || items.set(vec![]));
    col()
        .size_full()
        .p_4()
        .gap_3()
        .child(popup_list(
            "search",
            "Suggestions",
            open,
            text_field(draft).label("Search").w_full().on_submit(move || submits.update(|n| *n += 1)),
            move || items.get(),
            move |id| {
                chosen.set(id);
                picks.update(|n| *n += 1);
            },
        ))
        .child(button("After").id("after").mt(260.0))
        .child(text(move || chosen.get()).id("chosen"))
        .child(text(move || picks.get().to_string()).id("picks"))
        .child(text(move || submits.get().to_string()).id("submits"))
        .into()
}

fn active(window: &Window, expected: Option<&str>) {
    let snapshot = window.inspect();
    assert_eq!(snapshot.focus.as_deref(), Some("search"));
    let owner = snapshot.find("search").unwrap();
    let expected = expected.map(|key| format!("search-option-{key}"));
    assert_eq!(owner.active_descendant, expected);
    let tree = window.accessibility_tree();
    let owner_id = a11y_id(window.kernel().find("search").unwrap());
    let owner = &tree.nodes.iter().find(|(node, _)| *node == owner_id).unwrap().1;
    assert_eq!(owner.active_descendant(), expected.as_ref().map(|key| a11y_id(window.kernel().find(key).unwrap())));
    if let Some(expected) = expected {
        let option = snapshot.find(&expected).unwrap();
        assert_eq!(option.role, "list_box_option");
        assert!(option.states.iter().any(|state| state == "selected"));
        assert!(!option.states.iter().any(|state| state == "focused"));
    }
}

#[test]
fn keyboard_navigation_chooses_stable_ids_without_moving_editor_focus() {
    let mut window = Window::headless(scene);
    window.tap("search").unwrap();
    let owner = window.inspect().find("search").unwrap().target.clone();
    window.press_key("down").unwrap();
    assert_eq!(window.kernel().role_of("search-list"), Some(Role::ListBox));
    assert_eq!(window.inspect().find("search").unwrap().expanded, Some(true));
    active(&window, Some("one"));
    window.press_key("down").unwrap();
    active(&window, Some("two"));
    window.press_key("up").unwrap();
    active(&window, Some("one"));
    window.press_key("up").unwrap();
    active(&window, Some("three"));
    window.press_key("down").unwrap();
    active(&window, Some("one"));
    window.press_key("down").unwrap();
    active(&window, Some("two"));
    window.press_key("enter").unwrap();
    assert_eq!(window.kernel().text_of("chosen").as_deref(), Some("two"), "identical labels do not identify choices");
    assert_eq!(window.kernel().text_of("picks").as_deref(), Some("1"));
    assert_eq!(window.kernel().text_of("submits").as_deref(), Some("0"));
    assert_eq!(window.kernel().value_of("search").as_deref(), Some("Original"), "the caller owns text replacement");
    assert!(!window.kernel().exists("search-list"));
    assert_eq!(window.inspect().find("search").unwrap().target, owner);
    assert_eq!(window.inspect().find("search").unwrap().expanded, Some(false));
    active(&window, None);
    clean(&window);
}

#[test]
fn escape_tab_and_blur_close_without_committing_and_editor_keys_keep_working() {
    let mut window = Window::headless(scene);
    window.tap("search").unwrap();
    window.invoke("popup.open").unwrap();
    window.set_selection(2, 6);
    window.press_key("shift+left").unwrap();
    assert!(window.kernel().exists("search-list"));
    active(&window, Some("one"));
    window.press_key("home").unwrap();
    assert_eq!(window.text_field().unwrap().selection, 0..0);
    active(&window, Some("one"));
    window.press_key("escape").unwrap();
    active(&window, None);
    assert!(!window.kernel().exists("search-list"));
    window.invoke("popup.open").unwrap();
    window.press_key("tab").unwrap();
    assert_eq!(window.inspect().focus.as_deref(), Some("after"));
    assert!(!window.kernel().exists("search-list"));
    window.tap("search").unwrap();
    window.invoke("popup.open").unwrap();
    window.tap("after").unwrap();
    assert_eq!(window.inspect().focus.as_deref(), Some("after"));
    assert!(!window.kernel().exists("search-list"));
    assert_eq!(window.kernel().text_of("picks").as_deref(), Some("0"));
    clean(&window);
}

#[test]
fn source_changes_retain_rows_and_reconcile_the_active_relation() {
    let mut window = Window::headless(scene);
    window.tap("search").unwrap();
    window.invoke("popup.open").unwrap();
    window.press_key("down").unwrap();
    active(&window, Some("two"));
    let owner = window.inspect().find("search").unwrap().target.clone();
    let row = window.inspect().find("search-option-two").unwrap().target.clone();
    let nodes = window.kernel().live_nodes();
    for _ in 0..9 {
        window.invoke("popup.reverse").unwrap();
        assert_eq!(window.inspect().find("search-option-two").unwrap().target, row);
        assert_eq!(window.kernel().live_nodes(), nodes);
        active(&window, Some("two"));
    }
    window.invoke("popup.rename").unwrap();
    assert_eq!(window.inspect().find("search-option-two").unwrap().name.as_deref(), Some("Renamed"));
    window.press_key("down").unwrap();
    active(&window, Some("one"));
    window.press_key("up").unwrap();
    active(&window, Some("two"));
    window.invoke("popup.remove-two").unwrap();
    assert!(!window.kernel().exists("search-option-two"));
    active(&window, Some("three"));
    window.invoke("popup.disable-three").unwrap();
    active(&window, Some("one"));
    assert_eq!(window.inspect().find("search").unwrap().target, owner);
    assert_eq!(window.kernel().text_of("picks").as_deref(), Some("0"));
    clean(&window);
}

#[test]
fn mouse_and_touch_choices_preserve_selection_editor_identity_and_undo() {
    for kind in [PointerKind::Mouse, PointerKind::Touch] {
        let mut window = Window::headless(scene);
        window.tap("search").unwrap();
        window.press_key("mod+a").unwrap();
        window.type_text("Edited");
        window.set_selection(1, 5);
        let before = window.inspect();
        let target = before.find("search").unwrap().target.clone();
        window.invoke("popup.open").unwrap();
        window.tap_with("search-option-two", kind).unwrap();
        assert_eq!(window.inspect().focus.as_deref(), Some("search"));
        assert_eq!(window.inspect().find("search").unwrap().target, target);
        assert_eq!(window.text_field().unwrap().selection, 1..5);
        assert_eq!(window.kernel().text_of("chosen").as_deref(), Some("two"));
        assert_eq!(window.kernel().text_of("picks").as_deref(), Some("1"));
        assert!(!window.kernel().exists("search-list"));
        window.press_key("mod+z").unwrap();
        assert_eq!(window.kernel().value_of("search").as_deref(), Some("Original"));
        clean(&window);
    }
}

#[test]
fn composing_input_keeps_navigation_and_pointer_choices_with_the_ime() {
    let mut window = Window::headless(scene);
    window.tap("search").unwrap();
    window.set_selection(1, 5);
    window.invoke("popup.open").unwrap();
    window.event(InputEvent::Preedit { text: "候选".into(), selection: Some((0, 6)) });
    let before = window.inspect();
    for key in ["down", "up", "right", "left", "home", "end", "enter", "escape", "tab", "space"] {
        window.press_key(key).unwrap();
        active(&window, Some("one"));
        assert_eq!(window.inspect().text_input, before.text_input, "IME owns {key}");
    }
    let target = before.find("search").unwrap().target.clone();
    let request = json!({"op":"key", "chord":"space", "target":target}).to_string();
    let reply = octara::inspect::handle_line(&mut window, &request);
    assert!(reply.ok, "{reply:?}");
    assert_eq!(window.inspect().text_input, before.text_input, "guarded Inspector Space preserves marked text");
    active(&window, Some("one"));
    for kind in [PointerKind::Mouse, PointerKind::Touch] {
        window.tap_with("search-option-two", kind).unwrap();
        assert_eq!(window.inspect().text_input, before.text_input);
        active(&window, Some("one"));
        assert!(window.kernel().exists("search-list"));
    }
    assert_eq!(window.kernel().text_of("picks").as_deref(), Some("0"));
    assert_eq!(window.kernel().text_of("submits").as_deref(), Some("0"));
    window.event(InputEvent::Preedit { text: String::new(), selection: None });
    window.press_key("down").unwrap();
    active(&window, Some("two"));
    window.press_key("enter").unwrap();
    assert_eq!(window.kernel().text_of("chosen").as_deref(), Some("two"));
    clean(&window);
}

#[test]
fn empty_and_disabled_only_lists_do_not_choose_or_submit() {
    for command in ["popup.empty", "popup.disable-all"] {
        let mut window = Window::headless(scene);
        window.tap("search").unwrap();
        window.invoke(command).unwrap();
        window.press_key("down").unwrap();
        active(&window, None);
        if command == "popup.empty" {
            assert_eq!(window.kernel().text_of("search-empty").as_deref(), Some("No suggestions"));
        } else {
            assert!(window.dispatch(Event::Press("search-option-one".into())).is_err());
        }
        for key in ["up", "down", "enter"] {
            window.press_key(key).unwrap();
            active(&window, None);
        }
        assert_eq!(window.kernel().text_of("picks").as_deref(), Some("0"));
        assert_eq!(window.kernel().text_of("submits").as_deref(), Some("0"));
        window.press_key("escape").unwrap();
        assert!(!window.kernel().exists("search-list"));
        clean(&window);
    }
}

#[test]
fn popup_clicks_belong_to_their_nested_dialog_and_outside_clicks_close_the_right_scope() {
    let mut window = Window::headless(|| {
        let outer = state("outer", || false);
        let inner = state("inner", || false);
        let suggestions = state("suggestions", || false);
        let draft = state("draft", String::new);
        let chosen = state("chosen", String::new);
        on_command("popup.open", move || suggestions.set(true));
        col()
            .size_full()
            .p_4()
            .child(button("Open outer").id("open").on_press(move || outer.set(true)))
            .child(dialog("outer", "Outer", outer, DialogOptions::default(), move || {
                col().w_full().child(button("Open inner").id("open-inner").on_press(move || inner.set(true))).child(dialog(
                    "inner",
                    "Inner",
                    inner,
                    DialogOptions::default(),
                    move || {
                        row()
                            .w_full()
                            .gap_3()
                            .items_start()
                            .child(popup_list(
                                "search",
                                "Suggestions",
                                suggestions,
                                text_field(draft).label("Search").w(220.0).autofocus(),
                                choices,
                                move |id| chosen.set(id),
                            ))
                            .child(button("Inside").id("inside"))
                    },
                ))
            }))
            .child(text(move || chosen.get()).id("chosen"))
    });
    window.tap("open").unwrap();
    window.tap("open-inner").unwrap();
    assert_eq!(window.inspect().focus.as_deref(), Some("search"));
    window.invoke("popup.open").unwrap();
    window.tap("search-option-two").unwrap();
    assert_eq!(window.kernel().text_of("chosen").as_deref(), Some("two"));
    assert_eq!(window.inspect().focus.as_deref(), Some("search"));
    assert!(window.kernel().exists("inner") && window.kernel().exists("outer"));
    window.invoke("popup.open").unwrap();
    window.tap("inside").unwrap();
    assert!(!window.kernel().exists("search-list"));
    assert!(window.kernel().exists("inner"));
    window.tap("search").unwrap();
    window.invoke("popup.open").unwrap();
    window.click_at(4.0, 4.0);
    assert!(!window.kernel().exists("inner"));
    assert!(window.kernel().exists("outer"));
    assert!(!window.kernel().exists("search-list"));
    assert_eq!(window.inspect().focus.as_deref(), Some("open-inner"));
    window.press_key("escape").unwrap();
    assert!(!window.kernel().exists("outer"));
    assert_eq!(window.inspect().focus.as_deref(), Some("open"));
    clean(&window);
}

#[test]
fn suggestion_popups_fit_narrow_windows_in_both_appearances() {
    for width in [240.0, 390.0, 800.0] {
        for theme in [Theme::light(), Theme::dark()] {
            let mut window = Window::headless(scene);
            window.app().set_target_platform(octara::app::TargetPlatform::Ios);
            window.resize(width, 640.0);
            window.app().set_theme(theme);
            window.tap("search").unwrap();
            window.invoke("popup.open").unwrap();
            let snapshot = window.inspect();
            let [x, y, w, h] = snapshot.find("search-list").unwrap().bounds;
            assert!(x >= 0.0 && y >= 0.0 && x + w <= width + 0.1 && y + h <= 640.1, "width={width}, popup=({x}, {y}, {w}, {h})");
            for id in ["one", "blocked", "two", "three"] {
                assert!(snapshot.find(&format!("search-option-{id}")).unwrap().bounds[3] >= 44.0);
            }
            clean(&window);
        }
    }
}
