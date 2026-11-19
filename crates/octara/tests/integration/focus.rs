//! Focus: Tab order with `tab_index`, focus groups and arrow keys, and modals that trap focus,
//! make the window inert, and give focus back when they close.

use octara::accesskit::{self as ak, Toggled};
use octara::app::{Event, InputEvent};
use octara::inspect::{self, Delivery};
use octara::kernel::a11y::a11y_id;
use octara::prelude::*;
use serde_json::json;
use std::cell::Cell;
use std::collections::HashMap;
use std::rc::Rc;

fn focused(app: &Window) -> Option<String> {
    let k = app.kernel();
    let all = ["a", "b", "c", "late", "first", "tab-1", "tab-2", "tab-3", "row-1", "row-2", "row-3", "name", "ok", "cancel", "open"];
    all.iter().find(|id| k.state_of(id).is_some_and(|s| s.contains(State::FOCUSED))).map(|s| s.to_string())
}

fn button(id: &str) -> El {
    el().id(id.to_string()).w(px(60.0)).h(px(24.0)).focusable().on_press(|| {})
}

#[test]
fn tab_index_orders_and_skips() {
    let mut app = Window::headless(|| {
        col()
            .child(button("a"))
            .child(button("b").tab_index(-1))
            .child(button("c"))
            .child(button("late").tab_index(2))
            .child(button("first").tab_index(1))
    });
    let mut seen = Vec::new();
    for _ in 0..5 {
        app.press_key("tab").unwrap();
        seen.push(focused(&app).unwrap());
    }
    assert_eq!(seen, ["first", "late", "a", "c", "first"]);
    app.press_key("shift+tab").unwrap();
    assert_eq!(focused(&app).as_deref(), Some("c"));
    // Out of the Tab order, but still focusable by pointer.
    app.tap("b").unwrap();
    assert_eq!(focused(&app).as_deref(), Some("b"));
}

#[test]
fn focus_groups_move_on_arrows_and_stop_once_for_tab() {
    let mut app = Window::headless(|| {
        let tab = state("tab", || 2);
        col()
            .child(button("a"))
            .child(row().role(Role::TabList).focus_group(FocusGroup::both().wrap()).children((1..=3).map(move |i| {
                let selected = move || tab.get() == i;
                button(&format!("tab-{i}")).role(Role::Tab).selected(selected).focus_preferred(selected).on_press(move || tab.set(i))
            })))
            .child(
                col()
                    .role(Role::List)
                    .focus_group(FocusGroup::vertical())
                    .children((1..=3).map(|i| button(&format!("row-{i}")).role(Role::ListItem))),
            )
            .child(button("c"))
    });
    app.press_key("tab").unwrap();
    app.press_key("tab").unwrap();
    assert_eq!(focused(&app).as_deref(), Some("tab-2"), "Tab enters a group at its preferred member");
    app.press_key("right").unwrap();
    assert_eq!(focused(&app).as_deref(), Some("tab-3"));
    app.press_key("right").unwrap();
    assert_eq!(focused(&app).as_deref(), Some("tab-1"), "tabs wrap");
    app.press_key("down").unwrap();
    assert_eq!(focused(&app).as_deref(), Some("tab-2"), "Down moves too");
    app.press_key("tab").unwrap();
    assert_eq!(focused(&app).as_deref(), Some("row-1"), "one stop per group");
    app.press_key("right").unwrap();
    assert_eq!(focused(&app).as_deref(), Some("row-1"), "lists move on Up and Down only");
    app.press_key("end").unwrap();
    assert_eq!(focused(&app).as_deref(), Some("row-3"));
    app.press_key("down").unwrap();
    assert_eq!(focused(&app).as_deref(), Some("row-3"), "lists stop at their ends");
    app.press_key("shift+tab").unwrap();
    assert_eq!(focused(&app).as_deref(), Some("tab-2"));
    app.press_key("tab").unwrap();
    assert_eq!(focused(&app).as_deref(), Some("row-3"), "the group remembers its focused row");
    app.press_key("tab").unwrap();
    assert_eq!(focused(&app).as_deref(), Some("c"));
}

#[test]
fn focus_group_policy_is_independent_of_accessibility_roles() {
    let mut app = Window::headless(|| {
        let presses = state("presses", Vec::new);
        col()
            .size_full()
            .child(
                col()
                    .id("radios")
                    .role(Role::RadioGroup)
                    .focus_group(FocusGroup::vertical())
                    .child(button("radio-a").role(Role::RadioButton).checked(false).on_press(move || presses.update(|p| p.push("radio-a"))))
                    .child(button("radio-b").role(Role::RadioButton).checked(true).on_press(move || presses.update(|p| p.push("radio-b")))),
            )
            .child(
                row()
                    .id("options")
                    .role(Role::ListBox)
                    .focus_group(FocusGroup::horizontal().wrap().activate_on_navigation())
                    .child(
                        button("option-a")
                            .role(Role::ListBoxOption)
                            .selected(true)
                            .on_press(move || presses.update(|p| p.push("option-a"))),
                    )
                    .child(
                        button("option-b")
                            .role(Role::ListBoxOption)
                            .focus_preferred(true)
                            .on_press(move || presses.update(|p| p.push("option-b"))),
                    ),
            )
            .child(text(move || presses.with(|p| p.join(", "))).id("presses"))
    });
    for (key, target, presses) in [
        ("tab", "radio-a", ""),
        ("right", "radio-a", ""),
        ("down", "radio-b", ""),
        ("down", "radio-b", ""),
        ("tab", "option-b", ""),
        ("down", "option-b", ""),
        ("right", "option-a", "option-a"),
        ("right", "option-b", "option-a, option-b"),
    ] {
        app.press_key(key).unwrap();
        assert_eq!(app.inspect().focus.as_deref(), Some(target), "{key}");
        assert_eq!(app.kernel().text_of("presses").as_deref(), Some(presses), "{key}");
    }

    let tree = app.accessibility_tree();
    let node = |id: &str| {
        let id = a11y_id(app.kernel().find(id).unwrap());
        &tree.nodes.iter().find(|(n, _)| *n == id).unwrap().1
    };
    assert_eq!(node("radios").role(), Role::RadioGroup);
    assert_eq!(node("radio-b").toggled(), Some(Toggled::True));
    assert_eq!(node("options").role(), Role::ListBox);
    assert_eq!(node("option-a").is_selected(), Some(true));
    assert_eq!(node("option-b").is_selected(), None);
    assert!(app.layout_issues().is_empty(), "{:?}", app.layout_issues());
    assert!(app.app().diagnostics().is_empty(), "{:?}", app.app().diagnostics());
}

#[test]
fn nested_focus_groups_keep_distinct_tab_stops() {
    let mut app = Window::headless(|| {
        col()
            .child(
                row()
                    .focus_group(FocusGroup::horizontal())
                    .child(button("outer"))
                    .child(col().focus_group(FocusGroup::vertical()).child(button("inner-first")).child(button("inner-last"))),
            )
            .child(button("after"))
    });
    for (key, expected) in [
        ("tab", "outer"),
        ("right", "outer"),
        ("tab", "inner-first"),
        ("down", "inner-last"),
        ("tab", "after"),
        ("shift+tab", "inner-last"),
        ("shift+tab", "outer"),
    ] {
        app.press_key(key).unwrap();
        assert_eq!(app.inspect().focus.as_deref(), Some(expected), "{key}");
    }
}

#[test]
fn tab_lists_enter_at_the_current_selection_after_outside_changes() {
    let mut app = Window::headless(|| {
        let selected = state("selected", || 1);
        on_command("select-third", move || selected.set(3));
        col()
            .size_full()
            .child(button("a"))
            .child(row().role(Role::TabList).focus_group(FocusGroup::both().wrap()).children((1..=3).map(move |i| {
                let is_selected = move || selected.get() == i;
                button(&format!("tab-{i}"))
                    .role(Role::Tab)
                    .selected(is_selected)
                    .focus_preferred(is_selected)
                    .on_press(move || selected.set(i))
            })))
            .child(button("c"))
    });
    app.press_key("tab").unwrap();
    app.press_key("tab").unwrap();
    app.press_key("right").unwrap();
    assert_eq!(app.inspect().focus.as_deref(), Some("tab-2"));
    app.press_key("tab").unwrap();
    assert_eq!(app.inspect().focus.as_deref(), Some("c"), "unselected focus still represents the group's stop while inside");
    app.invoke("select-third").unwrap();
    app.press_key("shift+tab").unwrap();
    assert_eq!(app.inspect().focus.as_deref(), Some("tab-3"), "selected tab wins over stale remembered focus on outside entry");
    assert!(app.app().diagnostics().is_empty(), "{:?}", app.app().diagnostics());
}

#[test]
fn radio_groups_enter_at_the_checked_radio_and_navigation_chooses_once() {
    let mut app = Window::headless(|| {
        let choice = state("choice", || 3);
        let presses = state("presses", || 0);
        on_command("choose-first", move || choice.set(1));
        on_command("choose-third", move || choice.set(3));
        col()
            .size_full()
            .child(button("before"))
            .child(row().role(Role::RadioGroup).focus_group(FocusGroup::both().wrap().activate_on_navigation()).children((1..=4).map(
                move |i| {
                    let checked = move || choice.get() == i;
                    button(&format!("radio-{i}"))
                        .role(Role::RadioButton)
                        .disabled(i == 2)
                        .checked(checked)
                        .focus_preferred(checked)
                        .on_press(move || {
                            choice.set(i);
                            presses.update(|n| *n += 1);
                        })
                },
            )))
            .child(button("after"))
            .child(text(move || presses.get().to_string()).id("presses"))
    });
    app.press_key("tab").unwrap();
    app.press_key("tab").unwrap();
    assert_eq!(app.inspect().focus.as_deref(), Some("radio-3"));
    assert_eq!(app.kernel().text_of("presses").as_deref(), Some("0"), "Tab does not choose");
    app.press_key("shift+tab").unwrap();
    app.invoke("choose-first").unwrap();
    app.press_key("tab").unwrap();
    assert_eq!(app.inspect().focus.as_deref(), Some("radio-1"), "checked beats the previously focused radio on entry");
    app.invoke("choose-third").unwrap();
    app.press_key("tab").unwrap();
    assert_eq!(app.inspect().focus.as_deref(), Some("after"), "the focused member remains the group's current stop");
    app.press_key("shift+tab").unwrap();
    assert_eq!(app.inspect().focus.as_deref(), Some("radio-3"));
    for (key, expected, count) in [("left", "radio-1", "1"), ("left", "radio-4", "2"), ("home", "radio-1", "3"), ("end", "radio-4", "4")] {
        app.press_key(key).unwrap();
        assert_eq!(app.inspect().focus.as_deref(), Some(expected));
        assert_eq!(app.kernel().checked_of(expected), Some(true));
        assert_eq!(app.kernel().text_of("presses").as_deref(), Some(count));
    }
    app.press_key("end").unwrap();
    assert_eq!(app.kernel().text_of("presses").as_deref(), Some("4"), "staying on the same radio does not press it again");
    app.dispatch(Event::Focus("radio-1".into())).unwrap();
    assert_eq!(app.kernel().checked_of("radio-4"), Some(true), "ordinary focus does not choose");
    assert_eq!(app.kernel().text_of("presses").as_deref(), Some("4"));
    let tree = app.accessibility_tree();
    for node in app.inspect().root.walk().into_iter().filter(|n| n.role == "radio_button") {
        assert!(!node.states.iter().any(|s| s == "selected"));
        let id = a11y_id(app.kernel().find(node.id.as_deref().unwrap()).unwrap());
        assert_eq!(tree.nodes.iter().find(|(n, _)| *n == id).unwrap().1.is_selected(), None);
    }
    assert!(app.layout_issues().is_empty(), "{:?}", app.layout_issues());
    assert!(app.app().diagnostics().is_empty(), "{:?}", app.app().diagnostics());
}

#[test]
fn radio_navigation_chooses_an_unchecked_current_radio() {
    let mut app = Window::headless(|| {
        let checked = state("checked", || false);
        let presses = state("presses", || 0);
        let is_checked = move || checked.get();
        on_command("clear", move || checked.set(false));
        col()
            .size_full()
            .child(
                row()
                    .role(Role::RadioGroup)
                    .focus_group(FocusGroup::both().wrap().activate_on_navigation())
                    .child(button("a").role(Role::RadioButton).checked(is_checked).focus_preferred(is_checked).on_press(move || {
                        checked.set(true);
                        presses.update(|n| *n += 1);
                    }))
                    .child(button("b").role(Role::RadioButton).checked(false).disabled(true)),
            )
            .child(text(move || presses.get().to_string()).id("presses"))
    });
    app.press_key("tab").unwrap();
    assert_eq!(app.inspect().focus.as_deref(), Some("a"));
    assert_eq!(app.kernel().checked_of("a"), Some(false), "Tab preserves an empty choice");
    app.press_key("home").unwrap();
    assert_eq!(app.kernel().checked_of("a"), Some(true), "Home chooses an unchecked first radio");
    assert_eq!(app.kernel().text_of("presses").as_deref(), Some("1"));
    app.press_key("right").unwrap();
    assert_eq!(app.kernel().text_of("presses").as_deref(), Some("1"), "the checked singleton is not pressed again");
    app.invoke("clear").unwrap();
    app.press_key("right").unwrap();
    assert_eq!(app.kernel().checked_of("a"), Some(true), "wrapping chooses the only enabled unchecked radio");
    assert_eq!(app.kernel().text_of("presses").as_deref(), Some("2"));
    assert!(app.app().diagnostics().is_empty(), "{:?}", app.app().diagnostics());
}

#[test]
fn radio_navigation_rechecks_focus_handlers_before_pressing() {
    for (change, deferred) in ["remove", "disable", "redirect", "modal"].into_iter().flat_map(|change| [(change, false), (change, true)]) {
        let presses = Rc::new(Cell::new(0));
        let recorded = presses.clone();
        let mut app = Window::headless(move || {
            let shown = state("shown", || true);
            let disabled = state("disabled", || false);
            let modal = state("modal", || false);
            let recorded = recorded.clone();
            col()
                .size_full()
                .child(
                    row()
                        .role(Role::RadioGroup)
                        .focus_group(FocusGroup::both().wrap().activate_on_navigation())
                        .child(button("a").role(Role::RadioButton).checked(true).focus_preferred(true))
                        .child(show(
                            move || shown.get(),
                            move || {
                                let recorded = recorded.clone();
                                button("b")
                                    .role(Role::RadioButton)
                                    .checked(false)
                                    .disabled(move || disabled.get())
                                    .on_focus(move || match change {
                                        "remove" => shown.set(false),
                                        "disable" => disabled.set(true),
                                        "redirect" => focus("outside"),
                                        "modal" => modal.set(true),
                                        _ => unreachable!(),
                                    })
                                    .on_press(move || recorded.set(recorded.get() + 1))
                            },
                        )),
                )
                .child(button("outside"))
                .child(button("move").on_press(|| invoke_after_update("focus.right")))
                .child(show(move || modal.get(), || col().modal(false, || {}).child(button("modal-first"))))
        });
        app.press_key("tab").unwrap();
        if deferred {
            app.dispatch(Event::Press("move".into())).unwrap();
        } else {
            app.press_key("right").unwrap();
        }
        assert_eq!(presses.get(), 0, "{change} deferred={deferred}: focus handlers retire the pending choice");
        if change == "redirect" {
            assert_eq!(app.inspect().focus.as_deref(), Some("outside"));
        } else if change == "modal" {
            assert_eq!(app.inspect().focus.as_deref(), Some("modal-first"));
        }
        assert!(app.app().diagnostics().is_empty(), "{:?}", app.app().diagnostics());
    }
}

fn dialog_app(click_outside: bool) -> Window {
    Window::headless(move || {
        let open = state("open", || false);
        let closes = state("closes", || 0);
        col()
            .size_full()
            .child(button("a"))
            .child(button("open").on_press(move || open.set(true)))
            .child(text(move || closes.get().to_string()).id("closes"))
            .child(show(
                move || open.get(),
                move || {
                    el().id("dialog")
                        .role(Role::Dialog)
                        .absolute()
                        .top(px(200.0))
                        .left(px(200.0))
                        .w(px(200.0))
                        .h(px(120.0))
                        .modal(click_outside, move || {
                            closes.update(|n| *n += 1);
                            open.set(false)
                        })
                        .child(input().id("name").value("").on_input(|_| {}))
                        .child(button("ok").on_press(move || open.set(false)))
                        .child(button("cancel").autofocus().on_press(move || open.set(false)))
                },
            ))
    })
}

#[test]
fn a_modal_takes_focus_traps_tab_and_gives_focus_back() {
    let mut app = dialog_app(true);
    app.press_key("tab").unwrap();
    app.press_key("tab").unwrap();
    assert_eq!(focused(&app).as_deref(), Some("open"));
    app.press_key("enter").unwrap();
    assert!(app.kernel().exists("dialog"));
    assert_eq!(focused(&app).as_deref(), Some("cancel"), "autofocus");
    let mut seen = Vec::new();
    for _ in 0..4 {
        app.press_key("tab").unwrap();
        seen.push(focused(&app).unwrap());
    }
    assert_eq!(seen, ["name", "ok", "cancel", "name"], "Tab stays inside");
    app.press_key("escape").unwrap();
    assert!(!app.kernel().exists("dialog"));
    assert_eq!(app.kernel().text_of("closes").unwrap(), "1");
    assert_eq!(focused(&app).as_deref(), Some("open"), "focus goes back to the opener");
    assert!(app.kernel().state_of("open").unwrap().contains(State::FOCUS_VISIBLE), "still from the keyboard");
}

#[test]
fn the_window_behind_a_modal_is_inert() {
    let mut app = dialog_app(false);
    app.tap("open").unwrap();
    assert_eq!(focused(&app).as_deref(), Some("cancel"));
    assert!(app.tap("a").unwrap_err().contains("behind the open modal `dialog`"));
    // A press outside reaches nothing, and this modal does not close on it.
    let k = app.kernel();
    let (x, y, w, h) = k.rect(k.find("a").unwrap());
    drop(k);
    app.click_at(x + w / 2.0, y + h / 2.0);
    assert!(app.kernel().exists("dialog"));
    assert_eq!(focused(&app).as_deref(), Some("cancel"));
    // The accessibility tree marks the dialog modal.
    let tree = app.accessibility_tree();
    let id = a11y_id(app.kernel().find("dialog").unwrap());
    assert!(tree.nodes.iter().find(|(n, _)| *n == id).unwrap().1.is_modal());
}

fn anchored_dialog_app() -> Window {
    Window::headless(|| {
        let dialog_open = state("dialog-open", || false);
        let saves = state("saves", || 0);
        let clicks = state("clicks", || 0);
        let workspace = col()
            .id("workspace")
            .size_full()
            .role(Role::Group)
            .label("Workspace")
            .on_command("workspace.save", || {})
            .child(button("open-dialog").role(Role::Button).label("Open dialog").on_press(move || dialog_open.set(true)).overlay(
                || true,
                Placement::RIGHT,
                || button("outside-overlay-action").role(Role::Button).label("Outside action"),
            ))
            .child(
                col()
                    .id("background-doc")
                    .selectable()
                    .child(text(move || if saves.get() == 0 { "Save changes" } else { "Changes saved" }).id("background-label")),
            )
            .child(text(move || saves.get().to_string()).id("saves"))
            .child(text(move || clicks.get().to_string()).id("clicks"))
            .child(show(
                move || dialog_open.get(),
                move || {
                    let overlay_open = state("overlay-open", || false);
                    col()
                        .id("dialog")
                        .role(Role::Dialog)
                        .label("Fallback title")
                        .labelled_by("background-label")
                        .controls("outside-overlay-action")
                        .active_descendant("outside-overlay-action")
                        .absolute()
                        .left(160.0)
                        .top(120.0)
                        .w(200.0)
                        .h(120.0)
                        .modal(false, move || dialog_open.set(false))
                        .key_context("dialog")
                        .on_command("dialog.save", move || saves.update(|n| *n += 1))
                        .child(col().id("dialog-doc").selectable().child(text("Dialog options").id("dialog-label")))
                        .child(
                            row()
                                .gap_8()
                                .child(button("open-overlay").autofocus().on_press(move || overlay_open.set(true)).overlay(
                                    move || overlay_open.get(),
                                    Placement::BELOW,
                                    move || {
                                        col().dismissible(true, move || overlay_open.set(false)).child(
                                            button("overlay-action")
                                                .role(Role::Button)
                                                .labelled_by("dialog-label")
                                                .controls("dialog-action"),
                                        )
                                    },
                                ))
                                .child(button("dialog-action").tab_index(-1).on_press(move || clicks.update(|n| *n += 1))),
                        )
                },
            ));
        col().size_full().p_4().child(workspace)
    })
}

#[test]
fn an_anchored_overlay_shares_its_dialogs_keyboard_scope() {
    let mut app = anchored_dialog_app();
    app.app().bind_keys([("mod+s", "dialog.save", Some("dialog"))]).unwrap();
    app.tap("open-dialog").unwrap();
    app.press_key("enter").unwrap();
    assert_eq!(app.inspect().focus.as_deref(), Some("open-overlay"));

    for (key, expected) in [("tab", "overlay-action"), ("tab", "open-overlay"), ("shift+tab", "overlay-action")] {
        app.press_key(key).unwrap();
        assert_eq!(app.inspect().focus.as_deref(), Some(expected), "{key}");
    }
    assert!(app.tap("outside-overlay-action").unwrap_err().contains("behind the open modal `dialog`"));
    app.press_key("mod+s").unwrap();
    assert_eq!(app.kernel().text_of("saves").as_deref(), Some("1"));
    app.invoke("dialog.save").unwrap();
    assert_eq!(app.kernel().text_of("saves").as_deref(), Some("2"));

    app.press_key("escape").unwrap();
    assert!(!app.kernel().exists("overlay-action"));
    assert!(app.kernel().exists("dialog"));

    app.tap("open-overlay").unwrap();
    app.press_key("tab").unwrap();
    assert_eq!(app.inspect().focus.as_deref(), Some("overlay-action"));
    app.tap("dialog-action").unwrap();
    assert!(!app.kernel().exists("overlay-action"));
    assert!(app.kernel().exists("dialog"));
    assert_eq!(app.kernel().text_of("clicks").as_deref(), Some("1"));

    app.press_key("escape").unwrap();
    assert!(!app.kernel().exists("dialog"));
    assert!(app.app().diagnostics().is_empty(), "{:?}", app.app().diagnostics());
}

/// Apply changes to the last tree. Reachability through child lists determines what the adapter retains.
fn apply_accessibility_update(
    nodes: &mut HashMap<ak::NodeId, ak::Node>,
    root: ak::NodeId,
    update: ak::TreeUpdate,
) -> HashMap<ak::NodeId, Option<ak::Rect>> {
    assert!(update.tree.as_ref().is_none_or(|tree| tree.root == root));
    nodes.extend(update.nodes);
    let mut bounds = HashMap::new();
    let mut pending = vec![(root, ak::Affine::IDENTITY)];
    while let Some((id, parent_transform)) = pending.pop() {
        let node = nodes.get(&id).expect("a child must already exist or arrive in this update");
        let transform = parent_transform * node.transform().copied().unwrap_or_default();
        assert!(bounds.insert(id, node.bounds().map(|rect| transform.transform_rect_bbox(rect))).is_none(), "duplicate AX child {id:?}");
        pending.extend(node.children().iter().map(|&child| (child, transform)));
    }
    nodes.retain(|id, _| bounds.contains_key(id));
    assert!(nodes.contains_key(&update.focus), "the focus must be in the applied accessibility tree");
    for node in nodes.values() {
        for target in node.labelled_by().iter().chain(node.controls()).copied().chain(node.active_descendant()) {
            assert!(nodes.contains_key(&target), "the applied accessibility tree has a dangling relation to {target:?}");
        }
    }
    bounds
}

fn assert_accessibility_bounds(actual: ak::Rect, expected: [f32; 4]) {
    let actual = [actual.x0, actual.y0, actual.width(), actual.height()];
    for (actual, expected) in actual.into_iter().zip(expected) {
        assert!((actual - f64::from(expected)).abs() < 0.051, "AX bounds {actual} differ from Inspector bounds {expected}");
    }
}

#[test]
fn modal_scope_is_applied_to_inspector_and_accessibility() {
    let mut app = anchored_dialog_app();
    let ax_id = |app: &Window, name| a11y_id(app.kernel().find(name).unwrap());
    app.press_key("tab").unwrap();
    let before = app.inspect();
    let opener = before.find("open-dialog").unwrap();
    let opener_id = ax_id(&app, "open-dialog");
    let background =
        ["workspace", "open-dialog", "background-doc", "background-label", "outside-overlay-action"].map(|name| (name, ax_id(&app, name)));
    let opener_request = json!({"op":"tap", "target":opener.target}).to_string();
    assert_eq!(before.focus.as_deref(), Some("open-dialog"));
    assert!(before.commands.iter().any(|(command, _)| command == "workspace.save"));
    assert!(opener.actions.iter().any(|action| action == "press"));

    let first = app.accessibility_update();
    let root = first.tree.as_ref().unwrap().root;
    assert_eq!(first.focus, opener_id);
    let mut ax = HashMap::new();
    let before_bounds = apply_accessibility_update(&mut ax, root, first);
    assert!(ax[&opener_id].supports_action(ak::Action::Click));

    let reply = inspect::handle_line(&mut app, &opener_request);
    assert!(reply.ok, "{reply:?}");
    let modal = app.inspect();
    let dialog = modal.find("dialog").unwrap();
    let dialog_id = ax_id(&app, "dialog");
    let dialog_action_id = ax_id(&app, "dialog-action");
    let dialog_action_request = json!({"op":"tap", "target":modal.find("dialog-action").unwrap().target}).to_string();
    assert_eq!(modal.focus.as_deref(), Some("open-overlay"));
    assert!(modal.commands.iter().any(|(command, _)| command == "dialog.save"));
    assert!(!modal.commands.iter().any(|(command, _)| command == "workspace.save"));
    assert_eq!(dialog.name.as_deref(), Some("Save changes"));
    assert!(dialog.labelled_by.is_none() && dialog.controls.is_none() && dialog.active_descendant.is_none());

    let opened = app.accessibility_update();
    assert_eq!(opened.focus, ax_id(&app, "open-overlay"));
    let open_bounds = apply_accessibility_update(&mut ax, root, opened);
    assert!(ax[&dialog_id].is_modal());
    assert_eq!(ax[&dialog_id].label(), dialog.name.as_deref());
    assert!(
        ax[&dialog_id].labelled_by().is_empty() && ax[&dialog_id].controls().is_empty() && ax[&dialog_id].active_descendant().is_none()
    );
    assert!(before.find("workspace").unwrap().bounds[0] > 0.0);
    assert_accessibility_bounds(open_bounds[&dialog_id].unwrap(), dialog.bounds);
    for (name, id) in background {
        let original = before.find(name).unwrap();
        let covered = modal.find(name).unwrap();
        assert_eq!((&covered.target, &covered.name, covered.bounds), (&original.target, &original.name, original.bounds), "{name}");
        assert!(covered.actions.is_empty() && covered.commands.is_empty(), "{covered:?}");
        if let Some(carrier) = ax.get(&id) {
            assert_eq!(carrier.role(), Role::GenericContainer, "{name}");
            assert!(carrier.label().is_none() && carrier.value().is_none(), "{name}");
            for action in [ak::Action::Click, ak::Action::Focus, ak::Action::SetTextSelection] {
                assert!(!carrier.supports_action(action), "{name} supports {action:?}");
            }
        }
    }
    let covered = inspect::handle_line(&mut app, &opener_request);
    assert_eq!(covered.delivery, Some(Delivery::Refused), "{covered:?}");
    assert!(covered.message.unwrap().contains("behind the open modal `dialog`"));
    assert!(!app.event(InputEvent::Accessibility { target: opener_id, action: ak::Action::Click, data: None }));

    let overlay_opener_id = ax_id(&app, "open-overlay");
    assert!(ax[&overlay_opener_id].supports_action(ak::Action::Click));
    assert!(app.event(InputEvent::Accessibility { target: overlay_opener_id, action: ak::Action::Click, data: None }));
    app.press_key("tab").unwrap();
    let portal = app.inspect();
    let action = portal.find("overlay-action").unwrap();
    assert_eq!(portal.focus.as_deref(), Some("overlay-action"));
    assert!(action.actions.iter().any(|action| action == "press"));
    assert_eq!(action.labelled_by.as_deref(), Some("dialog-label"));
    assert_eq!(action.controls.as_deref(), Some("dialog-action"));
    let portal_id = ax_id(&app, "overlay-action");
    let expanded = app.accessibility_update();
    assert_eq!(expanded.focus, portal_id);
    let expanded_bounds = apply_accessibility_update(&mut ax, root, expanded);
    assert_accessibility_bounds(expanded_bounds[&portal_id].unwrap(), action.bounds);
    assert!(ax[&portal_id].supports_action(ak::Action::Click));
    assert_eq!(ax[&portal_id].labelled_by(), &[ax_id(&app, "dialog-label")]);
    assert_eq!(ax[&portal_id].controls(), &[dialog_action_id]);
    assert!(app.event(InputEvent::Accessibility { target: portal_id, action: ak::Action::Click, data: None }));

    app.invoke("dialog.save").unwrap();
    let relabelled = app.accessibility_update();
    apply_accessibility_update(&mut ax, root, relabelled);
    assert_eq!(ax[&dialog_id].label(), Some("Changes saved"));
    assert!(ax[&dialog_id].labelled_by().is_empty());

    app.press_key("escape").unwrap();
    app.press_key("escape").unwrap();
    let restored = app.inspect();
    assert_eq!(restored.focus.as_deref(), Some("open-dialog"));
    let restored_opener = restored.find("open-dialog").unwrap();
    assert_eq!(restored_opener.target, opener.target);
    assert_eq!(restored_opener.actions, opener.actions);
    assert!(restored.commands.iter().any(|(command, _)| command == "workspace.save"));
    let closed = app.accessibility_update();
    assert_eq!(closed.focus, opener_id);
    let closed_bounds = apply_accessibility_update(&mut ax, root, closed);
    assert_eq!(closed_bounds[&opener_id], before_bounds[&opener_id]);
    assert!(ax[&opener_id].supports_action(ak::Action::Click));
    assert!(!ax.contains_key(&dialog_id) && !ax.contains_key(&portal_id));
    let stale = inspect::handle_line(&mut app, &dialog_action_request);
    assert_eq!(stale.code.as_deref(), Some("STALE_TARGET"));
    assert_eq!(stale.delivery, Some(Delivery::Refused));
    assert!(!app.event(InputEvent::Accessibility { target: dialog_action_id, action: ak::Action::Click, data: None }));
    assert!(app.app().diagnostics().is_empty(), "{:?}", app.app().diagnostics());
}

#[test]
fn accessibility_selection_rejects_cached_background_runs_while_a_modal_is_open() {
    let mut app = anchored_dialog_app();
    let ax_id = |app: &Window, name| a11y_id(app.kernel().find(name).unwrap());
    let run_of = |app: &Window, text| {
        let text = ax_id(app, text);
        let tree = app.accessibility_tree();
        tree.nodes.iter().find(|(id, _)| *id == text).unwrap().1.children()[0]
    };
    let selection = |target, run| InputEvent::Accessibility {
        target,
        action: ak::Action::SetTextSelection,
        data: Some(ak::ActionData::SetTextSelection(ak::TextSelection {
            anchor: ak::TextPosition { node: run, character_index: 0 },
            focus: ak::TextPosition { node: run, character_index: 4 },
        })),
    };
    let background_doc = ax_id(&app, "background-doc");
    let background_run = run_of(&app, "background-label");
    assert!(app.event(selection(background_doc, background_run)));
    assert_eq!(app.selected_text().as_deref(), Some("Save"));
    app.clear_selection();

    app.tap("open-dialog").unwrap();
    let modal_doc = ax_id(&app, "dialog-doc");
    assert!(!app.event(selection(background_doc, background_run)));
    assert!(!app.event(selection(modal_doc, background_run)));
    assert!(app.selected_text().is_none());

    let modal_run = run_of(&app, "dialog-label");
    assert!(app.event(selection(modal_doc, modal_run)));
    assert_eq!(app.selected_text().as_deref(), Some("Dial"));
}

#[test]
fn a_press_outside_dismisses_when_allowed() {
    let mut app = dialog_app(true);
    app.tap("open").unwrap();
    app.click_at(5.0, 590.0);
    assert!(!app.kernel().exists("dialog"));
    assert_eq!(app.kernel().text_of("closes").unwrap(), "1");
    assert_eq!(focused(&app).as_deref(), Some("open"));
}

#[test]
fn focus_and_blur_are_heard() {
    let log: std::rc::Rc<std::cell::RefCell<Vec<String>>> = Default::default();
    let l = log.clone();
    let mut app = Window::headless(move || {
        let shown = state("shown", || true);
        let ev = |l: &std::rc::Rc<std::cell::RefCell<Vec<String>>>, what: &str| {
            let (l, what) = (l.clone(), what.to_owned());
            move || l.borrow_mut().push(what.clone())
        };
        col()
            .child(show(move || shown.get(), {
                let l = l.clone();
                move || {
                    input().id("name").on_focus(ev(&l, "focus name")).on_blur(ev(&l, "blur name")).on_key(move |k| {
                        if k.key == Key::Escape {
                            shown.set(false);
                        }
                        k.key == Key::Escape
                    })
                }
            }))
            .child(button("ok").on_focus(ev(&l, "focus ok")).on_blur(ev(&l, "blur ok")))
    });
    app.tap("name").unwrap();
    app.press_key("tab").unwrap();
    assert_eq!(*log.borrow(), ["focus name", "blur name", "focus ok"]);

    // The window going inactive keeps focus: nothing is blurred.
    log.borrow_mut().clear();
    app.event(InputEvent::Active(false));
    app.event(InputEvent::Active(true));
    assert!(log.borrow().is_empty(), "{:?}", log.borrow());

    // A focused element leaving the tree is not blurred.
    app.tap("name").unwrap();
    log.borrow_mut().clear();
    app.press_key("escape").unwrap();
    assert!(app.kernel().find("name").is_none());
    assert!(log.borrow().is_empty(), "{:?}", log.borrow());
}

#[test]
fn the_app_moves_focus_to_an_element_built_in_the_same_frame() {
    let mut app = Window::headless(|| {
        let editing = state("editing", || false);
        col().child(show(move || editing.get(), || input().id("name"))).child(button("open").on_press(move || {
            editing.set(true);
            focus("name");
        }))
    });
    app.tap("open").unwrap();
    assert_eq!(focused(&app).as_deref(), Some("name"));
}

#[test]
fn a_modal_keeps_its_original_opener_across_hot_reload() {
    let mut app = dialog_app(true);
    app.tap("open").unwrap();
    app.dispatch(Event::Focus("name".into())).unwrap();
    for _ in 0..2 {
        app.app().hot_reload();
        assert_eq!(app.inspect().focus.as_deref(), Some("name"));
    }
    app.press_key("escape").unwrap();
    assert_eq!(app.inspect().focus.as_deref(), Some("open"));
}

#[derive(Default)]
struct ReloadModalChanges {
    hide_opener: Cell<bool>,
    hide_inner_opener: Cell<bool>,
    remove_outer: Cell<bool>,
    remove_inner: Cell<bool>,
    remove_inner_field: Cell<bool>,
}

#[ui]
fn reload_modal_view(changes: Rc<ReloadModalChanges>) -> View {
    let outer_open = state("outer-open", || false);
    let inner_open = state("inner-open", || false);
    let (opener, outer) = (changes.clone(), changes.clone());
    col()
        .size_full()
        .child(show(move || !opener.hide_opener.get(), move || button("open").on_press(move || outer_open.set(true))))
        .child(show(
            move || outer_open.get() && !outer.remove_outer.get(),
            move || {
                let (opener, inner, field) = (changes.clone(), changes.clone(), changes.clone());
                col()
                    .id("outer-dialog")
                    .absolute()
                    .left(200.0)
                    .top(100.0)
                    .w(240.0)
                    .modal(true, move || outer_open.set(false))
                    .child(input().id("outer-name"))
                    .child(show(
                        move || !opener.hide_inner_opener.get(),
                        move || button("open-inner").on_press(move || inner_open.set(true)),
                    ))
                    // The overlay's stable anchor is separate from its opener, so a reload
                    // can remove that opener while leaving the inner modal open.
                    .child(el().size_8().overlay(
                        move || inner_open.get() && !inner.remove_inner.get(),
                        Placement::BELOW,
                        move || {
                            let field = field.clone();
                            col()
                                .id("inner-dialog")
                                .w(180.0)
                                .modal(true, move || inner_open.set(false))
                                .child(show(move || !field.remove_inner_field.get(), || input().id("inner-name")))
                                .child(button("inner-fallback"))
                        },
                    ))
            },
        ))
        .into()
}

fn nested_reload_modal_app() -> (Window, Rc<ReloadModalChanges>) {
    let changes = Rc::new(ReloadModalChanges::default());
    let view_changes = changes.clone();
    let mut app = Window::headless(move || reload_modal_view(view_changes.clone()));
    app.tap("open").unwrap();
    app.tap("open-inner").unwrap();
    assert_eq!(app.inspect().focus.as_deref(), Some("inner-name"));
    (app, changes)
}

#[test]
fn nested_overlay_modals_keep_both_openers_across_hot_reload() {
    let (mut app, _) = nested_reload_modal_app();
    for _ in 0..2 {
        app.app().hot_reload();
        assert_eq!(app.inspect().focus.as_deref(), Some("inner-name"));
    }
    app.press_key("escape").unwrap();
    assert_eq!(app.inspect().focus.as_deref(), Some("open-inner"));
    app.press_key("escape").unwrap();
    assert_eq!(app.inspect().focus.as_deref(), Some("open"));
}

#[test]
fn a_missing_original_opener_is_not_replaced_by_the_reloaded_field() {
    let (mut app, changes) = nested_reload_modal_app();
    changes.hide_opener.set(true);
    app.app().hot_reload();
    app.press_key("escape").unwrap();
    app.press_key("escape").unwrap();
    assert!(app.inspect().focus.is_none());
    assert!(app.app().diagnostics().is_empty(), "{:?}", app.app().diagnostics());
}

#[test]
fn a_missing_inner_opener_restores_focus_inside_the_outer_modal() {
    let (mut app, changes) = nested_reload_modal_app();
    changes.hide_inner_opener.set(true);
    app.app().hot_reload();
    app.press_key("escape").unwrap();
    assert_eq!(app.inspect().focus.as_deref(), Some("outer-name"));
    app.press_key("escape").unwrap();
    assert_eq!(app.inspect().focus.as_deref(), Some("open"));
}

#[test]
fn a_modal_removed_by_hot_reload_restores_its_opener() {
    for outer in [false, true] {
        let (app, changes) = nested_reload_modal_app();
        changes.remove_outer.set(outer);
        changes.remove_inner.set(!outer);
        app.app().hot_reload();
        let expected = if outer { "open" } else { "open-inner" };
        assert_eq!(app.inspect().focus.as_deref(), Some(expected));
        assert!(!app.kernel().exists("inner-dialog"));
        assert_eq!(app.kernel().exists("outer-dialog"), !outer);
        assert!(app.app().diagnostics().is_empty(), "{:?}", app.app().diagnostics());
    }
}

#[test]
fn a_field_removed_by_hot_reload_keeps_focus_in_its_surviving_modal() {
    let (mut app, changes) = nested_reload_modal_app();
    changes.remove_inner_field.set(true);
    app.app().hot_reload();
    assert_eq!(app.inspect().focus.as_deref(), Some("inner-fallback"));
    app.press_key("escape").unwrap();
    assert_eq!(app.inspect().focus.as_deref(), Some("open-inner"));
    app.press_key("escape").unwrap();
    assert_eq!(app.inspect().focus.as_deref(), Some("open"));
}
