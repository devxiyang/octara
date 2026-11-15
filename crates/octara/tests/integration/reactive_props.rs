//! Writable and derived read handles use ordinary property bindings.

use octara::prelude::*;

#[test]
fn signals_bind_text_and_semantics_without_reading_during_build() {
    let mut window = Window::headless(|| {
        let name = state("name", || "First".to_owned());
        let checked = state("checked", || false);
        let label: ReadSignal<String> = memo(move || format!("Chosen: {}", name.get()));
        col()
            .size_full()
            .child(text(name).id("name"))
            .child(text(label).id("derived"))
            .child(el().id("option").role(Role::CheckBox).label(label).checked(checked.read_only()).size_8())
            .child(el().id("change").role(Role::Button).label("Change").size_8().on_press(move || {
                name.set("Second".into());
                checked.set(true);
            }))
            .into()
    });
    assert_eq!(window.kernel().text_of("name").as_deref(), Some("First"));
    assert_eq!(window.kernel().text_of("derived").as_deref(), Some("Chosen: First"));
    assert_eq!(window.kernel().checked_of("option"), Some(false));

    window.tap("change").unwrap();

    assert_eq!(window.kernel().text_of("name").as_deref(), Some("Second"));
    assert_eq!(window.kernel().text_of("derived").as_deref(), Some("Chosen: Second"));
    assert_eq!(window.kernel().checked_of("option"), Some(true));
    assert!(window.layout_issues().is_empty(), "{:?}", window.layout_issues());
    assert!(window.app().diagnostics().is_empty(), "{:?}", window.app().diagnostics());
}
