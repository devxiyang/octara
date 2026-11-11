//! Property motion: where `motion` is set, style and state changes move on their curves; the
//! display list shows them at the app's time.

use octara::prelude::*;
use octara::task::WorkKind;
use std::time::Duration;

fn ms(n: u64) -> Duration {
    Duration::from_millis(n)
}

fn fill(app: &App, id: &str) -> Option<Color> {
    let n = app.kernel().find(id).unwrap();
    app.display_list().quads().find(|q| q.source == n).and_then(|q| q.fill)
}

fn red(app: &App, id: &str) -> f32 {
    fill(app, id).map_or(0.0, |c| c.components[0])
}

const A: u32 = 0x000000;
const B: u32 = 0xff0000;

fn view() -> View {
    let lit = state("lit", || false);
    let gone = state("gone", || false);
    col()
        .child(el().id("plain").size_8().bg(move || rgb(if lit.get() { B } else { A })))
        .child(el().id("moving").size_8().motion(Spring::SNAP).bg(move || rgb(if lit.get() { B } else { A })))
        .child(el().id("hover").size_8().motion(Spring::SNAP).bg(rgb(A)).hover(|p| p.bg(rgb(B))).on_press(|| {}))
        .child(el().id("fade").size_8().bg(rgb(A)).motion(Spring::SNAP).opacity(move || if gone.get() { 0.0 } else { 1.0 }))
        .child(el().id("lit").size_4().on_press(move || lit.update(|l| *l = !*l)))
        .child(el().id("go").size_4().on_press(move || gone.update(|g| *g = !*g)))
        .child(el().id("jump").size_4().on_press(move || instant(|| lit.update(|l| *l = !*l))))
        .into()
}

#[test]
fn without_motion_changes_land_at_once() {
    let mut app = App::new(view);
    app.tap("lit").unwrap();
    assert_eq!(red(&app, "plain"), 1.0);
}

#[test]
fn with_motion_a_change_moves_on_its_spring() {
    let mut app = App::new(view);
    assert_eq!(red(&app, "moving"), 0.0, "a first mount does not move");
    assert_eq!(app.next_deadline(), None);
    app.tap("lit").unwrap();
    assert_eq!(red(&app, "moving"), 0.0, "it starts where it was");
    assert!(app.next_deadline().is_some(), "frames keep coming while it moves");
    let p = app.pending();
    assert!(p.iter().any(|w| w.kind == WorkKind::Motion && w.scope == "moving"), "{p:?}");
    app.advance(ms(60));
    let mid = red(&app, "moving");
    assert!(mid > 0.05 && mid < 0.95, "{mid}");
    app.advance(ms(60));
    assert!(red(&app, "moving") > mid, "still moving toward red");
    assert!(app.run_until_idle(ms(2000)).is_ok());
    assert_eq!(red(&app, "moving"), 1.0);
    assert_eq!(app.next_deadline(), None, "at rest: no more frames");
    // The resolved value was the target all along.
    assert_eq!(app.kernel().visual_of("moving").unwrap().background, Some(rgb(B)));
}

#[test]
fn an_interrupted_change_turns_around_without_a_jump() {
    let mut app = App::new(view);
    app.tap("lit").unwrap();
    app.advance(ms(80));
    let before = red(&app, "moving");
    app.tap("lit").unwrap();
    let after = red(&app, "moving");
    assert!((before - after).abs() < 1e-3, "{before} → {after}");
    assert!(app.run_until_idle(ms(2000)).is_ok());
    assert_eq!(red(&app, "moving"), 0.0);
}

#[test]
fn state_changes_move_too() {
    let mut app = App::new(view);
    let k = app.kernel();
    let (x, y, w, h) = k.rect(k.find("hover").unwrap());
    drop(k);
    app.pointer_move(x + w / 2.0, y + h / 2.0);
    assert_eq!(red(&app, "hover"), 0.0);
    app.advance(ms(80));
    assert!(red(&app, "hover") > 0.1);
    app.settle_motion();
    assert_eq!(red(&app, "hover"), 1.0, "settle_motion lands everything now");
}

#[test]
fn instant_changes_land_at_once() {
    let mut app = App::new(view);
    app.tap("jump").unwrap();
    assert_eq!(red(&app, "moving"), 1.0);
    assert_eq!(app.next_deadline(), None);
    // The next change moves again, from where it is.
    app.tap("lit").unwrap();
    assert!((red(&app, "moving") - 1.0).abs() < 1e-4);
    assert!(app.next_deadline().is_some());
}

fn opacity(app: &App, id: &str) -> f32 {
    let n = app.kernel().find(id).unwrap();
    app.display_list().quads().find(|q| q.source == n).and_then(|q| q.fill).map_or(0.0, |c| c.components[3])
}

#[test]
fn opacity_moves_and_reduce_motion_fades_it_briefly() {
    let mut app = App::new(view);
    app.tap("go").unwrap();
    app.advance(ms(60));
    let springy = opacity(&app, "fade");
    assert!(springy > 0.0 && springy < 1.0);
    assert!(app.run_until_idle(ms(2000)).is_ok());
    assert!(opacity(&app, "fade") < 1e-3);
    // Reduce Motion: a 0.18 s fade instead of the spring.
    app.set_reduced_motion(true);
    app.tap("go").unwrap();
    app.advance(ms(90));
    let half = opacity(&app, "fade");
    assert!((half - 0.5).abs() < 0.05, "{half}");
    app.advance(ms(100));
    assert_eq!(opacity(&app, "fade"), 1.0);
}

#[test]
fn theme_changes_move_where_motion_is_set() {
    let mut app = App::new(|| col().child(el().id("card").size_8().motion(Spring::SETTLE).bg(Token::Surface)).into());
    let light = fill(&app, "card").unwrap();
    app.set_theme(Theme::dark());
    let now = fill(&app, "card").unwrap();
    assert!((0..4).all(|i| (now.components[i] - light.components[i]).abs() < 1e-4), "the color moves from where it was");
    assert!(app.run_until_idle(ms(2000)).is_ok());
    assert_eq!(fill(&app, "card").unwrap(), Theme::dark().get(Token::Surface));
}

#[test]
fn motion_is_inherited_and_can_be_turned_off() {
    let mut app = App::new(|| {
        let lit = state("lit", || false);
        let c = move || rgb(if lit.get() { B } else { A });
        col()
            .motion(Spring::SNAP)
            .child(el().id("inherits").size_8().bg(c))
            .child(el().id("off").size_8().motion_none().bg(c))
            .child(el().id("lit").size_4().on_press(move || lit.update(|l| *l = !*l)))
            .into()
    });
    app.tap("lit").unwrap();
    assert_eq!(red(&app, "inherits"), 0.0);
    assert_eq!(red(&app, "off"), 1.0);
}

#[test]
fn shadows_move_between_steps() {
    let mut app = App::new(|| {
        col()
            .p_8()
            .child(el().id("card").size_16().bg(Token::Surface).motion(Spring::SNAP).shadow_sm().hover(|p| p.shadow_xl()).on_press(|| {}))
            .into()
    });
    let blur = |app: &App| -> f32 {
        let n = app.kernel().find("card").unwrap();
        app.display_list()
            .items
            .iter()
            .filter_map(|i| if let octara::paint::Item::Shadow(s) = i { (s.source == n).then_some(s.blur) } else { None })
            .fold(0.0, f32::max)
    };
    assert_eq!(blur(&app), 3.0);
    let k = app.kernel();
    let (x, y, w, h) = k.rect(k.find("card").unwrap());
    drop(k);
    app.pointer_move(x + w / 2.0, y + h / 2.0);
    app.advance(ms(60));
    let mid = blur(&app);
    assert!(mid > 3.0 && mid < 25.0, "{mid}");
    assert!(app.run_until_idle(ms(2000)).is_ok());
    assert_eq!(blur(&app), 25.0);
}

mod layout {
    use super::ms;
    use octara::prelude::*;

    fn rows() -> View {
        let items = list("items", || vec![1u32, 2, 3], |i: &u32| *i);
        let next = state("next", || 4u32);
        let tall = state("tall", || false);
        col()
            .size_full()
            .layout_motion(Spring::REFLOW)
            .child(el().id("add").size_4().on_press(move || {
                let n = next.get();
                next.set(n + 1);
                items.insert(0, n);
            }))
            .child(el().id("grow").size_4().on_press(move || tall.update(|t| *t = !*t)))
            .child(el().id("panel").w(100).h(20).selected(move || tall.get()).style_when(State::SELECTED, |p| p.h(80)).bg(Token::Surface))
            .child(col().id("list").children_each(items))
            .child(col().id("scroller").h(40).overflow_y_scroll().children((0..10).map(|i| el().id(format!("s{i}")).h(20).flex_shrink_0())))
            .into()
    }

    trait EachExt {
        fn children_each(self, items: List<u32, u32>) -> Self;
    }
    impl EachExt for El {
        fn children_each(self, items: List<u32, u32>) -> Self {
            self.child(each(move || items.source(), |id, _| el().id(format!("row-{id}")).h(20).w(60).on_press(|| {}).into()))
        }
    }

    fn shown_y(app: &App, id: &str) -> f32 {
        let k = app.kernel();
        k.shown_rect(k.find(id).unwrap()).1
    }
    fn target_y(app: &App, id: &str) -> f32 {
        let k = app.kernel();
        k.rect(k.find(id).unwrap()).1
    }

    #[test]
    fn rows_slide_when_one_is_inserted_above() {
        let mut app = App::new(rows);
        let before = shown_y(&app, "row-1");
        assert_eq!(app.next_deadline(), None, "the first layout does not move");
        app.tap("add").unwrap();
        // Laid out at the target at once; shown where it was, moving there.
        assert_eq!(target_y(&app, "row-1"), before + 20.0);
        assert_eq!(shown_y(&app, "row-1"), before);
        // The new row appears where it lands (it does not fly in from the corner).
        assert_eq!(shown_y(&app, "row-4"), target_y(&app, "row-4"));
        app.advance(ms(100));
        let mid = shown_y(&app, "row-1");
        assert!(mid > before && mid < before + 20.0, "{mid}");
        // Hit testing follows what is shown.
        let k = app.kernel();
        let (x, y, w, _) = k.shown_rect(k.find("row-1").unwrap());
        assert_eq!(k.hit_test(x + w / 2.0, y + 1.0), k.find("row-1"));
        drop(k);
        assert!(app.run_until_idle(ms(3000)).is_ok());
        assert_eq!(shown_y(&app, "row-1"), before + 20.0);
    }

    #[test]
    fn a_size_change_moves_and_what_follows_moves_with_it() {
        let mut app = App::new(rows);
        let below = shown_y(&app, "row-1");
        app.tap("grow").unwrap();
        let k = app.kernel();
        assert_eq!(k.shown_rect(k.find("panel").unwrap()).3, 20.0);
        drop(k);
        app.advance(ms(120));
        let k = app.kernel();
        let h = k.shown_rect(k.find("panel").unwrap()).3;
        drop(k);
        assert!(h > 20.0 && h < 80.0, "{h}");
        assert!(shown_y(&app, "row-1") > below && shown_y(&app, "row-1") < below + 60.0);
        assert!(app.run_until_idle(ms(3000)).is_ok());
        let k = app.kernel();
        assert_eq!(k.shown_rect(k.find("panel").unwrap()).3, 80.0);
    }

    #[test]
    fn scrolling_is_not_layout_motion() {
        let mut app = App::new(rows);
        app.dispatch(Event::Scroll("scroller".into(), 0.0, 30.0)).unwrap();
        assert_eq!(app.next_deadline(), None);
        assert_eq!(shown_y(&app, "s2"), target_y(&app, "s2"));
    }

    #[test]
    fn reduce_motion_lands_layout_at_once() {
        let mut app = App::new(rows);
        app.set_reduced_motion(true);
        app.tap("add").unwrap();
        assert_eq!(shown_y(&app, "row-1"), target_y(&app, "row-1"));
    }
}

mod presence {
    use super::ms;
    use octara::prelude::*;

    fn app(with_motion: bool) -> App {
        App::new(move || {
            let open = state("open", || true);
            let items = list("items", || vec![1u32, 2, 3], |i: &u32| *i);
            let root = col().size_full();
            let root = if with_motion { root.motion(Spring::SNAP) } else { root };
            root.child(el().id("toggle").size_4().on_press(move || open.update(|o| *o = !*o)))
                .child(el().id("drop").size_4().on_press(move || items.remove(&2)))
                .child(show(
                    move || open.get(),
                    || el().id("panel").w(80).h(30).bg(Token::Accent).focusable().child(text("inside").id("inside")).into(),
                ))
                .child(el().id("after").w(80).h(10).bg(Token::Surface))
                .child(col().children(vec![each(
                    move || items.source(),
                    |id, _| el().id(format!("row-{id}")).w(60).h(10).bg(Token::Surface).into(),
                )]))
                .into()
        })
    }

    /// How opaque the element is drawn: its group layer's opacity, else its fill's alpha.
    fn drawn(app: &App, id: octara::kernel::NodeId) -> Option<f32> {
        let dl = app.display_list();
        let layer = dl.items.iter().find_map(|i| {
            if let octara::paint::Item::PushLayer { source, opacity } = i { (*source == id).then_some(*opacity) } else { None }
        });
        let fill = dl.quads().find(|q| q.source == id).and_then(|q| q.fill).map(|c| c.components[3]);
        fill.map(|f| f * layer.unwrap_or(1.0))
    }

    #[test]
    fn a_removed_branch_leaves_the_tree_now_and_fades_out_where_it_was() {
        let mut app = app(true);
        let panel = app.kernel().find("panel").unwrap();
        let after_y = {
            let k = app.kernel();
            k.rect(k.find("after").unwrap()).1
        };
        let live = app.kernel().live_nodes();
        app.tap("toggle").unwrap();
        let k = app.kernel();
        assert!(k.find("panel").is_none() && k.find("inside").is_none(), "gone for test ids at once");
        assert!(k.rect(k.find("after").unwrap()).1 < after_y, "siblings reflow at once");
        drop(k);
        assert_eq!(drawn(&app, panel), Some(1.0), "still shown, starting its fade");
        app.advance(ms(60));
        let a = drawn(&app, panel).unwrap_or(0.0);
        assert!(a > 0.0 && a < 1.0, "{a}");
        assert!(app.run_until_idle(ms(2000)).is_ok());
        assert_eq!(drawn(&app, panel), None);
        assert_eq!(app.kernel().live_nodes(), live - 2, "destroyed once faded");
    }

    #[test]
    fn a_new_branch_fades_in_but_not_on_the_first_mount() {
        let mut app = app(true);
        assert_eq!(app.next_deadline(), None);
        app.tap("toggle").unwrap();
        assert!(app.run_until_idle(ms(2000)).is_ok());
        app.tap("toggle").unwrap();
        let panel = app.kernel().find("panel").unwrap();
        let a0 = drawn(&app, panel).unwrap_or(0.0);
        assert!(a0 < 0.05, "{a0}");
        assert!(app.run_until_idle(ms(2000)).is_ok());
        assert_eq!(drawn(&app, panel), Some(1.0));
    }

    #[test]
    fn list_rows_fade_out_and_without_motion_everything_is_immediate() {
        let mut app = app(true);
        let row = app.kernel().find("row-2").unwrap();
        app.tap("drop").unwrap();
        assert!(app.kernel().find("row-2").is_none());
        assert!(drawn(&app, row).is_some());
        app.settle_motion();
        assert_eq!(drawn(&app, row), None, "settle_motion lands exits too");

        let mut app = self::app(false);
        let panel = app.kernel().find("panel").unwrap();
        app.tap("toggle").unwrap();
        assert_eq!(drawn(&app, panel), None);
        assert_eq!(app.next_deadline(), None);
    }

    #[test]
    fn a_leaving_element_loses_focus_and_hot_reload_does_not_fade() {
        let mut app = app(true);
        app.dispatch(Event::Focus("panel".into())).unwrap();
        app.tap("toggle").unwrap();
        assert!(app.kernel().state_of("toggle").is_some());
        assert!(
            app.accessibility_tree(1.0).nodes.iter().all(|(_, n)| n.label() != Some("inside")),
            "assistive technology no longer sees it"
        );
        app.settle_motion();
        app.tap("toggle").unwrap();
        assert!(app.run_until_idle(ms(2000)).is_ok());
        app.hot_reload();
        assert_eq!(app.next_deadline(), None, "a rebuild does not enter");
    }

    #[test]
    fn a_virtual_list_scrolling_does_not_fade_rows() {
        let mut app = App::new(|| {
            let items = list("items", || (0..200u32).collect(), |i: &u32| *i);
            col()
                .size_full()
                .motion(Spring::SNAP)
                .child(col().id("list").h(200).overflow_y_scroll().child(virtual_each(
                    move || items.source(),
                    |_| 20.0,
                    |id, _| el().id(format!("v{id}")).h(20).into(),
                )))
                .into()
        });
        app.dispatch(Event::Scroll("list".into(), 0.0, 600.0)).unwrap();
        assert_eq!(app.next_deadline(), None);
    }
}

mod scrolling {
    use super::ms;
    use octara::prelude::*;

    fn app(with_motion: bool) -> App {
        App::new(move || {
            let list =
                col().id("list").h(100).overflow_y_scroll().children((0..50).map(|i| el().id(format!("r{i}")).h(20).flex_shrink_0()));
            let list = if with_motion { list.motion(Spring::GLIDE) } else { list };
            col()
                .size_full()
                .child(list)
                .child(el().id("jump").size_4().on_press(|| {
                    scroll_to("list", 0.0, 400.0);
                }))
                .into()
        })
    }

    fn y(app: &App) -> f32 {
        let k = app.kernel();
        k.scroll_offset(k.find("list").unwrap()).1
    }

    #[test]
    fn a_requested_scroll_glides_where_layout_moves() {
        let mut app = app(true);
        app.tap("jump").unwrap();
        assert_eq!(y(&app), 0.0);
        assert!(app.pending().iter().any(|p| p.kind == octara::task::WorkKind::Motion));
        app.advance(ms(100));
        let mid = y(&app);
        assert!(mid > 0.0 && mid < 400.0, "{mid}");
        // Agents' ScrollTo glides the same way; a new target keeps the speed.
        app.dispatch(Event::ScrollTo("list".into(), 0.0, 200.0)).unwrap();
        assert!((y(&app) - mid).abs() < 1e-3);
        assert!(app.run_until_idle(ms(3000)).is_ok());
        assert_eq!(y(&app), 200.0);
    }

    #[test]
    fn the_users_own_scrolling_takes_over() {
        let mut app = app(true);
        app.tap("jump").unwrap();
        app.advance(ms(60));
        let k = app.kernel();
        let (x, yy, w, h) = k.rect(k.find("list").unwrap());
        drop(k);
        let at = y(&app);
        app.wheel(x + w / 2.0, yy + h / 2.0, 0.0, 10.0);
        assert_eq!(y(&app), at + 10.0);
        app.advance(ms(500));
        assert_eq!(y(&app), at + 10.0, "the glide stopped");
    }

    #[test]
    fn without_motion_a_requested_scroll_jumps() {
        let mut app = app(false);
        app.tap("jump").unwrap();
        assert_eq!(y(&app), 400.0);
        assert_eq!(app.next_deadline(), None);
    }
}
