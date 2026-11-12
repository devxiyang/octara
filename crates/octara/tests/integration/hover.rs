//! Hover: the element under the mouse and its ancestors are hovered (GPUI, Rune, CSS); groups
//! style members on an ancestor's hover; `hover_alone` keeps the elements around it unlit; hover
//! holds still while layout moves under a still pointer.

use octara::prelude::*;
use std::time::Duration;

fn hovered(app: &Window, id: &str) -> bool {
    app.kernel().state_of(id).unwrap().contains(State::HOVER)
}

fn center(app: &Window, id: &str) -> (f32, f32) {
    let k = app.kernel();
    let (x, y, w, h) = k.shown_rect(k.find(id).unwrap());
    (x + w / 2.0, y + h / 2.0)
}

fn view() -> View {
    col()
        .size_full()
        .p_4()
        .gap_4()
        .child(
            el().id("card")
                .w(200)
                .h(60)
                .p_2()
                .bg(Token::Surface)
                .hover(|p| p.bg(Token::SurfaceHover))
                .child(el().id("button").w(40).h(20).on_press(|| {})),
        )
        .child(
            row()
                .id("row")
                .group("row")
                .w(200)
                .h(30)
                .child(text("A row"))
                .child(el().id("delete").size_4().opacity(0.0).group_hover("row", |p| p.opacity(1.0)).on_press(|| {})),
        )
        .child(
            el().id("outer")
                .w(200)
                .h(60)
                .p_2()
                .hover(|p| p.bg(Token::SurfaceHover))
                .child(el().id("inner").w(80).h(30).hover_alone().hover(|p| p.bg(Token::Accent))),
        )
        .child(el().id("away").w(200).h(20))
        .into()
}

#[test]
fn an_element_and_its_ancestors_are_hovered() {
    let mut app = Window::headless(view);
    let (x, y) = (center(&app, "card").0 + 60.0, center(&app, "card").1);
    app.pointer_move(x, y);
    assert!(hovered(&app, "card"), "a card with only a hover style lights up");
    assert_eq!(app.kernel().visual_of("card").unwrap().background, Some(Theme::light().get(Token::SurfaceHover)));
    let (bx, by) = center(&app, "button");
    app.pointer_move(bx, by);
    assert!(hovered(&app, "button") && hovered(&app, "card"), "over the button the card stays hovered");
    let (ax, ay) = center(&app, "away");
    app.pointer_move(ax, ay);
    assert!(!hovered(&app, "button") && !hovered(&app, "card"));
    // Tests and agents can hover by id; the ancestors come along.
    app.dispatch(Event::Hover("button".into(), true)).unwrap();
    assert!(hovered(&app, "card"));
    app.dispatch(Event::Hover("button".into(), false)).unwrap();
    assert!(!hovered(&app, "card"));
}

#[test]
fn group_members_follow_their_groups_hover() {
    let mut app = Window::headless(view);
    assert_eq!(app.kernel().visual_of("delete").unwrap().opacity, 0.0);
    let (x, y) = center(&app, "row");
    app.pointer_move(x - 60.0, y);
    assert_eq!(app.kernel().visual_of("delete").unwrap().opacity, 1.0, "the row's delete button shows while the row is hovered");
    let (ax, ay) = center(&app, "away");
    app.pointer_move(ax, ay);
    assert_eq!(app.kernel().visual_of("delete").unwrap().opacity, 0.0);
}

#[test]
fn hover_alone_keeps_the_elements_around_it_unlit() {
    let mut app = Window::headless(view);
    let (x, y) = center(&app, "inner");
    app.pointer_move(x, y);
    assert!(hovered(&app, "inner") && !hovered(&app, "outer"));
    let (ox, oy) = center(&app, "outer");
    app.pointer_move(ox + 80.0, oy);
    assert!(hovered(&app, "outer") && !hovered(&app, "inner"));
}

#[test]
fn hover_holds_while_layout_moves_under_a_still_pointer() {
    let mut app = Window::headless(|| {
        let items = state_list("items", || vec![1u32, 2, 3], |i: &u32| *i);
        let next = state("next", || 10u32);
        col()
            .size_full()
            .motion(Spring::REFLOW)
            .child(el().id("add").size_4().on_press(move || {
                let n = next.get();
                next.set(n + 1);
                items.insert(0, n);
            }))
            .child(col().child(each(
                move || items.source(),
                |id, _| el().id(format!("row-{id}")).w(100).h(20).hover(|p| p.bg(Token::SurfaceHover)).into(),
            )))
            .into()
    });
    let (x, y) = center(&app, "row-2");
    app.pointer_move(x, y);
    assert!(hovered(&app, "row-2"));
    app.dispatch(Event::Press("add".into())).unwrap();
    app.app().advance(Duration::from_millis(80));
    assert!(hovered(&app, "row-2"), "held while the rows slide under the still pointer");
    assert!(app.app().run_until_idle(Duration::from_secs(3)).is_ok());
    // Settled: hover is on what is under the pointer now (row-1 slid there).
    assert!(hovered(&app, "row-1") && !hovered(&app, "row-2"));
}

#[test]
fn hover_survives_hot_reload() {
    let mut app = Window::headless(view);
    let (bx, by) = center(&app, "button");
    app.pointer_move(bx, by);
    app.app().hot_reload();
    assert!(hovered(&app, "button") && hovered(&app, "card"));
}

mod tooltips {
    use octara::prelude::*;
    use std::time::Duration;

    fn app() -> Window {
        Window::headless(|| {
            row()
                .child(el().id("copy").size(24.0).focusable().on_press(|| {}).tooltip("Copy"))
                .child(el().id("retry").size(24.0).on_press(|| {}).tooltip("Retry"))
                .child(el().id("plain").size(24.0))
                .into()
        })
    }

    fn center(w: &Window, id: &str) -> (f32, f32) {
        let k = w.kernel();
        let (x, y, ww, h) = k.shown_rect(k.find(id).unwrap());
        (x + ww / 2.0, y + h / 2.0)
    }

    #[test]
    fn a_tooltip_shows_after_the_mouse_rests_and_goes_when_it_leaves() {
        let mut w = app();
        let (x, y) = center(&w, "copy");
        w.pointer_move(x, y);
        assert_eq!(w.tooltip(), None, "not at once");
        w.app().advance(Duration::from_millis(499));
        assert_eq!(w.tooltip(), None);
        w.app().advance(Duration::from_millis(1));
        assert_eq!(w.tooltip().as_deref(), Some("Copy"));
        let tip = w.kernel().find("tooltip").expect("drawn");
        assert_eq!(w.kernel().role(tip), Role::Tooltip);
        let (_, ty, _, _) = w.kernel().shown_rect(tip);
        assert!(ty >= 24.0, "below the element: {ty}");

        // Moving on along the row: the next shows at once.
        let (x, y) = center(&w, "retry");
        w.pointer_move(x, y);
        assert_eq!(w.tooltip().as_deref(), Some("Retry"));
        let (x, y) = center(&w, "plain");
        w.pointer_move(x, y);
        assert_eq!(w.tooltip(), None);
        assert!(w.kernel().find("tooltip").is_none());
    }

    #[test]
    fn a_press_puts_it_away_until_hover_leaves() {
        let mut w = app();
        let (x, y) = center(&w, "copy");
        w.pointer_move(x, y);
        w.app().advance(Duration::from_millis(600));
        assert!(w.tooltip().is_some());
        w.click_at(x, y);
        assert_eq!(w.tooltip(), None);
        w.app().advance(Duration::from_secs(2));
        assert_eq!(w.tooltip(), None, "not again while the mouse stays");
        assert_eq!(w.app().next_deadline(), None, "nothing waits");
    }

    #[test]
    fn its_text_is_the_description_unless_it_repeats_the_label() {
        let w = Window::headless(|| {
            row()
                .child(el().id("a").size(24.0).label("Copy").tooltip("Copy"))
                .child(el().id("b").size(24.0).label("Copy").tooltip("Copy the reply as Markdown"))
                .into()
        });
        let tree = w.accessibility_tree();
        let desc = |id: &str| {
            let n = octara::kernel::a11y::a11y_id(w.kernel().find(id).unwrap());
            tree.nodes.iter().find(|(i, _)| *i == n).and_then(|(_, n)| n.description().map(str::to_owned))
        };
        assert_eq!(desc("a"), None);
        assert_eq!(desc("b").as_deref(), Some("Copy the reply as Markdown"));
    }
}
