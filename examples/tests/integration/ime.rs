//! Input method composition and the caret, headless.

use octara::prelude::*;
use octara_examples::apps;

#[test]
fn composition_is_shown_underlined_and_committed_text_replaces_it() {
    let mut app = Window::headless(|| apps::todo(3));
    let (x, y, w, h) = {
        let k = app.kernel();
        k.rect(k.find("new-todo").unwrap())
    };
    app.click_at(x + w / 2.0, y + h / 2.0);
    let caret0 = app.caret().expect("a focused field has a caret");
    app.event(InputEvent::Composition(("ni hao").into()));
    let caret1 = app.caret().unwrap();
    assert!(caret1.0 > caret0.0, "the caret follows the composition");
    assert_eq!(app.kernel().value_of("new-todo").unwrap(), "", "composition is not the value yet");
    let id = app.kernel().find("new-todo").unwrap();
    let dl = app.display_list();
    let quads: Vec<_> = dl.quads().filter(|q| q.source == id && q.rect.h <= 1.0).collect();
    assert_eq!(quads.len(), 1, "one underline under the composition");
    app.type_text("你好");
    assert_eq!(app.kernel().value_of("new-todo").unwrap(), "你好");
    assert_eq!(app.kernel().preedit(id), "");
    // The caret is drawn while focused: a 1 px wide quad as tall as a line.
    let caret = app.display_list().quads().filter(|q| q.source == id && q.rect.w == 1.0).count();
    assert_eq!(caret, 1);
}
