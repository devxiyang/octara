//! Ordinary functions compose UI; explicitly keyed scopes own state and lifetime.

use octara::prelude::*;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

fn counter(id: &'static str, value: Signal<u32>) -> El {
    el().id(id)
        .role(Role::Button)
        .label(id)
        .size_12()
        .child(text(move || value.get().to_string()).id(format!("{id}-value")))
        .on_press(move || value.update(|value| *value += 1))
}

#[ui]
fn owned_counter(key: &'static str, id: &'static str, initial: u32) -> View {
    scope(key, move || {
        let value = state("value", || initial);
        counter(id, value)
    })
}

fn clean(window: &Window) {
    assert!(window.app().diagnostics().is_empty(), "{:?}", window.app().diagnostics());
    assert!(window.layout_issues().is_empty(), "{:?}", window.layout_issues());
}

#[test]
fn ordinary_functions_reuse_caller_owned_state_without_an_annotation_or_scope() {
    let mut window = Window::headless(|| {
        let left = state("left", || 0u32);
        let right = state("right", || 10u32);
        col().size_full().child(counter("left", left)).child(counter("right", right)).into()
    });
    let left = window.inspect().find("left").unwrap().target.clone();
    window.tap("left").unwrap();
    window.tap("right").unwrap();
    window.tap("right").unwrap();
    assert_eq!(window.kernel().text_of("left-value").as_deref(), Some("1"));
    assert_eq!(window.kernel().text_of("right-value").as_deref(), Some("12"));
    assert_eq!(window.inspect().find("left").unwrap().target, left);
    assert_eq!(window.inspect().find("left").unwrap().component, None);
    clean(&window);
}

#[test]
fn scope_accepts_elements_and_borrowed_or_owned_strings_without_a_wrapper_node() {
    let window = Window::headless(|| {
        col()
            .id("content")
            .size_full()
            .child(scope(7, || text("Element").id("element")))
            .child(scope("borrowed", || "Borrowed"))
            .child(scope(String::from("owned"), || String::from("Owned")))
            .into()
    });
    let snapshot = window.inspect();
    let content = snapshot.find("content").unwrap();
    let names: Vec<_> = content.children.iter().map(|node| node.name.as_deref()).collect();
    assert_eq!(names, [Some("Element"), Some("Borrowed"), Some("Owned")]);
    assert_eq!(content.children[0].id.as_deref(), Some("element"));
    clean(&window);
}

#[test]
fn duplicate_scope_keys_skip_the_body_and_leave_later_siblings_usable() {
    let bodies = Rc::new(Cell::new(0));
    let cleaned = Rc::new(RefCell::new(Vec::new()));
    let (out, cleanup) = (bodies.clone(), cleaned.clone());
    let mut window = Window::headless(move || {
        let (first, duplicate) = (out.clone(), out.clone());
        let (first_cleanup, duplicate_cleanup) = (cleanup.clone(), cleanup.clone());
        col()
            .size_full()
            .child(scope("repeated", move || {
                first.set(first.get() + 1);
                let value = state("value", || 10u32);
                on_cleanup(move || first_cleanup.borrow_mut().push("first"));
                counter("first", value)
            }))
            .child(scope("repeated", move || {
                duplicate.set(duplicate.get() + 1);
                let value = state("value", || 99u32);
                on_cleanup(move || duplicate_cleanup.borrow_mut().push("duplicate"));
                counter("duplicate", value)
            }))
            .child(owned_counter("later", "later", 0))
            .into()
    });
    assert_eq!(bodies.get(), 1, "duplicate content never runs");
    assert!(!window.kernel().exists("duplicate"));
    window.tap("first").unwrap();
    window.tap("later").unwrap();
    assert_eq!(window.kernel().text_of("first-value").as_deref(), Some("11"));
    assert_eq!(window.kernel().text_of("later-value").as_deref(), Some("1"));
    let diagnostics = window.app().diagnostics();
    assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
    assert_eq!(diagnostics[0].code.id, "DUPLICATE_SCOPE_KEY");
    assert!(window.layout_issues().is_empty(), "{:?}", window.layout_issues());
    window.app().shutdown();
    assert_eq!(&*cleaned.borrow(), &["first"], "the refused scope did not register cleanup");
}

#[test]
fn scoped_providers_handlers_and_cleanup_keep_their_owner() {
    let seen = Rc::new(RefCell::new(Vec::new()));
    let cleaned = Rc::new(RefCell::new(Vec::new()));
    let (out, cleanup) = (seen.clone(), cleaned.clone());
    let mut window = Window::headless(move || {
        provide(1u32);
        let visible = state("visible", || true);
        let (root, scoped, cleanup) = (out.clone(), out.clone(), cleanup.clone());
        col()
            .size_full()
            .child(el().id("root").size_12().on_press(move || root.borrow_mut().push(use_context::<u32>().unwrap())))
            .child(show(
                move || visible.get(),
                move || {
                    let (out, cleanup) = (scoped.clone(), cleanup.clone());
                    scope("provider", move || {
                        assert_eq!(use_context::<u32>(), Some(1));
                        provide(2u32);
                        let (parent, parent_cleanup) = (out.clone(), cleanup.clone());
                        on_cleanup(move || parent_cleanup.borrow_mut().push(use_context::<u32>().unwrap()));
                        col()
                            .child(el().id("parent").size_12().on_press(move || parent.borrow_mut().push(use_context::<u32>().unwrap())))
                            .child(scope("child", move || {
                                assert_eq!(use_context::<u32>(), Some(2));
                                provide(3u32);
                                on_cleanup(move || cleanup.borrow_mut().push(use_context::<u32>().unwrap()));
                                el().id("child").size_12().on_press(move || out.borrow_mut().push(use_context::<u32>().unwrap()))
                            }))
                    })
                },
            ))
            .child(el().id("hide").size_12().on_press(move || visible.set(false)))
            .into()
    });
    for id in ["child", "parent", "root"] {
        window.tap(id).unwrap();
    }
    assert_eq!(&*seen.borrow(), &[3, 2, 1]);
    window.tap("hide").unwrap();
    assert!(!window.kernel().exists("parent"));
    assert!(!window.kernel().exists("child"));
    assert_eq!(&*cleaned.borrow(), &[3, 2], "children clean up before their provider");
    window.tap("root").unwrap();
    assert_eq!(&*seen.borrow(), &[3, 2, 1, 1]);
    clean(&window);
    window.app().shutdown();
    assert_eq!(&*cleaned.borrow(), &[3, 2], "retired scopes clean up once");
}

#[test]
fn explicit_keys_keep_state_across_reordered_reload_and_ui_keeps_deferred_provenance() {
    let reversed = Rc::new(Cell::new(false));
    let order = reversed.clone();
    let mut window = Window::headless(move || {
        let ids = if order.get() { ["right", "left"] } else { ["left", "right"] };
        col().id("counters").size_full().children(ids.map(|id| owned_counter(id, id, 0))).into()
    });
    window.tap("left").unwrap();
    window.tap("right").unwrap();
    window.tap("right").unwrap();
    let before = window.inspect().find("left").unwrap().target.clone();
    reversed.set(true);
    window.app().hot_reload();
    let snapshot = window.inspect();
    let ids: Vec<_> = snapshot.find("counters").unwrap().children.iter().map(|node| node.id.as_deref()).collect();
    assert_eq!(ids, [Some("right"), Some("left")]);
    assert_eq!(window.kernel().text_of("left-value").as_deref(), Some("1"));
    assert_eq!(window.kernel().text_of("right-value").as_deref(), Some("2"));
    let left = snapshot.find("left").unwrap();
    assert_ne!(left.target, before, "reload mounts a fresh tree while keeping keyed state");
    let made = left.component.as_ref().expect("deferred content retains its #[ui] provenance");
    assert_eq!((made.name.as_str(), made.module.as_str()), ("owned_counter", "integration::scopes"));
    assert!(made.called_at.contains("scopes.rs:"), "{}", made.called_at);
    clean(&window);
}

#[test]
fn scope_keys_with_path_syntax_and_percent_escapes_have_distinct_state() {
    let mut window = Window::headless(|| {
        col()
            .size_full()
            .child(scope("a", || owned_counter("b", "nested", 10)))
            .child(owned_counter("a]/scope[b", "raw", 20))
            .child(owned_counter("a/b", "slash", 30))
            .child(owned_counter("a%2Fb", "percent", 40))
            .into()
    });
    for (id, expected) in [("nested", "10"), ("raw", "20"), ("slash", "30"), ("percent", "40")] {
        assert_eq!(window.kernel().text_of(&format!("{id}-value")).as_deref(), Some(expected));
    }
    window.tap("raw").unwrap();
    window.tap("slash").unwrap();
    window.app().hot_reload();
    for (id, expected) in [("nested", "10"), ("raw", "21"), ("slash", "31"), ("percent", "40")] {
        assert_eq!(window.kernel().text_of(&format!("{id}-value")).as_deref(), Some(expected));
    }
    clean(&window);
}
