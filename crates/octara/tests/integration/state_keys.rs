//! A state key used twice in one component is reported, and both calls keep working: the same
//! type shares one signal (as the diagnostic says); another type gets a state of its own instead
//! of destroying the first.

use octara::prelude::*;

#[test]
fn a_repeated_key_of_another_type_does_not_destroy_the_first() {
    let mut app = Window::headless(|| {
        let items = list("notes", || vec![(1u32, "first".to_string())], |n: &(u32, String)| n.0);
        let draft = state("notes", || "draft".to_string());
        col()
            .child(text(move || format!("{} items", items.len())).id("count"))
            .child(text(move || draft.get()).id("draft"))
            .child(el().id("add").role(Role::Button).label("Add").size_8().on_press(move || {
                items.push((2, "second".into()));
                draft.set("edited".into());
            }))
            .into()
    });
    app.tap("add").unwrap();
    assert_eq!(app.kernel().text_of("count").unwrap(), "2 items");
    assert_eq!(app.kernel().text_of("draft").unwrap(), "edited");
    assert!(app.app().diagnostics().iter().any(|d| d.code.id == "DUPLICATE_STATE_KEY"), "{:?}", app.app().diagnostics());
}
