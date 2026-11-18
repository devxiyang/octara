//! Drawn command menus: native keyboard rules, snapshot identity and restored command context.

use octara::app::{Event, InputEvent};
use octara::prelude::*;
use octara_kit::*;
use std::time::Duration;

fn clean(window: &Window) {
    assert!(window.layout_issues().is_empty(), "{:?}", window.layout_issues());
    assert!(window.kernel().accessibility_issues().is_empty(), "{:?}", window.kernel().accessibility_issues());
    assert!(window.app().diagnostics().is_empty(), "{:?}", window.app().diagnostics());
}

fn focused(window: &Window) -> Option<String> {
    window.inspect().focus
}

fn actions() -> Menu {
    Menu::new("Actions").items([
        MenuItem::action("Alpha", "app.alpha").shortcut("mod+a"),
        MenuItem::action("Beta", "app.beta").disabled(),
        MenuItem::separator(),
        MenuItem::action("Alpine", "app.alpine"),
        MenuItem::action("Wrap", "app.wrap").checked(true),
        MenuItem::submenu(Menu::new("More").items([
            MenuItem::action("Copy", "app.copy"),
            MenuItem::action("Cut", "app.cut").disabled(),
            MenuItem::submenu(Menu::new("Nested").item(MenuItem::action("Deep", "app.deep"))),
        ])),
        MenuItem::submenu(Menu::new("More").item(MenuItem::action("Other", "app.other"))),
        MenuItem::submenu(Menu::new("Unavailable").item(MenuItem::action("Hidden action", "app.hidden"))).disabled(),
    ])
}

#[ui]
fn commands() -> View {
    let open = state("open", || false);
    let source = state("source", actions);
    let result = state("result", || "none".to_owned());
    for command in ["app.alpha", "app.beta", "app.alpine", "app.wrap", "app.copy", "app.deep", "app.other"] {
        on_command(command, move || result.set(command.to_owned()));
    }
    on_command("menu.open", move || open.set(true));
    on_command("menu.close", move || open.set(false));
    on_command("menu.change", move || {
        source.update(|menu| {
            if let MenuItem::Action { label, .. } = &mut menu.items[0] {
                *label = "Changed alpha".into();
            }
            menu.items.reverse();
        });
    });
    col()
        .size_full()
        .p_4()
        .gap_3()
        .child(menu_button("actions", "Actions", open, move || source.get()))
        .child(button("Outside").id("outside"))
        .child(text(move || result.get()).id("result"))
        .into()
}

fn open_from_keyboard(window: &mut Window) {
    window.dispatch(Event::Focus("actions".into())).unwrap();
    window.press_key("down").unwrap();
}

#[test]
fn edit_menu_uses_the_context_it_will_restore_on_pick() {
    let mut window = Window::headless(|| {
        let draft = state("draft", || "Draft".to_owned());
        let open = state("edit-menu", || false);
        col().size_full().p_4().gap_4().child(text_field(draft).id("editor").label("Name")).child(menu_button(
            "edit",
            "Edit",
            open,
            Menu::edit,
        ))
    });
    window.tap("editor").unwrap();
    window.tap("edit").unwrap();
    let undo = "edit-action-edit.undo";
    assert!(window.kernel().state_of(undo).unwrap().contains(State::DISABLED));
    window.press_key("escape").unwrap();
    assert_eq!(focused(&window).as_deref(), Some("editor"));
    window.type_text("!");
    window.tap("edit").unwrap();
    assert!(!window.kernel().state_of(undo).unwrap().contains(State::DISABLED));
    window.tap(undo).unwrap();
    assert_eq!(focused(&window).as_deref(), Some("editor"));
    assert_eq!(window.inspect().text_input.unwrap().text, "Draft");
    assert!(!window.command_enabled("edit.undo"));
    clean(&window);
    window.app().shutdown();
}

#[test]
fn menu_navigation_skips_disabled_rows_and_submenus_follow_native_horizontal_keys() {
    let mut window = Window::headless(commands);
    open_from_keyboard(&mut window);
    assert_eq!(window.kernel().role_of("actions-menu"), Some(Role::Menu));
    assert_eq!(window.kernel().expanded_of("actions"), Some(true));
    assert_eq!(focused(&window).as_deref(), Some("actions-action-app.alpha"));
    window.press_key("right").unwrap();
    assert_eq!(focused(&window).as_deref(), Some("actions-action-app.alpha"), "Right is inert on a leaf");
    window.press_key("left").unwrap();
    assert_eq!(focused(&window).as_deref(), Some("actions-action-app.alpha"), "Left is inert in a top-level menu");
    window.press_key("down").unwrap();
    assert_eq!(focused(&window).as_deref(), Some("actions-action-app.alpine"));
    window.press_key("down").unwrap();
    assert_eq!(window.kernel().role_of("actions-action-app.wrap"), Some(Role::MenuItemCheckBox));
    assert_eq!(window.kernel().checked_of("actions-action-app.wrap"), Some(true));
    window.press_key("down").unwrap();
    assert_eq!(focused(&window).as_deref(), Some("actions-submenu-5"));
    window.press_key("right").unwrap();
    assert_eq!(focused(&window).as_deref(), Some("actions-submenu-5-action-app.copy"));
    assert_eq!(window.kernel().expanded_of("actions-submenu-5"), Some(true));
    assert_eq!(window.kernel().expanded_of("actions-submenu-6"), Some(false), "same-label sibling submenu state is independent");
    window.press_key("down").unwrap();
    assert_eq!(focused(&window).as_deref(), Some("actions-submenu-5-submenu-2"));
    window.press_key("right").unwrap();
    assert_eq!(focused(&window).as_deref(), Some("actions-submenu-5-submenu-2-action-app.deep"));
    window.press_key("left").unwrap();
    assert_eq!(focused(&window).as_deref(), Some("actions-submenu-5-submenu-2"));
    assert!(!window.kernel().exists("actions-submenu-5-submenu-2-menu"));
    window.press_key("escape").unwrap();
    assert_eq!(focused(&window).as_deref(), Some("actions-submenu-5"));
    assert!(window.kernel().exists("actions-menu"));
    window.press_key("right").unwrap();
    window.press_key("enter").unwrap();
    assert!(!window.kernel().exists("actions-menu"));
    assert_eq!(window.kernel().text_of("result").as_deref(), Some("app.copy"));
    assert_eq!(focused(&window).as_deref(), Some("actions"));
    clean(&window);
}

#[test]
fn menu_typeahead_cycles_matches_builds_prefixes_and_never_activates() {
    let mut window = Window::headless(commands);
    open_from_keyboard(&mut window);
    window.press_key("a").unwrap();
    assert_eq!(focused(&window).as_deref(), Some("actions-action-app.alpine"));
    window.press_key("a").unwrap();
    assert_eq!(focused(&window).as_deref(), Some("actions-action-app.alpha"));
    for key in ["l", "p", "i"] {
        window.press_key(key).unwrap();
    }
    assert_eq!(focused(&window).as_deref(), Some("actions-action-app.alpine"));
    window.app().advance(Duration::from_millis(701));
    window.press_key("w").unwrap();
    assert_eq!(focused(&window).as_deref(), Some("actions-action-app.wrap"));
    window.press_key("b").unwrap();
    assert_eq!(focused(&window).as_deref(), Some("actions-action-app.wrap"), "disabled labels are skipped");
    assert_eq!(window.kernel().text_of("result").as_deref(), Some("none"));
    window.press_key("end").unwrap();
    assert_eq!(focused(&window).as_deref(), Some("actions-submenu-6"));
    window.press_key("down").unwrap();
    assert_eq!(focused(&window).as_deref(), Some("actions-action-app.alpha"), "vertical navigation wraps");
    window.press_key("home").unwrap();
    assert_eq!(focused(&window).as_deref(), Some("actions-action-app.alpha"));
    window.press_key("escape").unwrap();
    window.press_key("up").unwrap();
    assert_eq!(focused(&window).as_deref(), Some("actions-submenu-6"), "Up opens on the last enabled row");
    clean(&window);
}

#[test]
fn opening_snapshots_preserve_focused_identity_and_refresh_only_on_reopen() {
    let mut window = Window::headless(commands);
    open_from_keyboard(&mut window);
    let target = window.inspect().find("actions-action-app.alpha").unwrap().target.clone();
    window.invoke("menu.change").unwrap();
    let snapshot = window.inspect();
    let alpha = snapshot.find("actions-action-app.alpha").unwrap();
    assert_eq!(alpha.name.as_deref(), Some("Alpha"));
    assert_eq!(alpha.target, target);
    window.press_key("enter").unwrap();
    assert_eq!(
        window.kernel().text_of("result").as_deref(),
        Some("app.alpha"),
        "the focused action stays the same when the source reorders"
    );
    window.press_key("down").unwrap();
    let snapshot = window.inspect();
    let alpha = snapshot.find("actions-action-app.alpha").unwrap();
    assert_eq!(alpha.name.as_deref(), Some("Changed alpha"));
    assert_ne!(alpha.target, target);
    assert_eq!(focused(&window).as_deref(), Some("actions-submenu-1"));
    window.press_key("right").unwrap();
    assert_eq!(focused(&window).as_deref(), Some("actions-submenu-1-action-app.other"));
    clean(&window);
}

#[test]
fn outside_and_escape_dismiss_only_the_top_menu_and_external_open_is_controlled() {
    let mut window = Window::headless(commands);
    window.tap("outside").unwrap();
    window.invoke("menu.open").unwrap();
    assert_eq!(focused(&window).as_deref(), Some("actions-action-app.alpha"));
    assert!(window.dispatch(Event::Press("actions-action-app.beta".into())).is_err());
    window.press_key("end").unwrap();
    window.press_key("right").unwrap();
    window.click_at(790.0, 590.0);
    assert!(!window.kernel().exists("actions-submenu-6-menu"));
    assert!(window.kernel().exists("actions-menu"));
    assert_eq!(focused(&window).as_deref(), Some("actions-submenu-6"));
    window.press_key("escape").unwrap();
    assert_eq!(focused(&window).as_deref(), Some("outside"));
    window.invoke("menu.open").unwrap();
    window.click_at(790.0, 590.0);
    assert!(!window.kernel().exists("actions-menu"));
    assert_eq!(focused(&window).as_deref(), Some("outside"));
    window.invoke("menu.open").unwrap();
    window.invoke("menu.close").unwrap();
    assert!(!window.kernel().exists("actions-menu"));
    assert_eq!(window.kernel().text_of("result").as_deref(), Some("none"));
    clean(&window);
}

#[test]
fn menu_commands_run_on_the_restored_editor_context_and_keep_its_selection() {
    let mut window = Window::headless(|| {
        let open = state("open", || false);
        let value = state("value", || "one two".to_owned());
        let result = state("result", || "none".to_owned());
        col()
            .size_full()
            .p_4()
            .gap_2()
            .child(text_field(value).id("editor").label("Editor").on_command("editor.context", move || result.set("editor".into())))
            .child(menu_button("edit", "Edit", open, || {
                Menu::new("Edit").items([MenuItem::action("Copy", "edit.copy"), MenuItem::action("Context", "editor.context")])
            }))
            .child(text(move || result.get()).id("result"))
    });
    window.tap("editor").unwrap();
    window.set_selection(4, 7);
    let target = window.inspect().find("editor").unwrap().target.clone();
    window.tap("edit").unwrap();
    window.tap("edit-action-edit.copy").unwrap();
    assert_eq!(focused(&window).as_deref(), Some("editor"));
    assert_eq!(window.clipboard_text().as_deref(), Some("two"));
    assert_eq!(window.text_field().unwrap().selection, 4..7);
    assert_eq!(window.inspect().find("editor").unwrap().target, target);
    window.tap("edit").unwrap();
    window.tap("edit-action-editor.context").unwrap();
    assert_eq!(window.kernel().text_of("result").as_deref(), Some("editor"));
    clean(&window);
}

#[test]
fn menu_inside_a_dialog_restores_the_editor_without_dismissing_its_enclosing_modal() {
    let mut window = Window::headless(|| {
        let open = state("dialog", || false);
        let menu = state("menu", || false);
        let value = state("value", || "draft".to_owned());
        col().size_full().p_4().child(button("Open").id("open").on_press(move || open.set(true))).child(dialog(
            "dialog",
            "Editor",
            open,
            DialogOptions::default(),
            move || {
                col().gap_2().child(text_field(value).id("editor").label("Editor").w_full()).child(menu_button(
                    "edit",
                    "Edit",
                    menu,
                    || Menu::new("Edit").item(MenuItem::action("Copy", "edit.copy")),
                ))
            },
        ))
    });
    window.tap("open").unwrap();
    window.press_key("mod+a").unwrap();
    window.tap("edit").unwrap();
    assert!(window.tap("dialog-close").is_err());
    window.press_key("enter").unwrap();
    assert!(window.kernel().exists("dialog"));
    assert!(!window.kernel().exists("edit-menu"));
    assert_eq!(focused(&window).as_deref(), Some("editor"));
    assert_eq!(window.clipboard_text().as_deref(), Some("draft"));
    window.press_key("escape").unwrap();
    assert!(!window.kernel().exists("dialog"));
    clean(&window);
}

#[test]
fn context_menu_requests_share_the_submenu_keyboard_and_typeahead_implementation() {
    let mut window = Window::headless(|| {
        let result = state("result", String::new);
        col()
            .size_full()
            .p_4()
            .child(button("Context").id("context").on_secondary_press(move || {
                spawn(async move {
                    result.set(popup_menu(actions(), None).await.unwrap_or_default());
                });
            }))
            .child(text(move || result.get()).id("result"))
    });
    window.secondary_tap("context").unwrap();
    window.press_key("m").unwrap();
    assert_eq!(focused(&window).as_deref(), Some("menu:More"));
    window.press_key("right").unwrap();
    assert_eq!(focused(&window).as_deref(), Some("menu:app.copy"));
    window.press_key("left").unwrap();
    assert_eq!(focused(&window).as_deref(), Some("menu:More"));
    window.press_key("right").unwrap();
    window.press_key("enter").unwrap();
    assert!(window.asks().is_empty());
    assert_eq!(window.kernel().text_of("result").as_deref(), Some("app.copy"));
    clean(&window);
}

#[test]
fn a_submenu_without_enabled_items_still_handles_left_and_escape() {
    let mut window = Window::headless(|| {
        let open = state("open", || false);
        col().size_full().p_4().child(menu_button("empty", "Menu", open, || {
            Menu::new("Menu").item(MenuItem::submenu(Menu::new("Unavailable").item(MenuItem::action("Unavailable", "missing").disabled())))
        }))
    });
    window.tap("empty").unwrap();
    window.press_key("right").unwrap();
    assert_eq!(focused(&window).as_deref(), Some("empty-submenu-0-menu"));
    window.press_key("left").unwrap();
    assert_eq!(focused(&window).as_deref(), Some("empty-submenu-0"));
    assert!(!window.kernel().exists("empty-submenu-0-menu"));
    window.press_key("right").unwrap();
    window.press_key("escape").unwrap();
    assert!(!window.kernel().exists("empty-submenu-0-menu"));
    assert!(window.kernel().exists("empty-menu"));
    clean(&window);
}

#[test]
fn finite_menus_fit_short_narrow_touch_windows_in_both_themes() {
    for theme in [Theme::light(), Theme::dark()] {
        let mut window = Window::headless(commands);
        window.resize(320.0, 480.0);
        window.app().set_theme(theme);
        window.app().set_target_platform(octara::app::TargetPlatform::Ios);
        open_from_keyboard(&mut window);
        clean(&window);
        window.press_key("end").unwrap();
        window.press_key("right").unwrap();
        clean(&window);
        window.press_key("escape").unwrap();
        window.press_key("escape").unwrap();
        clean(&window);
    }
}

#[test]
fn deferred_commands_settle_explicit_focus_and_each_preceding_command_before_routing() {
    let mut window = Window::headless(|| {
        let result = state("result", String::new);
        col()
            .size_full()
            .p_4()
            .gap_2()
            .child(button("First").id("first").on_command("choose.second", || focus("second")))
            .child(button("Second").id("second").on_command("context", move || result.set("second".into())))
            .child(button("Queue").id("queue").on_press(|| {
                focus("first");
                invoke_after_update("choose.second");
                invoke_after_update("context");
            }))
            .child(text(move || result.get()).id("result"))
    });
    window.tap("queue").unwrap();
    assert_eq!(focused(&window).as_deref(), Some("second"));
    assert_eq!(window.kernel().text_of("result").as_deref(), Some("second"));
    clean(&window);
}

#[test]
fn unavailable_or_runaway_deferred_commands_report_and_leave_no_work_queued() {
    let mut window = Window::headless(|| {
        let count = state("count", || 0);
        on_command("loop", move || {
            count.update(|n| *n += 1);
            invoke_after_update("loop");
        });
        col()
            .size_full()
            .child(button("Loop").id("loop").on_press(|| invoke_after_update("loop")))
            .child(button("Unavailable").id("unavailable").on_press(|| invoke_after_update("missing.command")))
            .child(text(move || count.get().to_string()).id("count"))
    });
    window.tap("unavailable").unwrap();
    assert!(window.app().diagnostics().iter().any(|diagnostic| diagnostic.code == "COMMAND_UNHANDLED"));
    window.tap("loop").unwrap();
    assert_eq!(window.kernel().text_of("count").as_deref(), Some("16"));
    assert!(window.app().diagnostics().iter().any(|diagnostic| diagnostic.code == "COMMAND_QUEUE_LIMIT"));
    window.app().advance(Duration::from_millis(1));
    assert_eq!(window.kernel().text_of("count").as_deref(), Some("16"), "a limited command does not restart next frame");
    assert!(window.app().run_until_idle(Duration::from_secs(1)).is_ok());
}

#[test]
fn closing_the_window_discards_its_deferred_commands() {
    let calls = std::rc::Rc::new(std::cell::Cell::new(0));
    let observed = calls.clone();
    let mut window = Window::headless(move || {
        let calls = calls.clone();
        on_command("closed.run", move || calls.set(calls.get() + 1));
        col().size_full().child(button("Close").id("close").on_press(|| {
            invoke_after_update("closed.run");
            close_window(current_window());
        }))
    });
    window.tap("close").unwrap();
    assert!(window.is_closed());
    assert_eq!(observed.get(), 0);
}

#[test]
fn a_hidden_or_disabled_retained_editor_stops_receiving_text_and_semantic_focus() {
    let mut window = Window::headless(|| {
        let hidden = state("hidden", || false);
        let disabled = state("disabled", || false);
        let draft = state("draft", || "Original".to_owned());
        on_command("editor.hide", move || hidden.set(true));
        on_command("editor.show", move || hidden.set(false));
        on_command("editor.disable", move || disabled.set(true));
        col()
            .size_full()
            .p_4()
            .child(
                col()
                    .role(Role::TabPanel)
                    .label("Editor")
                    .style_with(move |p| if hidden.get() { p.hidden() } else { p.flex() })
                    .child(text_field(draft).id("editor").label("Editor").disabled(move || disabled.get())),
            )
            .child(text(move || draft.get()).id("draft"))
    });
    window.tap("editor").unwrap();
    window.event(InputEvent::Preedit { text: "候选".into(), selection: None });
    window.invoke("editor.hide").unwrap();
    assert!(focused(&window).is_none());
    assert!(window.kernel().exists("editor"), "the panel is retained");
    window.type_text("hidden edit");
    assert_eq!(window.kernel().text_of("draft").as_deref(), Some("Original"));
    assert!(window.dispatch(Event::Focus("editor".into())).is_err());
    window.invoke("editor.show").unwrap();
    window.tap("editor").unwrap();
    window.invoke("editor.disable").unwrap();
    assert!(focused(&window).is_none());
    window.type_text("disabled edit");
    assert_eq!(window.kernel().text_of("draft").as_deref(), Some("Original"));
    assert!(window.dispatch(Event::Focus("editor".into())).is_err());
    clean(&window);
}

#[test]
fn a_deferred_command_that_hides_the_editor_retires_its_context_before_the_next_command() {
    let mut window =
        Window::headless(|| {
            let hidden = state("hidden", || false);
            let draft = state("draft", String::new);
            let result = state("result", String::new);
            on_command("hide", move || hidden.set(true));
            on_command("context", move || result.set("application".into()));
            col()
                .size_full()
                .p_4()
                .gap_2()
                .child(col().style_with(move |p| if hidden.get() { p.hidden() } else { p.flex() }).child(
                    text_field(draft).id("editor").label("Editor").on_command("context", move || result.set("hidden editor".into())),
                ))
                .child(button("Queue").id("queue").preserve_focus_on_press(true).on_press(|| {
                    invoke_after_update("hide");
                    invoke_after_update("context");
                }))
                .child(text(move || result.get()).id("result"))
        });
    window.tap("editor").unwrap();
    window.tap("queue").unwrap();
    assert_eq!(window.kernel().text_of("result").as_deref(), Some("application"));
    assert!(focused(&window).is_none());
    clean(&window);
}

#[test]
fn a_task_or_focus_callback_closing_the_window_cancels_commands_before_dispatch() {
    for close_from_focus in [false, true] {
        let calls = std::rc::Rc::new(std::cell::Cell::new(0));
        let observed = calls.clone();
        let mut window = Window::headless(move || {
            let calls = calls.clone();
            on_command("closed.run", move || calls.set(calls.get() + 1));
            col()
                .size_full()
                .p_4()
                .gap_2()
                .child(button("Closer").id("closer").on_focus(move || {
                    if close_from_focus {
                        close_window(current_window());
                    }
                }))
                .child(button("Queue").id("queue").on_press(move || {
                    if close_from_focus {
                        focus("closer");
                    } else {
                        spawn(async { close_window(current_window()) });
                    }
                    invoke_after_update("closed.run");
                }))
        });
        window.tap("queue").unwrap();
        assert!(window.is_closed());
        assert_eq!(observed.get(), 0);
    }
}

#[test]
fn semantic_focus_cannot_leave_an_open_modal() {
    let mut window = Window::headless(|| {
        let open = state("open", || false);
        col()
            .size_full()
            .p_4()
            .child(button("Outside").id("outside"))
            .child(button("Open").id("open").on_press(move || open.set(true)))
            .child(dialog("modal", "Modal", open, DialogOptions::default(), || button("Inside").id("inside")))
    });
    window.tap("open").unwrap();
    assert!(window.dispatch(Event::Focus("outside".into())).is_err());
    assert_eq!(focused(&window).as_deref(), Some("modal-close"));
    clean(&window);
}
