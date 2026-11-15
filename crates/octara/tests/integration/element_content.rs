//! Content-specific builder calls fail before mounting incompatible payloads or bindings.

use octara::prelude::*;
use std::panic::{AssertUnwindSafe, catch_unwind};

fn rejected(method: &str, build: impl FnOnce() -> El) {
    let error = catch_unwind(AssertUnwindSafe(build)).err().expect("incompatible content was accepted");
    let message = error.downcast_ref::<String>().map(String::as_str).or_else(|| error.downcast_ref::<&str>().copied()).unwrap();
    assert!(message.contains("ELEMENT_CONTENT") && message.contains(method), "{message}");
}

#[test]
fn images_cannot_replace_another_content_kind() {
    let image = Image::rgba(1, 1, vec![255; 4]);
    rejected("img", || text(|| -> String { panic!("a rejected text binding must not run") }).img(&image));
    rejected("img", || canvas(|_| panic!("a rejected canvas must not run")).img(&image));
    rejected("img", || text_input().secure().value("private draft").img(&image));
    rejected("img", || web_view("about:blank").img(&image));
}

#[test]
fn specialized_configuration_requires_matching_content() {
    rejected("multiline", || el().multiline());
    rejected("rows", || text("label").rows(1, 3));
    rejected("submit_on_enter", || el().submit_on_enter());
    rejected("secure", || el().secure());
    rejected("revealed", || el().revealed(|| panic!("a rejected binding must not run")));
    rejected("props", || el().props(|| panic!("a rejected binding must not run")));
    rejected("slot", || el().slot(octara::native::Slot::Underlay));
    rejected("on_native", || el().on_native(|_| {}));
    rejected("stroke_width", || el().stroke_width(1.0));
    rejected("object_fit", || el().object_fit(ObjectFit::Cover));
}

#[test]
fn image_replacement_preserves_fit_children_and_shared_semantics() {
    let first = Image::rgba(2, 1, vec![255; 8]);
    let second = Image::rgba(1, 1, vec![255; 4]);
    let expected = second.clone();
    let window = Window::headless(move || {
        col()
            .size_full()
            .child(
                el().id("picture")
                    .size(80)
                    .img(&first)
                    .object_fit(ObjectFit::Contain)
                    .img(&second)
                    .label("Picture")
                    .value("Image metadata")
                    .child(text("Caption").id("caption")),
            )
            .child(el().id("status").value("Ready"))
            .into()
    });
    let k = window.kernel();
    let node = k.find("picture").unwrap();
    assert_eq!(k.image(node), Some((&expected, ObjectFit::Contain)));
    assert_eq!(k.text_of("caption").as_deref(), Some("Caption"));
    assert_eq!(k.value_of("picture").as_deref(), Some("Image metadata"));
    assert_eq!(k.value_of("status").as_deref(), Some("Ready"));
    assert!(window.layout_issues().is_empty());
    assert!(window.app().diagnostics().is_empty());
}
