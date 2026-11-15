//! Dialog composition on the runtime's modal stack and window overlay layer.

use octara::prelude::*;
use octara_kit::*;
use std::cell::Cell;
use std::rc::Rc;

fn clean(window: &Window) {
    assert!(window.layout_issues().is_empty(), "{:?}", window.layout_issues());
    assert!(window.kernel().accessibility_issues().is_empty(), "{:?}", window.kernel().accessibility_issues());
    assert!(window.app().diagnostics().is_empty(), "{:?}", window.app().diagnostics());
}

fn focus(window: &Window) -> Option<String> {
    window.inspect().focus
}

#[component]
fn editor(click_outside: bool) -> View {
    let open = state("open", || false);
    let draft = state("draft", String::new);
    let saves = state("saves", || 0);
    on_command("dialog.open", move || open.set(true));
    on_command("dialog.close", move || open.set(false));
    col()
        .size_full()
        .p_4()
        .gap_3()
        .child(button("Edit draft").id("open").on_press(move || open.set(true)))
        .child(button("Outside").id("outside"))
        .child(text(move || saves.get().to_string()).id("saves"))
        // A dialog can be owned by a small, offset and clipped part of the application.
        .child(col().w(100.0).h(40.0).overflow_hidden().child(dialog(
            "edit",
            "Edit draft",
            open,
            DialogOptions { description: "Changes stay in this draft until you save.".into(), click_outside },
            move || {
                col()
                    .w_full()
                    .gap_2()
                    .child(text_field(draft).id("draft").label("Draft").w_full().on_submit(move || {
                        saves.update(|n| *n += 1);
                        open.set(false);
                    }))
                    .child(button("Save").id("save").on_press(move || {
                        saves.update(|n| *n += 1);
                        open.set(false);
                    }))
                    .child(button("Unavailable").id("disabled").disabled(true))
                    .into()
            },
        )))
        .into()
}

#[test]
fn dialog_names_its_modal_and_traps_then_restores_focus() {
    let mut window = Window::headless(|| editor(true));
    assert!(!window.kernel().exists("edit"));
    window.tap("open").unwrap();
    let snapshot = window.inspect();
    let panel = snapshot.find("edit").unwrap();
    assert_eq!(panel.role, "dialog");
    assert_eq!(panel.name.as_deref(), Some("Edit draft"));
    assert_eq!(panel.description.as_deref(), Some("Changes stay in this draft until you save."));
    assert_eq!(snapshot.find("edit-title").unwrap().role, "heading");
    assert_eq!(snapshot.find("edit-close").unwrap().name.as_deref(), Some("Close"));
    let tree = window.accessibility_tree();
    let id = octara::kernel::a11y::a11y_id(window.kernel().find("edit").unwrap());
    let panel = &tree.nodes.iter().find(|(node, _)| *node == id).unwrap().1;
    assert!(panel.is_modal());
    assert_eq!(panel.label(), Some("Edit draft"));
    assert_eq!(panel.description(), Some("Changes stay in this draft until you save."));
    assert_eq!(focus(&window).as_deref(), Some("draft"));
    for expected in ["save", "edit-close", "draft"] {
        window.press_key("tab").unwrap();
        assert_eq!(focus(&window).as_deref(), Some(expected));
    }
    window.press_key("shift+tab").unwrap();
    assert_eq!(focus(&window).as_deref(), Some("edit-close"));
    assert!(window.tap("outside").is_err());
    window.press_key("escape").unwrap();
    assert!(!window.kernel().exists("edit"));
    assert_eq!(focus(&window).as_deref(), Some("open"));
    assert_eq!(window.kernel().text_of("saves").as_deref(), Some("0"));
    clean(&window);
}

#[test]
fn outside_press_policy_leaves_escape_close_button_and_external_control_available() {
    for click_outside in [false, true] {
        let mut window = Window::headless(move || editor(click_outside));
        window.tap("open").unwrap();
        window.click_at(4.0, 4.0);
        assert_eq!(window.kernel().exists("edit"), !click_outside);
        if !click_outside {
            assert_eq!(focus(&window).as_deref(), Some("draft"));
            window.press_key("escape").unwrap();
        }
        assert_eq!(focus(&window).as_deref(), Some("open"));
        window.invoke("dialog.open").unwrap();
        window.tap("edit-close").unwrap();
        assert!(!window.kernel().exists("edit"));
        assert_eq!(focus(&window).as_deref(), Some("open"));
        window.invoke("dialog.open").unwrap();
        window.invoke("dialog.close").unwrap();
        assert!(!window.kernel().exists("edit"));
        assert_eq!(focus(&window).as_deref(), Some("open"));
        clean(&window);
    }
}

#[test]
fn input_method_enter_and_escape_do_not_submit_or_dismiss_the_dialog() {
    let mut window = Window::headless(|| editor(true));
    window.tap("open").unwrap();
    window.type_text("A draft");
    window.set_selection(2, 7);
    let target = window.inspect().find("draft").unwrap().target.clone();
    window.event(InputEvent::Preedit { text: "候选".into(), selection: Some((0, 6)) });
    let before = window.text_field().unwrap();
    window.press_key("enter").unwrap();
    window.press_key("escape").unwrap();
    assert!(window.kernel().exists("edit"));
    assert_eq!(window.kernel().text_of("saves").as_deref(), Some("0"));
    assert_eq!(window.inspect().find("draft").unwrap().target, target);
    let after = window.text_field().unwrap();
    assert_eq!(after.composition, before.composition);
    assert_eq!(after.composition_selection, before.composition_selection);
    window.event(InputEvent::Preedit { text: String::new(), selection: None });
    assert_eq!(window.text_field().unwrap().selection, 2..7);
    window.press_key("escape").unwrap();
    assert_eq!(focus(&window).as_deref(), Some("open"));
    window.tap("open").unwrap();
    assert_ne!(window.inspect().find("draft").unwrap().target, target, "a new opening gets a new editor allocation");
    assert_eq!(window.kernel().value_of("draft").as_deref(), Some("A draft"), "the app's draft survives content unmounting");
    window.press_key("mod+z").unwrap();
    assert_eq!(window.kernel().value_of("draft").as_deref(), Some("A draft"), "the old editor's undo history was disposed");
    clean(&window);
}

#[test]
fn closing_unmounts_content_and_reopening_resets_its_local_state() {
    let cleanups = Rc::new(Cell::new(0));
    let observed = cleanups.clone();
    let mut window = Window::headless(move || {
        let open = state("open", || false);
        let cleanups = cleanups.clone();
        col()
            .size_full()
            .p_4()
            .child(button("Open").id("open").on_press(move || open.set(true)))
            .child(dialog(
                "local",
                "Local state",
                (move || open.get(), move |value| open.set(value)),
                DialogOptions::default(),
                move || {
                    let count = state("count", || 0);
                    let cleanups = cleanups.clone();
                    on_cleanup(move || cleanups.set(cleanups.get() + 1));
                    col()
                        .gap_2()
                        .child(text(move || count.get().to_string()).id("count"))
                        .child(button("Increment").id("increment").on_press(move || count.update(|value| *value += 1)))
                        .into()
                },
            ))
            .into()
    });
    window.tap("open").unwrap();
    assert_eq!(focus(&window).as_deref(), Some("local-close"), "a dialog without a field or autofocus uses its first Tab stop");
    assert!(!window.kernel().exists("local-description"));
    window.tap("increment").unwrap();
    assert_eq!(window.kernel().text_of("count").as_deref(), Some("1"));
    window.tap("local-close").unwrap();
    assert!(!window.kernel().exists("count"));
    assert_eq!(observed.get(), 1);
    window.tap("open").unwrap();
    assert_eq!(window.kernel().text_of("count").as_deref(), Some("0"));
    clean(&window);
}

#[test]
fn nested_popup_and_dialog_dismiss_only_the_top_modal_and_restore_each_opener() {
    let mut window = Window::headless(|| {
        let outer = state("outer", || false);
        let inner = state("inner", || false);
        let popup = state("popup", || false);
        let draft = state("draft", String::new);
        col()
            .size_full()
            .p_4()
            .child(button("Open outer").id("open").on_press(move || outer.set(true)))
            .child(dialog("outer", "Outer dialog", outer, DialogOptions::default(), move || {
                col()
                    .gap_2()
                    .child(button("Open inner").id("open-inner").on_press(move || inner.set(true)).autofocus())
                    .child(popover("details", popup, Placement::BELOW, button("Details"), move || {
                        text_field(draft).id("popup-draft").label("Details").w(200.0).into()
                    }))
                    .child(dialog("inner", "Inner dialog", inner, DialogOptions::default(), move || {
                        text_field(draft).id("inner-draft").label("Draft").w_full().into()
                    }))
                    .into()
            }))
            .into()
    });
    window.tap("open").unwrap();
    assert_eq!(focus(&window).as_deref(), Some("open-inner"), "explicit autofocus overrides the built-in close button");
    window.tap("open-inner").unwrap();
    assert_eq!(focus(&window).as_deref(), Some("inner-draft"));
    assert!(window.tap("details").is_err());
    window.press_key("escape").unwrap();
    assert!(!window.kernel().exists("inner"));
    assert!(window.kernel().exists("outer"));
    assert_eq!(focus(&window).as_deref(), Some("open-inner"));
    window.tap("details").unwrap();
    assert_eq!(focus(&window).as_deref(), Some("popup-draft"));
    assert!(window.tap("outer-close").is_err());
    window.press_key("escape").unwrap();
    assert!(!window.kernel().exists("details-popover"));
    assert!(window.kernel().exists("outer"));
    assert_eq!(focus(&window).as_deref(), Some("details"));
    window.press_key("escape").unwrap();
    assert!(!window.kernel().exists("outer"));
    assert_eq!(focus(&window).as_deref(), Some("open"));
    clean(&window);
}

#[test]
fn a_long_dialog_fits_the_window_scrolls_its_body_and_keeps_close_visible_in_both_themes() {
    for (width, height) in [(320.0, 480.0), (390.0, 844.0), (800.0, 600.0)] {
        for theme in [Theme::light(), Theme::dark()] {
            let mut window = Window::headless(|| {
                let open = state("open", || false);
                col()
                    .size_full()
                    .p_4()
                    .child(button("Open").id("open").on_press(move || open.set(true)))
                    .child(dialog(
                        "long",
                        "A dialog with a descriptive title",
                        open,
                        DialogOptions {
                            description: "This content remains readable in a narrow window.".into(),
                            ..DialogOptions::default()
                        },
                        || {
                            col()
                                .gap_2()
                                .children((0..30).map(|index| text(format!("Detail {index}: content for this dialog.")).min_h(32.0)))
                                .child(button("Last action").id("last-action"))
                                .into()
                        },
                    ))
                    .into()
            });
            window.resize(width, height);
            window.app().set_theme(theme);
            window.tap("open").unwrap();
            {
                let kernel = window.kernel();
                let (x, y, w, h) = kernel.shown_rect(kernel.find("long").unwrap());
                assert!(x >= 15.5 && y >= 15.5);
                assert!(x + w <= width - 15.5 && y + h <= height - 15.5);
                assert!((x + w / 2.0 - width / 2.0).abs() <= 0.5);
                assert!((y + h / 2.0 - height / 2.0).abs() <= 0.5);
            }
            let close_target = window.inspect().find("long-close").unwrap().target.clone();
            let close_bounds = window.inspect().find("long-close").unwrap().bounds;
            let body = window.inspect().find("long-body").unwrap().bounds;
            window.wheel(body[0] + body[2] / 2.0, body[1] + body[3] / 2.0, 0.0, 300.0);
            {
                let kernel = window.kernel();
                assert!(kernel.scroll_offset(kernel.find("long-body").unwrap()).1 > 0.0);
            }
            assert_eq!(window.inspect().find("long-close").unwrap().bounds, close_bounds, "the close button stays outside the scroll body");
            clean(&window);
            window.resize(320.0, 480.0);
            assert_eq!(window.inspect().find("long-close").unwrap().target, close_target, "resizing retains the open dialog");
            window.tap("long-close").unwrap();
            assert_eq!(focus(&window).as_deref(), Some("open"));
            clean(&window);
        }
    }
}
