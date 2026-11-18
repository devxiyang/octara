//! Content-specific builder calls fail before mounting incompatible payloads or bindings.

use octara::prelude::*;
use octara::view::{canvas, native_view, web_view};
use std::panic::{AssertUnwindSafe, catch_unwind};

fn rejected(method: &str, build: impl FnOnce() -> El) {
    let error = catch_unwind(AssertUnwindSafe(build)).err().expect("incompatible content was accepted");
    let message = error.downcast_ref::<String>().map(String::as_str).or_else(|| error.downcast_ref::<&str>().copied()).unwrap();
    assert!(message.contains("ELEMENT_CONTENT") && message.contains(method), "{message}");
}

#[test]
fn atomic_content_rejects_application_children_before_conversion_or_iteration() {
    struct Unconverted;
    impl IntoView for Unconverted {
        fn into_view(self, _: &'static std::panic::Location<'static>) -> View {
            panic!("rejected child conversion must not run")
        }
    }

    let picture = Image::rgba(1, 1, vec![255; 4]);
    let atomic = || {
        vec![
            text("Text"),
            text_input(),
            image(&picture),
            icon(Icon::stroked("empty", &[])),
            canvas(|_| panic!("rejected canvas must not draw")),
            native_view("test"),
            web_view("about:blank"),
        ]
    };
    for element in atomic() {
        rejected("child", || element.child(Unconverted));
    }
    for element in atomic() {
        rejected("children", || element.children(std::iter::from_fn(|| -> Option<El> { panic!("rejected iterator must not advance") })));
    }
    for element in atomic() {
        rejected("children", || element.children(std::iter::empty::<El>()));
    }
}

#[test]
fn specialized_configuration_requires_matching_content() {
    rejected("placeholder", || el().placeholder(|| -> String { panic!("rejected hint must not run") }));
    rejected("multiline", || el().multiline());
    rejected("rows", || text("label").rows(1, 3));
    rejected("submit_on_enter", || el().submit_on_enter());
    rejected("secure", || el().secure());
    rejected("revealed", || el().revealed(|| panic!("a rejected binding must not run")));
    rejected("props", || el().props(|| panic!("a rejected binding must not run")));
    rejected("slot", || el().slot(octara_primitives::native::Slot::Underlay));
    rejected("on_native", || el().on_native(|_| {}));
    rejected("stroke_width", || el().stroke_width(1.0));
    rejected("object_fit", || el().object_fit(ObjectFit::Cover));
}

#[test]
fn an_image_has_its_own_fit_and_semantics_inside_a_container() {
    let picture = Image::rgba(1, 1, vec![255; 4]);
    let expected = picture.clone();
    let window = Window::headless(move || {
        col()
            .size_full()
            .child(
                col()
                    .id("figure")
                    .child(image(&picture).id("picture").size(80).object_fit(ObjectFit::Contain).label("Picture").value("Image metadata"))
                    .child(text("Caption").id("caption")),
            )
            .child(image(&picture).id("preview").size(40).role(Role::Button).label("Preview image").on_press(|| {}))
            .child(el().id("status").value("Ready"))
    });
    let tree = window.inspect();
    assert_eq!(tree.find("picture").unwrap().role, "image");
    let preview = tree.find("preview").unwrap();
    assert_eq!(preview.role, "button");
    assert!(preview.actions.iter().any(|action| action == "press"));
    let k = window.kernel();
    let node = k.find("picture").unwrap();
    assert_eq!(k.kind(node), octara::kernel::Kind::Image);
    assert_eq!(k.role(node), Role::Image);
    assert_eq!(k.image(node), Some((&expected, ObjectFit::Contain)));
    assert_eq!(k.origin(node).unwrap().file(), file!());
    assert!(k.children(node).is_empty());
    assert_eq!(k.children(k.find("figure").unwrap()), &[node, k.find("caption").unwrap()]);
    assert_eq!(k.text_of("caption").as_deref(), Some("Caption"));
    assert_eq!(k.value_of("picture").as_deref(), Some("Image metadata"));
    assert_eq!(k.value_of("status").as_deref(), Some("Ready"));
    assert!(window.layout_issues().is_empty());
    assert!(window.app().diagnostics().is_empty());
}
