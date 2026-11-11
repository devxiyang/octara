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
    let mut app = App::new(|| {
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
