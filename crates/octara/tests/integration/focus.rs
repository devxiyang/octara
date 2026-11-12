//! Focus: Tab order with `tab_index`, focus groups and arrow keys, and modals that trap focus,
//! make the window inert, and give focus back when they close.

use octara::prelude::*;

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
            .into()
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
    let mut app =
        Window::headless(|| {
            let tab = state("tab", || 2);
            col()
                .child(button("a"))
                .child(row().role(Role::TabList).focus_group().children((1..=3).map(move |i| {
                    button(&format!("tab-{i}")).role(Role::Tab).selected(move || tab.get() == i).on_press(move || tab.set(i))
                })))
                .child(col().role(Role::List).focus_group().children((1..=3).map(|i| button(&format!("row-{i}")).role(Role::ListItem))))
                .child(button("c"))
                .into()
        });
    app.press_key("tab").unwrap();
    app.press_key("tab").unwrap();
    assert_eq!(focused(&app).as_deref(), Some("tab-2"), "Tab enters a group at its selected member");
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
                        .child(text_input().id("name").value("").on_input(|_| {}))
                        .child(button("ok").on_press(move || open.set(false)))
                        .child(button("cancel").autofocus().on_press(move || open.set(false)))
                        .into()
                },
            ))
            .into()
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
    let tree = app.accessibility_tree(1.0);
    let id = octara::kernel::a11y::a11y_id(app.kernel().find("dialog").unwrap());
    assert!(tree.nodes.iter().find(|(n, _)| *n == id).unwrap().1.is_modal());
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
