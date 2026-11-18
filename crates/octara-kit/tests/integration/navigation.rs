//! Tabs activate manually; compact segments use ordinary radio keyboard semantics.

use octara::accesskit::{Action, Toggled};
use octara::kernel::a11y::a11y_id;
use octara::prelude::*;
use octara_kit::*;

fn clean(window: &Window) {
    assert!(window.layout_issues().is_empty(), "{:?}", window.layout_issues());
    assert!(window.kernel().accessibility_issues().is_empty(), "{:?}", window.kernel().accessibility_issues());
    assert!(window.app().diagnostics().is_empty(), "{:?}", window.app().diagnostics());
}

fn expect_focus(window: &Window, id: &str) {
    assert_eq!(window.inspect().focus.as_deref(), Some(id));
}

fn tab_options() -> Vec<TabOption> {
    vec![
        TabOption::new("one", "First"),
        TabOption::new("two", "Second"),
        TabOption::new("blocked", "Unavailable").disabled(true),
        TabOption::new("three", "Third"),
    ]
}

#[ui]
fn tab_scene() -> View {
    let value = state("value", || Some("two".to_owned()));
    let writes = state("writes", || 0);
    let options = state("options", tab_options);
    on_command("tabs.reverse", move || options.update(|options| options.reverse()));
    on_command("tabs.rename", move || {
        options.update(|options| options.iter_mut().find(|option| option.id == "two").unwrap().label = "Renamed".into())
    });
    on_command("tabs.third", move || value.set(Some("three".into())));
    on_command("tabs.clear", move || value.set(None));
    on_command("tabs.remove-third", move || options.update(|options| options.retain(|option| option.id != "three")));
    on_command("tabs.disable-two", move || {
        options.update(|options| options.iter_mut().find(|option| option.id == "two").unwrap().disabled = true)
    });
    on_command("tabs.disable-all", move || options.update(|options| options.iter_mut().for_each(|option| option.disabled = true)));
    on_command("tabs.empty", move || options.set(vec![]));
    col()
        .size_full()
        .p_4()
        .gap_3()
        .child(button("Before").id("before"))
        .child(tabs(
            "views",
            "Views",
            (
                move || value.get(),
                move |next| {
                    writes.update(|count| *count += 1);
                    value.set(next);
                },
            ),
            move || options.get(),
        ))
        .child(button("After").id("after"))
        .child(text(move || value.get().unwrap_or_else(|| "none".into())).id("chosen"))
        .child(text(move || writes.get().to_string()).id("writes"))
        .into()
}

#[test]
fn tabs_rove_without_activation_and_are_one_tab_stop() {
    let mut window = Window::headless(tab_scene);
    assert_eq!(window.kernel().role_of("views"), Some(Role::TabList));
    window.press_key("tab").unwrap();
    expect_focus(&window, "before");
    window.press_key("tab").unwrap();
    expect_focus(&window, "views-tab-two");
    for (key, expected) in [("right", "three"), ("right", "one"), ("end", "three"), ("home", "one"), ("left", "three")] {
        window.press_key(key).unwrap();
        expect_focus(&window, &format!("views-tab-{expected}"));
        assert_eq!(window.kernel().text_of("chosen").as_deref(), Some("two"));
        assert_eq!(window.kernel().text_of("writes").as_deref(), Some("0"), "arrows never activate a tab");
    }
    window.press_key("enter").unwrap();
    assert_eq!(window.kernel().text_of("chosen").as_deref(), Some("three"));
    assert_eq!(window.kernel().text_of("writes").as_deref(), Some("1"));
    window.press_key("left").unwrap();
    expect_focus(&window, "views-tab-two");
    window.press_key("space").unwrap();
    assert_eq!(window.kernel().text_of("chosen").as_deref(), Some("two"));
    assert_eq!(window.kernel().text_of("writes").as_deref(), Some("2"));
    window.dispatch(Event::Focus("views-tab-one".into())).unwrap();
    assert_eq!(window.kernel().text_of("chosen").as_deref(), Some("two"), "semantic focus is also manual");
    window.press_key("tab").unwrap();
    expect_focus(&window, "after");
    window.press_key("shift+tab").unwrap();
    expect_focus(&window, "views-tab-two");
    let snapshot = window.inspect();
    let selected = snapshot.find("views-tab-two").unwrap();
    assert_eq!(selected.role, "tab");
    assert_eq!(selected.checked, None);
    assert!(selected.states.iter().any(|state| state == "selected"));
    let tree = window.accessibility_tree();
    let selected_id = a11y_id(window.kernel().find("views-tab-two").unwrap());
    let selected = &tree.nodes.iter().find(|(id, _)| *id == selected_id).unwrap().1;
    assert_eq!(selected.is_selected(), Some(true));
    assert_eq!(selected.toggled(), None);
    assert!(window.dispatch(Event::Press("views-tab-blocked".into())).is_err());
    clean(&window);
}

#[test]
fn dynamic_tabs_preserve_targets_and_follow_selection_and_live_order() {
    let mut window = Window::headless(tab_scene);
    let original = window.inspect().find("views-tab-two").unwrap().target.clone();
    let nodes = window.kernel().live_nodes();
    for _ in 0..19 {
        window.invoke("tabs.reverse").unwrap();
        assert_eq!(window.inspect().find("views-tab-two").unwrap().target, original);
        assert_eq!(window.kernel().live_nodes(), nodes);
    }
    window.invoke("tabs.rename").unwrap();
    assert_eq!(window.inspect().find("views-tab-two").unwrap().name.as_deref(), Some("Renamed"));
    assert_eq!(window.inspect().find("views-tab-two").unwrap().target, original);
    window.tap("before").unwrap();
    window.press_key("tab").unwrap();
    expect_focus(&window, "views-tab-two");
    window.press_key("right").unwrap();
    expect_focus(&window, "views-tab-one");
    window.press_key("tab").unwrap();
    expect_focus(&window, "after");
    window.invoke("tabs.third").unwrap();
    window.press_key("shift+tab").unwrap();
    expect_focus(&window, "views-tab-three");
    window.invoke("tabs.remove-third").unwrap();
    assert_eq!(window.kernel().text_of("chosen").as_deref(), Some("three"));
    assert!(!window.kernel().exists("views-tab-three"));
    assert!(!window.inspect().find("views").unwrap().walk().iter().any(|node| node.states.iter().any(|state| state == "selected")));
    window.tap("before").unwrap();
    window.press_key("tab").unwrap();
    expect_focus(&window, "views-tab-two");
    window.invoke("tabs.disable-two").unwrap();
    window.tap("before").unwrap();
    window.press_key("tab").unwrap();
    expect_focus(&window, "views-tab-one");
    window.invoke("tabs.clear").unwrap();
    assert_eq!(window.kernel().text_of("chosen").as_deref(), Some("none"));
    window.invoke("tabs.disable-all").unwrap();
    window.tap("before").unwrap();
    window.press_key("tab").unwrap();
    expect_focus(&window, "after");
    window.invoke("tabs.empty").unwrap();
    assert!(window.inspect().find("views").unwrap().children.is_empty());
    clean(&window);
}

#[test]
fn segmented_choices_use_ids_radio_semantics_and_one_press_per_navigation() {
    let mut window = Window::headless(|| {
        let value = state("value", || None::<String>);
        let writes = state("writes", || 0);
        col()
            .size_full()
            .p_4()
            .gap_3()
            .child(segmented(
                "mode",
                "Mode",
                (
                    move || value.get(),
                    move |next| {
                        value.set(next);
                        writes.update(|count| *count += 1);
                    },
                ),
                || {
                    vec![
                        RadioOption::new("one", "Same"),
                        RadioOption::new("blocked", "Blocked").disabled(true),
                        RadioOption::new("two", "Same").description("Second choice"),
                    ]
                },
            ))
            .child(button("After").id("after"))
            .child(text(move || value.get().unwrap_or_else(|| "none".into())).id("chosen"))
            .child(text(move || writes.get().to_string()).id("writes"))
    });
    window.press_key("tab").unwrap();
    expect_focus(&window, "mode-option-one");
    assert_eq!(window.kernel().text_of("chosen").as_deref(), Some("none"));
    for (key, expected, count) in [("home", "one", "1"), ("right", "two", "2"), ("right", "one", "3"), ("end", "two", "4")] {
        window.press_key(key).unwrap();
        expect_focus(&window, &format!("mode-option-{expected}"));
        assert_eq!(window.kernel().text_of("chosen").as_deref(), Some(expected));
        assert_eq!(window.kernel().text_of("writes").as_deref(), Some(count));
    }
    window.dispatch(Event::Focus("mode-option-one".into())).unwrap();
    assert_eq!(window.kernel().text_of("chosen").as_deref(), Some("two"));
    assert!(window.dispatch(Event::Press("mode-option-blocked".into())).is_err());
    let snapshot = window.inspect();
    assert_eq!(snapshot.find("mode").unwrap().role, "radio_group");
    let choice = snapshot.find("mode-option-two").unwrap();
    assert_eq!(choice.checked, Some(true));
    assert_eq!(choice.description.as_deref(), Some("Second choice"));
    assert!(!choice.states.iter().any(|state| state == "selected"));
    let tree = window.accessibility_tree();
    let id = a11y_id(window.kernel().find("mode-option-two").unwrap());
    let choice = &tree.nodes.iter().find(|(node, _)| *node == id).unwrap().1;
    assert_eq!(choice.toggled(), Some(Toggled::True));
    assert_eq!(choice.is_selected(), None);
    window.press_key("tab").unwrap();
    expect_focus(&window, "after");
    clean(&window);
}

#[test]
fn tabs_in_a_modal_keep_keyboard_activation_separate_and_restore_focus() {
    let mut window = Window::headless(|| {
        let open = state("open", || false);
        let value = state("value", || Some("one".to_owned()));
        col()
            .size_full()
            .p_4()
            .child(button("Open").id("open").on_press(move || open.set(true)))
            .child(show(
                move || open.get(),
                move || {
                    col()
                        .id("dialog")
                        .role(Role::Dialog)
                        .label("Views")
                        .absolute()
                        .left(40.0)
                        .top(40.0)
                        .w(320.0)
                        .p_4()
                        .bg(Token::Surface)
                        .modal(true, move || open.set(false))
                        .child(tabs("modal-tabs", "Views", value, tab_options))
                },
            ))
            .child(text(move || value.get().unwrap_or_default()).id("chosen"))
    });
    window.tap("open").unwrap();
    expect_focus(&window, "modal-tabs-tab-one");
    window.press_key("right").unwrap();
    expect_focus(&window, "modal-tabs-tab-two");
    assert_eq!(window.kernel().text_of("chosen").as_deref(), Some("one"));
    window.press_key("space").unwrap();
    assert_eq!(window.kernel().text_of("chosen").as_deref(), Some("two"));
    window.press_key("tab").unwrap();
    expect_focus(&window, "modal-tabs-tab-two");
    window.press_key("escape").unwrap();
    assert!(!window.kernel().exists("modal-tabs"));
    expect_focus(&window, "open");
    clean(&window);
}

#[test]
fn a_reentrant_tab_setter_can_remove_its_own_tab_and_move_focus() {
    let mut window = Window::headless(|| {
        let value = state("value", || Some("one".to_owned()));
        let options = state("options", tab_options);
        col()
            .size_full()
            .p_4()
            .child(tabs(
                "views",
                "Views",
                (
                    move || value.get(),
                    move |next: Option<String>| {
                        value.set(next.clone());
                        options.update(|options| options.retain(|option| Some(&option.id) != next.as_ref()));
                        focus("after");
                    },
                ),
                move || options.get(),
            ))
            .child(button("After").id("after"))
            .child(text(move || value.get().unwrap_or_default()).id("chosen"))
    });
    window.press_key("tab").unwrap();
    window.press_key("right").unwrap();
    expect_focus(&window, "views-tab-two");
    window.press_key("enter").unwrap();
    assert!(!window.kernel().exists("views-tab-two"));
    assert_eq!(window.kernel().text_of("chosen").as_deref(), Some("two"));
    expect_focus(&window, "after");
    clean(&window);
}

#[test]
fn retained_panels_keep_editor_identity_composition_and_undo_during_option_changes() {
    let mut window = Window::headless(|| {
        let selected = state("selected", || Some("notes".to_owned()));
        let draft = state("draft", || "Draft".to_owned());
        let options = state("options", || vec![TabOption::new("notes", "Notes"), TabOption::new("preview", "Preview")]);
        on_command("tabs.change-options", move || {
            options.update(|options| {
                options.reverse();
                options.iter_mut().find(|option| option.id == "notes").unwrap().label = "Writing".into();
            });
        });
        col()
            .size_full()
            .p_4()
            .gap_3()
            .child(tabs("editor", "Editor views", selected, move || options.get()))
            .child(
                col()
                    .id("editor-panel-notes")
                    .role(Role::TabPanel)
                    .label("Notes")
                    .style_with(move |p| if selected.get().as_deref() == Some("notes") { p.flex() } else { p.hidden() })
                    .child(text_field(draft).id("draft").label("Draft").w_full()),
            )
            .child(
                col()
                    .id("editor-panel-preview")
                    .role(Role::TabPanel)
                    .label("Preview")
                    .style_with(move |p| if selected.get().as_deref() == Some("preview") { p.flex() } else { p.hidden() })
                    .child(text(move || draft.get())),
            )
    });
    window.tap("draft").unwrap();
    window.press_key("mod+a").unwrap();
    window.type_text("Edited");
    window.set_selection(1, 5);
    window.event(InputEvent::Preedit { text: "候选".into(), selection: Some((0, 6)) });
    let before = window.inspect();
    let field = before.find("draft").unwrap().target.clone();
    let tab = before.find("editor-tab-notes").unwrap().target.clone();
    window.invoke("tabs.change-options").unwrap();
    let after = window.inspect();
    assert_eq!(after.text_input, before.text_input);
    assert_eq!(after.find("draft").unwrap().target, field);
    assert_eq!(after.find("editor-tab-notes").unwrap().target, tab);
    window.event(InputEvent::Preedit { text: String::new(), selection: None });
    window.tap("editor-tab-preview").unwrap();
    assert!(window.inspect().find("draft").is_none(), "hidden panels leave accessibility and Tab order");
    assert!(window.kernel().exists("draft"), "display hiding keeps the editor mounted");
    window.tap("editor-tab-notes").unwrap();
    assert_eq!(window.inspect().find("draft").unwrap().target, field);
    window.tap("draft").unwrap();
    window.press_key("mod+z").unwrap();
    assert_eq!(window.kernel().value_of("draft").as_deref(), Some("Draft"));
    clean(&window);
}

#[test]
fn navigation_wraps_narrow_labels_in_both_themes_and_touch_targets() {
    for width in [240.0, 390.0, 800.0] {
        for theme in [Theme::light(), Theme::dark()] {
            let mut window = Window::headless(|| {
                let tab = state("tab", || Some("one".to_owned()));
                let segment = state("segment", || Some("one".to_owned()));
                col()
                    .size_full()
                    .p_4()
                    .gap_3()
                    .child(tabs("views", "Views", tab, || {
                        vec![TabOption::new("one", "A longer tab label"), TabOption::new("two", "Another view")]
                    }))
                    .child(segmented("mode", "Mode", segment, || {
                        vec![RadioOption::new("one", "A longer segment label"), RadioOption::new("two", "Another choice")]
                    }))
            });
            window.app().set_target_platform(octara::app::TargetPlatform::Ios);
            window.resize(width, 844.0);
            window.app().set_theme(theme);
            for id in ["views-tab-one", "views-tab-two", "mode-option-one", "mode-option-two"] {
                assert!(window.inspect().find(id).unwrap().bounds[3] >= 44.0);
            }
            clean(&window);
        }
    }
}

#[test]
fn accessibility_activation_uses_the_same_manual_tab_handler() {
    let mut window = Window::headless(tab_scene);
    let id = a11y_id(window.kernel().find("views-tab-three").unwrap());
    assert!(window.event(InputEvent::Accessibility { action: Action::Click, target: id, data: None }));
    assert_eq!(window.kernel().text_of("chosen").as_deref(), Some("three"));
    assert_eq!(window.kernel().text_of("writes").as_deref(), Some("1"));
    clean(&window);
}

#[test]
fn controlled_choice_scopes_keep_handler_state_with_component_ids_across_reload() {
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;

    for kind in ["tabs", "segmented", "radio"] {
        let reversed = Rc::new(Cell::new(false));
        let picks = Rc::new(RefCell::new(Vec::new()));
        let retired = Rc::new(RefCell::new(Vec::new()));
        let (order, seen, cleaned) = (reversed.clone(), picks.clone(), retired.clone());
        let mut window = Window::headless(move || {
            let ids = if order.get() { ["right", "left"] } else { ["left", "right"] };
            col().size_full().children(ids.map(|id| {
                let (seen, cleaned) = (seen.clone(), cleaned.clone());
                let model = (
                    || None::<String>,
                    move |_: Option<String>| {
                        // Register once per mounted row, just as a handler can start owner-bound work.
                        let local = state("handler-count", || if id == "left" { 10 } else { 20 });
                        local.update(|n| *n += 1);
                        seen.borrow_mut().push((id, local.get()));
                        let cleaned = cleaned.clone();
                        on_cleanup(move || cleaned.borrow_mut().push(id));
                    },
                );
                match kind {
                    "tabs" => tabs(id, id, model, || vec![TabOption::new("shared", "Choice")]),
                    "segmented" => segmented(id, id, model, || vec![RadioOption::new("shared", "Choice")]),
                    _ => radio_group(id, id, model, || vec![RadioOption::new("shared", "Choice")]),
                }
            }))
        });
        let suffix = if kind == "tabs" { "tab" } else { "option" };
        for id in ["left", "right"] {
            window.tap(&format!("{id}-{suffix}-shared")).unwrap();
        }
        reversed.set(true);
        window.app().hot_reload();
        let mut old = retired.borrow().clone();
        old.sort_unstable();
        assert_eq!(old, ["left", "right"], "reload retires the old handlers' work");
        for id in ["left", "right"] {
            window.tap(&format!("{id}-{suffix}-shared")).unwrap();
        }
        assert_eq!(
            *picks.borrow(),
            [("left", 11), ("right", 21), ("left", 12), ("right", 22)],
            "{kind}: handler state follows component IDs, not sibling positions"
        );
        clean(&window);
        window.app().shutdown();
        assert_eq!(retired.borrow().len(), 4, "shutdown retires the replacement rows' work");
    }
}
