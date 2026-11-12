//! The accessibility tree from the kernel's semantics, and actions from assistive technology.

use octara::accesskit::{Action, Role as AkRole, Toggled};
use octara::kernel::a11y::a11y_id;
use octara::prelude::*;

fn view() -> View {
    let on = state("on", || false);
    let count = state("count", || 0);
    col()
        .child(text(move || format!("Count {}", count.get())).id("count"))
        .child(el().id("inc").size_8().role(Role::Button).label("Increment").focusable().on_press(move || count.update(|n| *n += 1)))
        .child(
            el().id("wifi")
                .role(Role::Switch)
                .label("Wi-Fi")
                .focusable()
                .checked(move || on.get())
                .on_toggle(move || on.update(|v| *v = !*v)),
        )
        .child(text_input().id("name").label("Name").value("Ada"))
        .child(spacer())
        .into()
}

#[test]
fn roles_names_states_actions_and_bounds() {
    let mut app = App::new(view);
    let tree = app.accessibility_tree(2.0);
    let node = |id: &str| {
        let n = app.kernel().find(id).unwrap();
        tree.nodes.iter().find(|(i, _)| *i == a11y_id(n)).map(|(_, n)| n.clone()).unwrap()
    };
    let root = &tree.nodes.iter().find(|(i, _)| *i == tree.tree.as_ref().unwrap().root).unwrap().1;
    assert_eq!(root.role(), AkRole::Window);
    assert!(root.transform().is_some(), "the window scales logical bounds to device pixels");
    assert_eq!(node("count").label(), Some("Count 0"));
    let inc = node("inc");
    assert_eq!((inc.role(), inc.label()), (AkRole::Button, Some("Increment")));
    assert!(inc.supports_action(Action::Click) && inc.supports_action(Action::Focus));
    assert_eq!(node("wifi").toggled(), Some(Toggled::False));
    assert_eq!(node("name").value(), Some("Ada"));
    let b = inc.bounds().unwrap();
    assert!(b.x1 > b.x0 && b.y1 > b.y0);
    // Decorative parts (spacers) are left out.
    assert_eq!(tree.nodes.len(), 1 + 1 + 4, "window, column, four elements");

    // Actions from assistive technology.
    let wifi = a11y_id(app.kernel().find("wifi").unwrap());
    assert!(app.accessibility_action(wifi, Action::Click, None));
    assert_eq!(app.kernel().checked_of("wifi"), Some(true));
    let inc_id = a11y_id(app.kernel().find("inc").unwrap());
    assert!(app.accessibility_action(inc_id, Action::Focus, None));
    assert_eq!(app.accessibility_tree(1.0).focus, inc_id);
}

#[test]
fn a_value_is_announced_for_any_control() {
    // A combo box's choice is its value, as a text input's content is.
    let app = App::new(|| {
        col()
            .child(el().id("theme").role(Role::ComboBox).label("Theme").value("Dark").focusable().on_press(|| {}))
            .child(text_input().id("name").label("Name").value("Ada"))
            .into()
    });
    let snap = app.inspect();
    assert_eq!(snap.find("theme").unwrap().value.as_deref(), Some("Dark"));
    assert_eq!(snap.find("name").unwrap().value.as_deref(), Some("Ada"));
    let tree = app.accessibility_tree(1.0);
    let theme = app.kernel().find("theme").unwrap();
    let node = tree.nodes.iter().find(|(i, _)| *i == a11y_id(theme)).unwrap();
    assert_eq!(node.1.value(), Some("Dark"));
}

/// A view where every kind of change happens: text, labels, states, structure, order, scroll,
/// overlays, motion.
fn busy() -> View {
    let count = state("count", || 0);
    let open = state("open", || false);
    let menu = state("menu", || false);
    let items = list("items", || (0..20u32).collect::<Vec<_>>(), |i| *i);
    let next = state("next", || 20u32);
    col()
        .size_full()
        .child(text(move || format!("Count {}", count.get())).id("count"))
        .child(
            el().id("inc")
                .size_8()
                .role(Role::Button)
                .label(move || format!("Add one to {}", count.get()))
                .on_press(move || count.update(|n| *n += 1))
                .bg(move || if count.get() % 2 == 0 { Token::Surface } else { Token::Accent })
                .motion(Spring::SNAP),
        )
        .child(el().id("toggle").size_8().role(Role::Button).label("Toggle").on_press(move || open.update(|o| *o = !*o)))
        .child(show(
            move || open.get(),
            || col().id("panel").child(text("Inside")).child(el().id("deep").role(Role::Button).label("Deep").on_press(|| {})).into(),
        ))
        .child(el().id("add").size_8().role(Role::Button).label("Add").on_press(move || {
            let n = next.get();
            next.set(n + 1);
            items.insert(0, n);
        }))
        .child(el().id("drop").size_8().role(Role::Button).label("Drop").on_press(move || {
            if let Some(k) = items.last_key() {
                items.remove(&k);
            }
        }))
        .child(el().id("menu-button").size_8().role(Role::Button).label("Menu").on_press(move || menu.update(|m| *m = !*m)).overlay(
            move || menu.get(),
            Placement::BELOW,
            || text("A menu item").id("menu-item").into(),
        ))
        .child(
            col().id("list").h(100).overflow_y_scroll().child(each(move || items.source(), |i, _| text(format!("Item {i}")).h(30).into())),
        )
        .into()
}

#[test]
fn updates_send_only_what_changed() {
    let mut app = App::new(busy);
    let full = app.accessibility_update(2.0);
    assert!(full.tree.is_some() && full.nodes.len() > 10, "the first update is the whole tree");
    let quiet = app.accessibility_update(2.0);
    assert!(quiet.nodes.is_empty(), "nothing changed: nothing sent");

    // Every update below is checked against the whole tree in debug builds.
    let sizes = |app: &mut App, what: &str, op: &dyn Fn(&mut App)| {
        op(app);
        let u = app.accessibility_update(2.0);
        assert!(u.tree.is_none(), "{what}: incremental");
        u.nodes.len()
    };
    let n = sizes(&mut app, "press", &|a| a.tap("inc").unwrap());
    assert!(n <= 3, "a press sends the text and the button: {n}");
    app.advance(std::time::Duration::from_secs(1));
    sizes(&mut app, "motion settles", &|_| {});
    let shown = sizes(&mut app, "show", &|a| a.tap("toggle").unwrap());
    assert!(shown >= 4, "the panel's subtree is new: {shown}");
    sizes(&mut app, "hide", &|a| a.tap("toggle").unwrap());
    sizes(&mut app, "show again", &|a| a.tap("toggle").unwrap());
    sizes(&mut app, "insert", &|a| a.tap("add").unwrap());
    sizes(&mut app, "remove", &|a| a.tap("drop").unwrap());
    let list = app.kernel().find("list").unwrap();
    let scrolled = sizes(&mut app, "scroll", &|a| {
        assert!(a.kernel_mut().scroll_to(list, 0.0, 60.0));
    });
    assert_eq!(scrolled, 1, "a scroll moves only the container's space");
    sizes(&mut app, "overlay", &|a| a.tap("menu-button").unwrap());
    sizes(&mut app, "overlay closes", &|a| a.tap("menu-button").unwrap());
    sizes(&mut app, "resize", &|a| a.resize(500.0, 400.0));
    sizes(&mut app, "theme", &|a| a.set_theme(Theme::dark()));

    // Restarting sends the whole tree again.
    app.restart_accessibility();
    assert!(app.accessibility_update(2.0).tree.is_some());
}
