//! Driving an app by position and keys, as a window does (and as an agent can).

use octara::prelude::*;
use octara_examples::apps;

fn center(app: &Window, id: &str) -> (f32, f32) {
    let k = app.kernel();
    let (x, y, w, h) = k.rect(k.find(id).unwrap());
    (x + w / 2.0, y + h / 2.0)
}

#[test]
fn type_click_toggle_scroll_and_tab_by_position() {
    let mut app = Window::headless(|| apps::todo(30));
    // Click the text field: it takes focus, without a focus ring (that follows the keyboard).
    let (x, y) = center(&app, "new-todo");
    app.click_at(x, y);
    let s = app.kernel().state_of("new-todo").unwrap();
    assert!(s.contains(State::FOCUSED) && !s.contains(State::FOCUS_VISIBLE));
    app.type_text("Buy milk");
    app.press_key("backspace").unwrap();
    assert_eq!(app.kernel().value_of("new-todo").unwrap(), "Buy mil");
    let (x, y) = center(&app, "add");
    app.pointer_move(x, y);
    assert!(app.kernel().state_of("add").unwrap().contains(State::HOVER));
    app.pointer_down(x, y);
    assert!(app.kernel().state_of("add").unwrap().contains(State::PRESSED));
    app.pointer_up(x, y);
    assert!(!app.kernel().state_of("add").unwrap().contains(State::PRESSED));
    assert_eq!(app.kernel().text_of("title-31").unwrap(), "Buy mil");

    // Clicking a checkbox toggles it.
    let (x, y) = center(&app, "toggle-1");
    app.click_at(x, y);
    assert_eq!(app.kernel().checked_of("toggle-1"), Some(true));

    // A press that leaves the element is not a click.
    let (x, y) = center(&app, "toggle-2");
    app.pointer_down(x, y);
    app.pointer_up(x + 300.0, y);
    assert_eq!(app.kernel().checked_of("toggle-2"), Some(false));

    // The wheel scrolls the list under the pointer.
    let (x, y) = center(&app, "todos");
    app.wheel(x, y, 0.0, 120.0);
    let k = app.kernel();
    assert_eq!(k.scroll_offset(k.find("todos").unwrap()).1, 120.0);
    drop(k);

    // Tab walks the focusable elements in order; Space activates a checkbox.
    app.press_key("escape").unwrap();
    app.press_key("tab").unwrap();
    assert!(app.kernel().state_of("new-todo").unwrap().contains(State::FOCUS_VISIBLE));
    app.press_key("tab").unwrap();
    assert!(app.kernel().state_of("add").unwrap().contains(State::FOCUS_VISIBLE));
    assert!(app.app().diagnostics().is_empty(), "{:?}", app.app().diagnostics());
}

#[test]
fn enter_in_the_new_todo_field_adds_it() {
    let mut app = Window::headless(|| apps::todo(2));
    app.tap("new-todo").unwrap();
    app.type_text("Water plants");
    app.press_key("enter").unwrap();
    assert_eq!(app.kernel().text_of("title-3").unwrap(), "Water plants");
    assert_eq!(app.kernel().value_of("new-todo").unwrap(), "", "the field is cleared");
    assert!(app.kernel().state_of("new-todo").unwrap().contains(State::FOCUSED), "and keeps focus for the next one");
}
