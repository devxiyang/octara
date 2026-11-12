//! Interaction states: hover, focus, pressed (forced), disabled, selected, theme.

use octara::prelude::*;

fn tok(t: Token) -> Color {
    Theme::light().get(t)
}

fn view() -> View {
    let disabled = state("disabled", || false);
    let pressed = state("pressed", || 0);
    col()
        .child(
            el().child(text("Go"))
                .id("go")
                .role(Role::Button)
                .focusable()
                .bg(Token::Surface)
                .hover(|p| p.bg(Token::SurfaceHover))
                .active(|p| p.bg(Token::SurfacePressed))
                .focus_visible(|p| p.border_color(Token::Focus))
                .style_when(State::DISABLED, |p| p.opacity(0.5))
                .disabled(move || disabled.get())
                .on_press(move || pressed.update(|n| *n += 1)),
        )
        .child(text(move || format!("{}", pressed.get())).id("count"))
        .child(el().child(text("lock")).id("lock").focusable().on_press(move || disabled.update(|d| *d = !*d)))
        .child(text("plain").id("plain"))
        .into()
}

fn bg(app: &Window, id: &str) -> Option<Color> {
    app.kernel().visual_of(id).unwrap().background
}

#[test]
fn hover_and_focus_move_between_elements() {
    let mut app = Window::headless(view);
    assert_eq!(bg(&app, "go"), Some(tok(Token::Surface)));
    app.dispatch(Event::Hover("go".into(), true)).unwrap();
    assert_eq!(bg(&app, "go"), Some(tok(Token::SurfaceHover)));
    app.dispatch(Event::Hover("lock".into(), true)).unwrap();
    assert!(!app.kernel().state_of("go").unwrap().contains(State::HOVER), "one hovered element at a time");
    assert_eq!(bg(&app, "go"), Some(tok(Token::Surface)));

    app.dispatch(Event::Focus("go".into())).unwrap();
    assert_eq!(app.kernel().visual_of("go").unwrap().border, tok(Token::Focus));
    app.dispatch(Event::Focus("lock".into())).unwrap();
    assert!(!app.kernel().state_of("go").unwrap().contains(State::FOCUS_VISIBLE));
    assert!(app.dispatch(Event::Focus("plain".into())).unwrap_err().contains("not focusable"));
    app.dispatch(Event::Blur).unwrap();
    assert!(!app.kernel().state_of("lock").unwrap().contains(State::FOCUS_VISIBLE));
}

#[test]
fn disabled_refuses_events_and_applies_its_style() {
    let mut app = Window::headless(view);
    app.dispatch(Event::Press("go".into())).unwrap();
    assert_eq!(app.kernel().text_of("count").unwrap(), "1");
    app.dispatch(Event::Press("lock".into())).unwrap();
    assert_eq!(app.kernel().visual_of("go").unwrap().opacity, 0.5);
    assert!(app.dispatch(Event::Press("go".into())).unwrap_err().contains("disabled"));
    assert!(app.dispatch(Event::Focus("go".into())).unwrap_err().contains("disabled"));
    assert_eq!(app.kernel().text_of("count").unwrap(), "1");
}

#[test]
fn forced_and_interaction_states_survive_hot_reload() {
    let mut app = Window::headless(view);
    app.force_state("go", State::PRESSED, true).unwrap();
    assert_eq!(bg(&app, "go"), Some(tok(Token::SurfacePressed)));
    app.dispatch(Event::Focus("lock".into())).unwrap();
    app.app().hot_reload();
    assert_eq!(bg(&app, "go"), Some(tok(Token::SurfacePressed)));
    assert!(app.kernel().state_of("lock").unwrap().contains(State::FOCUS_VISIBLE));
    assert!(app.force_state("go", State::DISABLED, true).is_err(), "props own DISABLED");
}

#[test]
fn theme_switch_repaints_without_running_closures() {
    let app = Window::headless(view);
    app.app().reset_work();
    app.app().set_theme(Theme::dark());
    assert_eq!(bg(&app, "go"), Some(Theme::dark().get(Token::Surface)));
    assert_eq!(app.app().work().closures_run, 0);
}
