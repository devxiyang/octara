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
    let mut app = Window::headless(view);
    let tree = app.accessibility_tree();
    let node = |id: &str| {
        let n = app.kernel().find(id).unwrap();
        tree.nodes.iter().find(|(i, _)| *i == a11y_id(n)).map(|(_, n)| n.clone()).unwrap()
    };
    let root = &tree.nodes.iter().find(|(i, _)| *i == tree.tree.as_ref().unwrap().root).unwrap().1;
    assert_eq!(root.role(), AkRole::Window);
    assert!(root.transform().is_some(), "the window scales logical bounds to device pixels");
    assert_eq!((node("count").role(), node("count").value()), (AkRole::Label, Some("Count 0")), "static text is a label's value");
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
    assert!(app.event(InputEvent::Accessibility { target: wifi, action: Action::Click, data: None }));
    assert_eq!(app.kernel().checked_of("wifi"), Some(true));
    let inc_id = a11y_id(app.kernel().find("inc").unwrap());
    assert!(app.event(InputEvent::Accessibility { target: inc_id, action: Action::Focus, data: None }));
    assert_eq!(app.accessibility_tree().focus, inc_id);
}

#[test]
fn a_value_is_announced_for_any_control() {
    // A combo box's choice is its value, as a text input's content is.
    let app = Window::headless(|| {
        col()
            .child(el().id("theme").role(Role::ComboBox).label("Theme").value("Dark").focusable().on_press(|| {}))
            .child(text_input().id("name").label("Name").value("Ada"))
            .into()
    });
    let snap = app.inspect();
    assert_eq!(snap.find("theme").unwrap().value.as_deref(), Some("Dark"));
    assert_eq!(snap.find("name").unwrap().value.as_deref(), Some("Ada"));
    let tree = app.accessibility_tree();
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
    let items = state_list("items", || (0..20u32).collect::<Vec<_>>(), |i| *i);
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
    let mut app = Window::headless(busy);
    let full = app.accessibility_update();
    assert!(full.tree.is_some() && full.nodes.len() > 10, "the first update is the whole tree");
    let quiet = app.accessibility_update();
    assert!(quiet.nodes.is_empty(), "nothing changed: nothing sent");

    // Every update below is checked against the whole tree in debug builds.
    let sizes = |app: &mut Window, what: &str, op: &dyn Fn(&mut Window)| {
        op(app);
        let u = app.accessibility_update();
        assert!(u.tree.is_none(), "{what}: incremental");
        u.nodes.len()
    };
    let n = sizes(&mut app, "press", &|a| a.tap("inc").unwrap());
    assert!(n <= 3, "a press sends the text and the button: {n}");
    app.app().advance(std::time::Duration::from_secs(1));
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
    sizes(&mut app, "theme", &|a| a.app().set_theme(Theme::dark()));

    // Restarting sends the whole tree again.
    app.restart_accessibility();
    assert!(app.accessibility_update().tree.is_some());
}

// ---- Text ([text core](../../../../docs/design/text.md) step 3) ----

fn article() -> View {
    col()
        .id("doc")
        .selectable()
        .w(160.0)
        .child(text("Title").id("h").role(Role::Heading).level(1))
        .child(text("A paragraph long enough to wrap over a few lines here.").id("p"))
        .child(row().select_none().child(text("•").id("marker")))
        .child(text("Last words\nafter a break").id("q"))
        .child(text("news").id("status").live(Live::Polite))
        .into()
}

fn nodes(u: &octara::accesskit::TreeUpdate) -> std::collections::HashMap<octara::accesskit::NodeId, octara::accesskit::Node> {
    u.nodes.iter().cloned().collect()
}

/// A text's runs, checked: their values make up its text, their characters their values, and
/// every per-character list is as long as the characters.
fn runs_of(
    app: &Window,
    tree: &std::collections::HashMap<octara::accesskit::NodeId, octara::accesskit::Node>,
    id: &str,
) -> Vec<octara::accesskit::NodeId> {
    let n = app.kernel().find(id).unwrap();
    let node = &tree[&a11y_id(n)];
    let runs: Vec<_> = node.children().iter().copied().filter(|c| tree[c].role() == AkRole::TextRun).collect();
    let mut joined = String::new();
    for r in &runs {
        let run = &tree[r];
        let value = run.value().unwrap();
        let lengths = run.character_lengths();
        assert_eq!(lengths.iter().map(|l| usize::from(*l)).sum::<usize>(), value.len(), "{id}: {value:?}");
        assert_eq!(run.character_positions().map(<[f32]>::len), Some(lengths.len()));
        assert_eq!(run.character_widths().map(<[f32]>::len), Some(lengths.len()));
        assert!(run.word_starts().windows(2).all(|w| w[0] < w[1]) && run.word_starts().iter().all(|w| usize::from(*w) < lengths.len()));
        assert_eq!(app.kernel().node_of_a11y(*r), Some(n), "a run names its text");
        joined.push_str(value);
    }
    assert_eq!(joined, app.kernel().text(n), "{id}: the runs make up the text");
    runs
}

#[test]
fn a_selectable_region_is_a_document_of_runs_with_its_selection() {
    let mut app = Window::headless(article);
    app.select(("p", 2), ("q", 4)).unwrap();
    let tree = nodes(&app.accessibility_tree());
    let doc = &tree[&a11y_id(app.kernel().find("doc").unwrap())];
    assert_eq!(doc.role(), AkRole::Document);
    let h = &tree[&a11y_id(app.kernel().find("h").unwrap())];
    assert_eq!((h.role(), h.level(), h.label()), (AkRole::Heading, Some(1), Some("Title")));
    assert_eq!(tree[&a11y_id(app.kernel().find("status").unwrap())].live(), Some(Live::Polite));
    runs_of(&app, &tree, "h");
    let p = runs_of(&app, &tree, "p");
    assert!(p.len() >= 3, "a run per line: {}", p.len());
    let q = runs_of(&app, &tree, "q");
    assert_eq!(tree[&q[0]].value(), Some("Last words\n"), "a hard break ends its line's run");
    assert!(tree[&q[0]].word_starts().contains(&5), "\"words\" starts a word");
    // The marker is not selectable: no runs.
    let marker = &tree[&a11y_id(app.kernel().find("marker").unwrap())];
    assert!(marker.children().is_empty() && marker.value() == Some("•"));

    // The selection's ends are runs: offset 2 of the paragraph, offset 4 of the last text.
    let sel = doc.text_selection().unwrap();
    assert_eq!((sel.anchor.node, sel.anchor.character_index), (p[0], 2));
    assert_eq!((sel.focus.node, sel.focus.character_index), (q[0], 4));
    // Backwards, anchor and focus swap.
    app.select(("q", 4), ("p", 2)).unwrap();
    let tree = nodes(&app.accessibility_tree());
    let sel = tree[&a11y_id(app.kernel().find("doc").unwrap())].text_selection().unwrap();
    assert_eq!((sel.anchor.node, sel.focus.node), (q[0], p[0]));
    // Assistive technology selects: from the start of the paragraph's second run to the
    // heading's end, backwards.
    let doc_id = a11y_id(app.kernel().find("doc").unwrap());
    let h = runs_of(&app, &nodes(&app.accessibility_tree()), "h");
    let at = |node, character_index| octara::accesskit::TextPosition { node, character_index };
    let ask = octara::accesskit::TextSelection { anchor: at(p[1], 0), focus: at(h[0], 5) };
    assert!(app.event(InputEvent::Accessibility {
        target: doc_id,
        action: Action::SetTextSelection,
        data: Some(octara::accesskit::ActionData::SetTextSelection(ask)),
    }));
    // The heading's part is empty (its end): the paragraph's first line is what is selected.
    assert_eq!(app.selected_text().as_deref(), tree[&p[0]].value());
    let tree = nodes(&app.accessibility_tree());
    assert_eq!(tree[&doc_id].text_selection(), Some(&ask), "the selection reads back as asked");
    // Cleared, none.
    app.clear_selection();
    let tree = nodes(&app.accessibility_tree());
    assert!(tree[&a11y_id(app.kernel().find("doc").unwrap())].text_selection().is_none());
}

#[test]
fn runs_and_the_selection_follow_changes() {
    let app = App::new(octara::platform::HeadlessPlatform::default());
    let live = app.run(|| TextBuffer::new("streaming"));
    let stream = live.clone();
    let mut w = app
        .open_window(octara::platform::WindowOptions::new("Chat"), move || {
            col().id("chat").selectable().w(200.0).child(text("first").id("a")).child(text(&stream).id("live")).into()
        })
        .unwrap();
    // Every update below is checked against the whole tree in debug builds.
    w.accessibility_update();
    w.select(("a", 1), ("live", 3)).unwrap();
    assert!(!w.accessibility_update().nodes.is_empty());
    app.run(|| live.append(" words that wrap the text over more lines than one"));
    app.settle();
    let u = w.accessibility_update();
    let tree = nodes(&w.accessibility_tree());
    let runs = runs_of(&w, &tree, "live");
    assert!(runs.len() > 1, "the stream wrapped");
    assert!(u.nodes.iter().any(|(id, _)| *id == runs[1]), "its new line is sent");
    w.resize(500.0, 400.0);
    w.accessibility_update();
    w.clear_selection();
    w.accessibility_update();
}

#[test]
fn a_selection_through_unmounted_rows_ends_at_the_mounted_runs() {
    let mut w = Window::headless(|| {
        let items = state_list("rows", || (0..1000u32).map(|i| (i, format!("Message {i}"))).collect::<Vec<_>>(), |m: &(u32, String)| m.0);
        col()
            .size_full()
            .child(col().id("list").selectable().flex_1().overflow_y_scroll().child(virtual_each(
                move || items.source(),
                |_| 24.0,
                |k, m| text(move || m.with(|m| m.1.clone())).id(format!("m{k}")).into(),
            )))
            .into()
    });
    w.select(("m3", 2), ("m20", 4)).unwrap();
    // Scroll row 3 out; row 20 stays.
    w.dispatch(Event::ScrollTo("list".into(), 0.0, 300.0)).unwrap();
    assert!(w.kernel().find("m3").is_none() && w.kernel().find("m20").is_some());
    let mounted: Vec<String> = (0..1000).map(|i| format!("m{i}")).filter(|id| w.kernel().find(id).is_some()).collect();
    let last = "m20".to_string();
    let tree = nodes(&w.accessibility_tree());
    let sel = tree[&a11y_id(w.kernel().find("list").unwrap())].text_selection().unwrap();
    let first = runs_of(&w, &tree, &mounted[0]);
    let end = runs_of(&w, &tree, &last);
    assert_eq!((sel.anchor.node, sel.anchor.character_index), (first[0], 0), "the anchor's row is not mounted: the mounted part begins");
    assert_eq!((sel.focus.node, sel.focus.character_index), (end[0], 4));
}
