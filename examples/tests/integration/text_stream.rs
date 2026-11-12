use octara::prelude::*;
use octara_examples::apps::chat_stream;
use octara_examples::scenarios::{external, press};
use octara_kit::button;

#[test]
fn streamed_chat_passes_the_chat_scenarios() {
    octara_examples::scenarios::chat_acceptance(&mut Window::headless(|| chat_stream(0))).unwrap();
    octara_examples::scenarios::chat_hot_reload(&mut Window::headless(|| chat_stream(0))).unwrap();
}

#[test]
fn a_token_costs_its_own_length() {
    let mut h = Window::headless(|| chat_stream(1_000));
    external(&mut h, "user", "go").unwrap();
    h.dispatch(Event::ScrollTo("messages".into(), 0.0, 1e9)).unwrap();
    for _ in 0..2_000 {
        external(&mut h, "token", "0123456789").unwrap(); // body grows to 20 KB
    }
    h.kernel_mut().reset_counters();
    external(&mut h, "token", "abc").unwrap();
    let c = h.kernel().counters();
    assert_eq!(c.text_bytes, 3, "only the token is written");
    assert_eq!(c.prop_changes, 1);
    assert_eq!(h.kernel().text_of("msg-body-1002").unwrap().len(), 20_003);
    let before = h.kernel().tree_dump();
    h.app().hot_reload();
    assert_eq!(h.kernel().tree_dump(), before, "incremental and from-scratch trees differ");
    assert!(h.app().diagnostics().is_empty(), "{:?}", h.app().diagnostics());
}

#[test]
fn random_edits_match_a_reference_and_logs_stay_bounded() {
    thread_local! {
        static HANDLE: std::cell::RefCell<Option<TextBuffer>> = const { std::cell::RefCell::new(None) };
    }
    let mut h = Window::headless(|| {
        let t = text_buffer("t", || "hello".into());
        HANDLE.with(|c| *c.borrow_mut() = Some(t.clone()));
        let t2 = t.clone();
        col()
            .child(text(&t).id("live"))
            .child(text(move || t2.get()).id("full"))
            .child(button("x").id("edit").on_press(move || {
                // A deterministic pseudo-random edit derived from the current length.
                let n = t.get_untracked().len();
                match n % 3 {
                    0 => t.append("ab"),
                    1 => t.splice(n / 3..n / 2, "xyz"),
                    _ => t.set(&"q".repeat(n % 7 + 1)),
                }
            }))
            .into()
    });
    let mut reference = String::from("hello");
    for _ in 0..500 {
        press(&mut h, "edit").unwrap();
        let n = reference.len();
        match n % 3 {
            0 => reference.push_str("ab"),
            1 => reference.replace_range(n / 3..n / 2, "xyz"),
            _ => reference = "q".repeat(n % 7 + 1),
        }
        assert_eq!(h.kernel().text_of("live").as_deref(), Some(reference.as_str()));
        assert_eq!(h.kernel().text_of("full").as_deref(), Some(reference.as_str()));
    }
    let len = HANDLE.with(|c| c.borrow().as_ref().unwrap().log_len());
    assert_eq!(len, 0, "the edit log is drained by its reader");
}
