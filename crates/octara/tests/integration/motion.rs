//! Property motion: where `motion` is set, style and state changes move on their curves; the
//! display list shows them at the app's time.

use octara::prelude::*;
use octara::task::WorkKind;
use std::time::Duration;

fn ms(n: u64) -> Duration {
    Duration::from_millis(n)
}

fn fill(app: &Window, id: &str) -> Option<Color> {
    let n = app.kernel().find(id).unwrap();
    app.display_list().quads().find(|q| q.source == n).and_then(|q| q.fill)
}

fn red(app: &Window, id: &str) -> f32 {
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
    let mut app = Window::headless(view);
    app.tap("lit").unwrap();
    assert_eq!(red(&app, "plain"), 1.0);
}

#[test]
fn settling_waits_for_touch_momentum_until_it_finishes_or_is_cancelled() {
    use octara::app::{InputEvent, Phase, PointerEvent};

    for cancel in [false, true] {
        let mut window =
            Window::headless(|| col().id("fling-list").w(120.0).h(120.0).overflow_y_scroll().children((0..30).map(|_| el().h(32.0))));
        let app = window.app();
        for (time, phase, y) in [(0, Phase::Down, 100.0), (30, Phase::Move, 70.0), (60, Phase::Move, 40.0), (60, Phase::Up, 40.0)] {
            app.advance_to(ms(time));
            window.event(InputEvent::Pointer(PointerEvent::touch(1, phase, 20.0, y)));
        }
        let list = window.kernel().find("fling-list").unwrap();
        assert_eq!(window.kernel().scroll_offset(list).1, 60.0);
        let pending = app.run_until_idle(Duration::ZERO).expect_err("a released touch is still scrolling");
        assert!(pending.iter().any(|work| work.kind == WorkKind::Motion && work.scope == "fling-list"));
        assert!(window.unsettled().iter().any(|label| label.contains("scroll momentum")));
        if cancel {
            window.event(InputEvent::Active(false));
            app.run_until_idle(Duration::ZERO).unwrap();
            assert_eq!(window.kernel().scroll_offset(list).1, 60.0);
        } else {
            app.run_until_idle(ms(3000)).unwrap();
            assert!(window.kernel().scroll_offset(list).1 > 60.0);
        }
        assert!(window.unsettled().is_empty());
    }
}

#[test]
fn with_motion_a_change_moves_on_its_spring() {
    let mut app = Window::headless(view);
    assert_eq!(red(&app, "moving"), 0.0, "a first mount does not move");
    assert_eq!(app.app().next_deadline(), None);
    app.tap("lit").unwrap();
    assert_eq!(red(&app, "moving"), 0.0, "it starts where it was");
    assert!(app.app().next_deadline().is_some(), "frames keep coming while it moves");
    let p = app.app().pending();
    assert!(p.iter().any(|w| w.kind == WorkKind::Motion && w.scope == "moving"), "{p:?}");
    app.app().advance(ms(60));
    let mid = red(&app, "moving");
    assert!(mid > 0.05 && mid < 0.95, "{mid}");
    app.app().advance(ms(60));
    assert!(red(&app, "moving") > mid, "still moving toward red");
    assert!(app.app().run_until_idle(ms(2000)).is_ok());
    assert_eq!(red(&app, "moving"), 1.0);
    assert_eq!(app.app().next_deadline(), None, "at rest: no more frames");
    // The resolved value was the target all along.
    assert_eq!(app.kernel().visual_of("moving").unwrap().background, Some(rgb(B)));
}

#[test]
fn an_interrupted_change_turns_around_without_a_jump() {
    let mut app = Window::headless(view);
    app.tap("lit").unwrap();
    app.app().advance(ms(80));
    let before = red(&app, "moving");
    app.tap("lit").unwrap();
    let after = red(&app, "moving");
    assert!((before - after).abs() < 1e-3, "{before} → {after}");
    assert!(app.app().run_until_idle(ms(2000)).is_ok());
    assert_eq!(red(&app, "moving"), 0.0);
}

#[test]
fn state_changes_move_too() {
    let mut app = Window::headless(view);
    let k = app.kernel();
    let (x, y, w, h) = k.rect(k.find("hover").unwrap());
    drop(k);
    app.pointer_move(x + w / 2.0, y + h / 2.0);
    assert_eq!(red(&app, "hover"), 0.0);
    app.app().advance(ms(80));
    assert!(red(&app, "hover") > 0.1);
    app.app().settle_motion();
    assert_eq!(red(&app, "hover"), 1.0, "settle_motion lands everything now");
}

#[test]
fn instant_changes_land_at_once() {
    let mut app = Window::headless(view);
    app.tap("jump").unwrap();
    assert_eq!(red(&app, "moving"), 1.0);
    assert_eq!(app.app().next_deadline(), None);
    // The next change moves again, from where it is.
    app.tap("lit").unwrap();
    assert!((red(&app, "moving") - 1.0).abs() < 1e-4);
    assert!(app.app().next_deadline().is_some());
}

fn opacity(app: &Window, id: &str) -> f32 {
    let n = app.kernel().find(id).unwrap();
    app.display_list().quads().find(|q| q.source == n).and_then(|q| q.fill).map_or(0.0, |c| c.components[3])
}

#[test]
fn opacity_moves_and_reduce_motion_fades_it_briefly() {
    let mut app = Window::headless(view);
    app.tap("go").unwrap();
    app.app().advance(ms(60));
    let springy = opacity(&app, "fade");
    assert!(springy > 0.0 && springy < 1.0);
    assert!(app.app().run_until_idle(ms(2000)).is_ok());
    assert!(opacity(&app, "fade") < 1e-3);
    // Reduce Motion: a 0.18 s fade instead of the spring.
    app.app().set_reduced_motion(true);
    app.tap("go").unwrap();
    app.app().advance(ms(90));
    let half = opacity(&app, "fade");
    assert!((half - 0.5).abs() < 0.05, "{half}");
    app.app().advance(ms(100));
    assert_eq!(opacity(&app, "fade"), 1.0);
}

#[test]
fn theme_changes_move_where_motion_is_set() {
    let app = Window::headless(|| col().child(el().id("card").size_8().motion(Spring::SETTLE).bg(Token::Surface)));
    let light = fill(&app, "card").unwrap();
    app.app().set_theme(Theme::dark());
    let now = fill(&app, "card").unwrap();
    assert!((0..4).all(|i| (now.components[i] - light.components[i]).abs() < 1e-4), "the color moves from where it was");
    assert!(app.app().run_until_idle(ms(2000)).is_ok());
    assert_eq!(fill(&app, "card").unwrap(), Theme::dark().get(Token::Surface));
}

#[test]
fn motion_is_inherited_and_can_be_turned_off() {
    let mut app = Window::headless(|| {
        let lit = state("lit", || false);
        let c = move || rgb(if lit.get() { B } else { A });
        col()
            .motion(Spring::SNAP)
            .child(el().id("inherits").size_8().bg(c))
            .child(el().id("off").size_8().motion_none().bg(c))
            .child(el().id("lit").size_4().on_press(move || lit.update(|l| *l = !*l)))
    });
    app.tap("lit").unwrap();
    assert_eq!(red(&app, "inherits"), 0.0);
    assert_eq!(red(&app, "off"), 1.0);
}

#[test]
fn shadows_move_between_steps() {
    let mut app = Window::headless(|| {
        col()
            .p_8()
            .child(el().id("card").size_16().bg(Token::Surface).motion(Spring::SNAP).shadow_sm().hover(|p| p.shadow_xl()).on_press(|| {}))
    });
    let blur = |app: &Window| -> f32 {
        let n = app.kernel().find("card").unwrap();
        app.display_list()
            .items()
            .iter()
            .filter_map(|i| if let octara_primitives::paint::Item::Shadow(s) = i { (s.source == n).then_some(s.blur) } else { None })
            .fold(0.0, f32::max)
    };
    assert_eq!(blur(&app), 3.0);
    let k = app.kernel();
    let (x, y, w, h) = k.rect(k.find("card").unwrap());
    drop(k);
    app.pointer_move(x + w / 2.0, y + h / 2.0);
    app.app().advance(ms(60));
    let mid = blur(&app);
    assert!(mid > 3.0 && mid < 25.0, "{mid}");
    assert!(app.app().run_until_idle(ms(2000)).is_ok());
    assert_eq!(blur(&app), 25.0);
}

mod layout {
    use super::ms;
    use octara::app::Event;
    use octara::prelude::*;

    fn rows() -> View {
        let items = state_list("items", || vec![1u32, 2, 3], |i: &u32| *i);
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
        fn children_each(self, items: StateList<u32, u32>) -> Self;
    }
    impl EachExt for El {
        fn children_each(self, items: StateList<u32, u32>) -> Self {
            self.child(each(move || items.read_only(), |id, _| el().id(format!("row-{id}")).h(20).w(60).on_press(|| {})))
        }
    }

    fn shown_y(app: &Window, id: &str) -> f32 {
        let k = app.kernel();
        k.shown_rect(k.find(id).unwrap()).1
    }
    fn target_y(app: &Window, id: &str) -> f32 {
        let k = app.kernel();
        k.rect(k.find(id).unwrap()).1
    }

    #[test]
    fn resizing_rewraps_kept_text_before_its_shown_size_starts_moving() {
        let mut app = Window::headless(|| {
            col()
                .size_full()
                .layout_motion(Spring::REFLOW)
                .child(text("This help message wraps onto more than one line after the window becomes narrow.").id("help"))
        });
        let help = app.kernel().find("help").unwrap();
        let baselines = |list: &octara_primitives::paint::DisplayList| {
            let mut ys = Vec::new();
            for run in list.glyph_runs().filter(|run| run.source == help) {
                ys.extend(run.glyphs.iter().map(|glyph| glyph.y.to_bits()));
            }
            ys.sort_unstable();
            ys.dedup();
            ys.len()
        };
        assert_eq!(baselines(&app.display_list()), 1, "prime the one-line drawing at the wide size");
        let before = app.kernel().shown_rect(help);

        app.resize(390.0, 640.0);

        let k = app.kernel();
        assert_eq!(k.shown_rect(help), before, "the first animation frame still shows the old size");
        assert_eq!(k.rect(help).2, 390.0, "text already uses the new target layout");
        drop(k);
        assert!(baselines(&app.display_list()) > 1, "the cached drawing must use the new line breaks on the first frame");
        assert!(app.app().run_until_idle(ms(3000)).is_ok());
        assert!(baselines(&app.display_list()) > 1);
        assert!(app.layout_issues().is_empty(), "{:?}", app.layout_issues());
        assert!(app.app().diagnostics().is_empty(), "{:?}", app.app().diagnostics());
    }

    #[test]
    fn rows_slide_when_one_is_inserted_above() {
        let mut app = Window::headless(rows);
        let before = shown_y(&app, "row-1");
        assert_eq!(app.app().next_deadline(), None, "the first layout does not move");
        app.tap("add").unwrap();
        // Laid out at the target at once; shown where it was, moving there.
        assert_eq!(target_y(&app, "row-1"), before + 20.0);
        assert_eq!(shown_y(&app, "row-1"), before);
        // The new row appears where it lands (it does not fly in from the corner).
        assert_eq!(shown_y(&app, "row-4"), target_y(&app, "row-4"));
        app.app().advance(ms(100));
        let mid = shown_y(&app, "row-1");
        assert!(mid > before && mid < before + 20.0, "{mid}");
        // Hit testing follows what is shown.
        let k = app.kernel();
        let (x, y, w, _) = k.shown_rect(k.find("row-1").unwrap());
        assert_eq!(k.hit_test(x + w / 2.0, y + 1.0), k.find("row-1"));
        drop(k);
        assert!(app.app().run_until_idle(ms(3000)).is_ok());
        assert_eq!(shown_y(&app, "row-1"), before + 20.0);
    }

    #[test]
    fn a_size_change_moves_and_what_follows_moves_with_it() {
        let mut app = Window::headless(rows);
        let below = shown_y(&app, "row-1");
        app.tap("grow").unwrap();
        let k = app.kernel();
        assert_eq!(k.shown_rect(k.find("panel").unwrap()).3, 20.0);
        drop(k);
        app.app().advance(ms(120));
        let k = app.kernel();
        let h = k.shown_rect(k.find("panel").unwrap()).3;
        drop(k);
        assert!(h > 20.0 && h < 80.0, "{h}");
        assert!(shown_y(&app, "row-1") > below && shown_y(&app, "row-1") < below + 60.0);
        assert!(app.app().run_until_idle(ms(3000)).is_ok());
        let k = app.kernel();
        assert_eq!(k.shown_rect(k.find("panel").unwrap()).3, 80.0);
    }

    #[test]
    fn scrolling_is_not_layout_motion() {
        let mut app = Window::headless(rows);
        app.dispatch(Event::Scroll("scroller".into(), 0.0, 30.0)).unwrap();
        assert_eq!(app.app().next_deadline(), None);
        assert_eq!(shown_y(&app, "s2"), target_y(&app, "s2"));
    }

    #[test]
    fn reduce_motion_lands_layout_at_once() {
        let mut app = Window::headless(rows);
        app.app().set_reduced_motion(true);
        app.tap("add").unwrap();
        assert_eq!(shown_y(&app, "row-1"), target_y(&app, "row-1"));
    }
}

mod presence {
    use super::ms;
    use octara::app::Event;
    use octara::inspect::{self, Delivery};
    use octara::prelude::*;
    use octara::reactive::untrack;
    use octara::serde_json::json;
    use octara::task::WorkKind;
    use octara_primitives::identity::NodeId;
    use octara_primitives::paint::{Item, Quad, Rect};
    use std::cell::RefCell;
    use std::rc::Rc;
    use std::time::Duration;

    fn app(with_motion: bool) -> Window {
        Window::headless(move || {
            let open = state("open", || true);
            let items = state_list("items", || vec![1u32, 2, 3], |i: &u32| *i);
            let root = col().size_full();
            let root = if with_motion { root.motion(Spring::SNAP) } else { root };
            root.child(el().id("toggle").size_4().on_press(move || open.update(|o| *o = !*o)))
                .child(el().id("drop").size_4().on_press(move || items.remove(&2)))
                .child(show(
                    move || open.get(),
                    || el().id("panel").w(80).h(30).bg(Token::Accent).focusable().child(text("inside").id("inside")),
                ))
                .child(el().id("after").w(80).h(10).bg(Token::Surface))
                .child(
                    col().children(vec![each(
                        move || items.read_only(),
                        |id, _| el().id(format!("row-{id}")).w(60).h(10).bg(Token::Surface),
                    )]),
                )
        })
    }

    /// How opaque the element is drawn: its group layer's opacity, else its fill's alpha.
    fn drawn(app: &Window, id: NodeId) -> Option<f32> {
        let dl = app.display_list();
        let layer = dl
            .items()
            .iter()
            .find_map(|i| if let Item::PushLayer { source, opacity } = i { (*source == id).then_some(*opacity) } else { None });
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
        app.app().advance(ms(60));
        let a = drawn(&app, panel).unwrap_or(0.0);
        assert!(a > 0.0 && a < 1.0, "{a}");
        assert!(app.app().run_until_idle(ms(2000)).is_ok());
        assert_eq!(drawn(&app, panel), None);
        assert_eq!(app.kernel().live_nodes(), live - 2, "destroyed once faded");
    }

    #[test]
    fn a_new_branch_fades_in_but_not_on_the_first_mount() {
        let mut app = app(true);
        assert_eq!(app.app().next_deadline(), None);
        app.tap("toggle").unwrap();
        assert!(app.app().run_until_idle(ms(2000)).is_ok());
        app.tap("toggle").unwrap();
        let panel = app.kernel().find("panel").unwrap();
        let a0 = drawn(&app, panel).unwrap_or(0.0);
        assert!(a0 < 0.05, "{a0}");
        assert!(app.app().run_until_idle(ms(2000)).is_ok());
        assert_eq!(drawn(&app, panel), Some(1.0));
    }

    #[test]
    fn list_rows_fade_out_and_without_motion_everything_is_immediate() {
        let mut app = app(true);
        let row = app.kernel().find("row-2").unwrap();
        app.tap("drop").unwrap();
        assert!(app.kernel().find("row-2").is_none());
        assert!(drawn(&app, row).is_some());
        app.app().settle_motion();
        assert_eq!(drawn(&app, row), None, "settle_motion lands exits too");

        let mut app = self::app(false);
        let panel = app.kernel().find("panel").unwrap();
        app.tap("toggle").unwrap();
        assert_eq!(drawn(&app, panel), None);
        assert_eq!(app.app().next_deadline(), None);
    }

    #[test]
    fn a_leaving_element_loses_focus_and_hot_reload_does_not_fade() {
        let mut app = app(true);
        app.dispatch(Event::Focus("panel".into())).unwrap();
        app.tap("toggle").unwrap();
        assert!(app.kernel().state_of("toggle").is_some());
        assert!(app.accessibility_tree().nodes.iter().all(|(_, n)| n.label() != Some("inside")), "assistive technology no longer sees it");
        app.app().settle_motion();
        app.tap("toggle").unwrap();
        assert!(app.app().run_until_idle(ms(2000)).is_ok());
        app.app().hot_reload();
        assert_eq!(app.app().next_deadline(), None, "a rebuild does not enter");
    }

    #[test]
    fn a_virtual_list_keeps_following_siblings_quiet_during_reload() {
        let window = Window::headless(|| {
            View::fragment([
                col().h(100).overflow_y_scroll().child(virtual_each(Vec::<(u32, ())>::new, |_| 20.0, |_, _| el().h(20))).into(),
                el().id("after-list").size_8().bg(Token::Accent).motion(Spring::SNAP).into(),
            ])
        });

        window.app().hot_reload();
        let after = window.kernel().find("after-list").unwrap();
        assert_eq!(drawn(&window, after), Some(1.0));
        assert_eq!(window.app().next_deadline(), None);
        window.app().shutdown();
    }

    #[test]
    fn a_virtual_list_scrolling_does_not_fade_rows() {
        let mut app = Window::headless(|| {
            let items = state_list("items", || (0..200u32).collect(), |i: &u32| *i);
            col().size_full().motion(Spring::SNAP).child(col().id("list").h(200).overflow_y_scroll().child(virtual_each(
                move || items.read_only(),
                |_| 20.0,
                |id, _| el().id(format!("v{id}")).h(20).bg(Token::Surface),
            )))
        });
        let original = app.kernel().find("v2").unwrap();
        assert_eq!(drawn(&app, original), Some(1.0));
        app.dispatch(Event::Scroll("list".into(), 0.0, 600.0)).unwrap();
        assert!(!app.kernel().is_alive(original));
        assert_eq!(drawn(&app, original), None);
        let incoming = app.kernel().find("v32").unwrap();
        assert_eq!(drawn(&app, incoming), Some(1.0));
        assert_eq!(app.app().next_deadline(), None);

        app.dispatch(Event::Scroll("list".into(), 0.0, -600.0)).unwrap();
        assert!(!app.kernel().is_alive(incoming));
        let remounted = app.kernel().find("v2").unwrap();
        assert_ne!(remounted, original);
        assert_eq!(drawn(&app, remounted), Some(1.0));
        assert_eq!(app.app().next_deadline(), None);
    }

    #[derive(Clone, Copy, Debug)]
    enum RowSource {
        Incremental,
        Snapshot,
    }

    const LAST_ROW: u32 = 99;
    const ROW_HEIGHT: f32 = 24.0;

    fn virtual_tail(source: RowSource) -> (Window, Rc<RefCell<Vec<bool>>>) {
        let cleaned = Rc::new(RefCell::new(Vec::new()));
        let seen = cleaned.clone();
        let window = Window::headless(move || {
            let items = state_list("rows", || (0..=LAST_ROW).map(|key| (key, false)).collect(), |row: &(u32, bool)| row.0);
            on_command("remove-last", move || items.remove(&LAST_ROW));
            on_command("remove-clipped", move || items.remove(&(LAST_ROW - 1)));
            on_command("restore-last", move || items.push((LAST_ROW, true)));
            let seen = seen.clone();
            let render = move |&key: &u32, item: ReadSignal<(u32, bool)>| {
                let replacement = untrack(|| item.get().1);
                if key == LAST_ROW {
                    let seen = seen.clone();
                    on_cleanup(move || seen.borrow_mut().push(replacement));
                }
                el().id(format!("row-{key}"))
                    .label(format!("Row {key}"))
                    .w_full()
                    .h(ROW_HEIGHT)
                    .flex_shrink_0()
                    .bg(if replacement { Token::Accent } else { Token::Surface })
                    .opacity_motion(Spring::SNAP)
                    .focusable()
                    .style_when(State::FOCUSED, |style| style.bg(Token::AccentHover))
                    .on_press(|| {})
            };
            let rows = match source {
                RowSource::Incremental => virtual_each(move || items.read_only(), |_| ROW_HEIGHT, render),
                RowSource::Snapshot => virtual_each(
                    move || items.keys().into_iter().map(|key| (key, items.item(&key).unwrap().get())).collect::<Vec<_>>(),
                    |_| ROW_HEIGHT,
                    render,
                ),
            };
            col().size_full().p_4().child(col().id("tail").w(180.0).h(20.0).overflow_y_scroll().follow_tail().child(rows))
        });
        (window, cleaned)
    }

    fn clipped_row(window: &Window, viewport: NodeId, row: NodeId) -> (Quad, Rect) {
        let mut clips = Vec::new();
        for item in window.display_list().items() {
            match item {
                Item::PushClip { source, rect, .. } => clips.push((source, rect)),
                Item::PopClip => {
                    clips.pop().expect("balanced drawing clips");
                }
                Item::Quad(quad) if quad.source == row => {
                    let &(_, clip) = clips.iter().find(|(source, _)| *source == viewport).expect("the row stays clipped to its viewport");
                    return (quad, clip);
                }
                _ => {}
            }
        }
        panic!("expected row {row:?} to be drawn");
    }

    #[test]
    fn removing_the_virtual_tail_keeps_its_clipped_exit_and_reaps_only_the_old_row() {
        let last_row = format!("row-{LAST_ROW}");
        for source in [RowSource::Incremental, RowSource::Snapshot] {
            let mut midpoint = None::<f32>;
            for sample_hz in [None, Some(60), Some(120)] {
                let (mut window, cleaned) = virtual_tail(source);
                let app = window.app();
                window.dispatch(Event::Focus(last_row.clone())).unwrap();
                let focused = window.inspect();
                assert_eq!(focused.focus.as_deref(), Some(last_row.as_str()));
                let started = app.now();
                let viewport = window.kernel().find("tail").unwrap();
                let old = window.kernel().find(&last_row).unwrap();
                let old_target = focused.find(&last_row).unwrap().target.clone();
                let baseline_nodes = window.kernel().live_nodes();
                let baseline_reactive = app.work().reactive_nodes;
                let scroll_before = window.kernel().scroll_offset(viewport).1;
                let (old_quad, clip) = clipped_row(&window, viewport, old);
                assert!(old_quad.rect.y < clip.y && old_quad.rect.bottom() > clip.y, "the tail row is partly clipped");
                assert_eq!(old_quad.fill, Some(app.theme().get(Token::AccentHover)));

                window.invoke("remove-last").unwrap();
                assert_eq!(window.kernel().scroll_offset(viewport).1, scroll_before - ROW_HEIGHT);
                let removed = window.inspect();
                assert!(removed.find(&last_row).is_none());
                assert!(removed.focus.is_none());
                assert_eq!(cleaned.borrow().as_slice(), &[false]);
                let (exiting, exiting_clip) = clipped_row(&window, viewport, old);
                assert_eq!((exiting.rect, exiting_clip), (old_quad.rect, clip));
                assert_eq!(drawn(&window, old), Some(1.0));

                window.invoke("restore-last").unwrap();
                let new = window.kernel().find(&last_row).unwrap();
                let new_target = window.inspect().find(&last_row).unwrap().target.clone();
                assert_ne!(new, old);
                assert_ne!(new_target, old_target);
                let (exiting, exiting_clip) = clipped_row(&window, viewport, old);
                assert_eq!((exiting.rect, exiting_clip), (old_quad.rect, clip));
                let (new_quad, new_clip) = clipped_row(&window, viewport, new);
                assert_eq!((new_quad.rect, new_clip), (old_quad.rect, clip));
                assert_ne!(new_quad.fill, exiting.fill);
                assert_eq!(drawn(&window, new), Some(1.0));

                let stale = inspect::handle_line(&mut window, &json!({"op":"tap","target":old_target}).to_string());
                assert_eq!(stale.code.as_deref(), Some("STALE_TARGET"));
                assert_eq!(stale.delivery, Some(Delivery::Refused));
                let fresh = inspect::handle_line(&mut window, &json!({"op":"tap","target":new_target}).to_string());
                assert!(fresh.ok, "{fresh:?}");
                assert_eq!(fresh.pointer.last().unwrap().captured.as_ref().unwrap().target, new_target);

                let end =
                    app.pending().into_iter().find(|work| work.kind == WorkKind::Motion).unwrap().due.unwrap() + Duration::from_micros(1);
                if let Some(hz) = sample_hz {
                    for tick in 1.. {
                        let elapsed = Duration::from_secs_f64(f64::from(tick) / f64::from(hz));
                        if started + elapsed >= end {
                            break;
                        }
                        app.advance_to(started + elapsed);
                        if elapsed == ms(50) {
                            let alpha = drawn(&window, old).expect("old row still fading");
                            assert!(alpha > 0.0 && alpha < 1.0, "{alpha}");
                            let (fading, fading_clip) = clipped_row(&window, viewport, old);
                            assert_eq!((fading.rect, fading_clip), (old_quad.rect, clip));
                            assert_eq!(drawn(&window, new), Some(1.0));
                            if let Some(previous) = midpoint {
                                assert!((alpha - previous).abs() < 1e-6, "{alpha} != {previous} at {hz} Hz");
                            }
                            midpoint = Some(alpha);
                        }
                    }
                }
                app.advance_to(end);
                assert!(!window.kernel().is_alive(old), "{source:?} at {sample_hz:?}");
                assert_eq!(drawn(&window, old), None);
                assert_eq!(drawn(&window, new), Some(1.0));
                assert_eq!(window.kernel().live_nodes(), baseline_nodes);
                assert_eq!(app.work().reactive_nodes, baseline_reactive);
                assert_eq!(cleaned.borrow().as_slice(), &[false]);
                assert!(app.pending().is_empty());
                assert!(app.diagnostics().is_empty(), "{:?}", app.diagnostics());
                app.shutdown();
                assert_eq!(cleaned.borrow().as_slice(), &[false, true]);
                assert_eq!(app.work().reactive_nodes, 0);
            }
        }
    }

    #[test]
    fn deleting_a_fully_clipped_resident_row_does_not_start_an_exit() {
        for source in [RowSource::Incremental, RowSource::Snapshot] {
            let (mut window, _) = virtual_tail(source);
            let clipped = window.kernel().find(&format!("row-{}", LAST_ROW - 1)).expect("overscanned row is resident");
            assert_eq!(drawn(&window, clipped), None);
            window.invoke("remove-clipped").unwrap();
            assert!(!window.kernel().is_alive(clipped));
            assert!(window.app().pending().is_empty());
            assert_eq!(window.app().next_deadline(), None);
            window.app().shutdown();
        }
    }
}

mod scrolling {
    use super::ms;
    use octara::app::Event;
    use octara::prelude::*;

    fn app(with_motion: bool) -> Window {
        Window::headless(move || {
            let list =
                col().id("list").h(100).overflow_y_scroll().children((0..50).map(|i| el().id(format!("r{i}")).h(20).flex_shrink_0()));
            let list = if with_motion { list.motion(Spring::GLIDE) } else { list };
            col().size_full().child(list).child(el().id("jump").size_4().on_press(|| {
                scroll_to("list", 0.0, 400.0);
            }))
        })
    }

    fn y(app: &Window) -> f32 {
        let k = app.kernel();
        k.scroll_offset(k.find("list").unwrap()).1
    }

    #[test]
    fn a_requested_scroll_glides_where_layout_moves() {
        let mut app = app(true);
        app.tap("jump").unwrap();
        assert_eq!(y(&app), 0.0);
        assert!(app.app().pending().iter().any(|p| p.kind == octara::task::WorkKind::Motion));
        app.app().advance(ms(100));
        let mid = y(&app);
        assert!(mid > 0.0 && mid < 400.0, "{mid}");
        // Agents' ScrollTo glides the same way; a new target keeps the speed.
        app.dispatch(Event::ScrollTo("list".into(), 0.0, 200.0)).unwrap();
        assert!((y(&app) - mid).abs() < 1e-3);
        assert!(app.app().run_until_idle(ms(3000)).is_ok());
        assert_eq!(y(&app), 200.0);
    }

    #[test]
    fn scroll_glides_sample_each_display_opportunity_and_stop_at_the_target() {
        for hz in [60, 120] {
            let mut window = app(true);
            window.tap("jump").unwrap();
            let app = window.app();
            assert_eq!(app.next_timer_deadline(), None);
            let mut offset = y(&window);
            for tick in 1..=3 * hz {
                assert!(app.next_deadline().is_some(), "{hz} Hz sample {tick}");
                let before = window.published_revision().unwrap();
                let time = std::time::Duration::from_secs_f64(f64::from(tick) / f64::from(hz));
                app.advance_to(time);
                let frame = window.published_frame().unwrap();
                assert!(frame.revision > before);
                assert_eq!(frame.sampled_at, time);
                assert_ne!(y(&window), offset);
                if app.next_deadline().is_none() {
                    break;
                }
                offset = y(&window);
            }
            assert_eq!(y(&window), 400.0);
            assert_eq!(app.next_deadline(), None);
        }
    }

    #[test]
    fn the_users_own_scrolling_takes_over() {
        let mut app = app(true);
        app.tap("jump").unwrap();
        app.app().advance(ms(60));
        let k = app.kernel();
        let (x, yy, w, h) = k.rect(k.find("list").unwrap());
        drop(k);
        let at = y(&app);
        app.wheel(x + w / 2.0, yy + h / 2.0, 0.0, 10.0);
        assert_eq!(y(&app), at + 10.0);
        app.app().advance(ms(500));
        assert_eq!(y(&app), at + 10.0, "the glide stopped");
    }

    #[test]
    fn a_glide_to_the_end_follows_the_tail_as_it_grows() {
        let mut app = Window::headless(|| {
            let rows = state_list("rows", || (0..50u32).collect(), |i: &u32| *i);
            col()
                .size_full()
                .child(
                    col()
                        .id("list")
                        .h(100)
                        .overflow_y_scroll()
                        .follow_tail()
                        .motion(Spring::GLIDE)
                        .child(each(move || rows.read_only(), |i, _| el().id(format!("r{i}")).h(20).flex_shrink_0())),
                )
                .child(el().id("end").size_4().on_press(|| {
                    scroll_to_end("list");
                }))
                .child(el().id("add").size_4().on_press(move || {
                    for _ in 0..10 {
                        rows.push(rows.len() as u32);
                    }
                }))
        });
        assert_eq!(y(&app), 900.0, "starts at the end");
        app.dispatch(Event::ScrollTo("list".into(), 0.0, 0.0)).unwrap();
        assert!(app.app().run_until_idle(ms(3000)).is_ok());
        assert_eq!(y(&app), 0.0);
        app.tap("end").unwrap();
        app.app().advance(ms(60));
        assert!(y(&app) > 0.0 && y(&app) < 900.0);
        app.tap("add").unwrap();
        assert!(app.app().run_until_idle(ms(3000)).is_ok());
        assert_eq!(y(&app), 1100.0, "the glide went on to the new end");
        let k = app.kernel();
        assert!(k.is_following_tail(k.find("list").unwrap()));
    }

    #[test]
    fn without_motion_a_requested_scroll_jumps() {
        let mut app = app(false);
        app.tap("jump").unwrap();
        assert_eq!(y(&app), 400.0);
        assert_eq!(app.app().next_deadline(), None);
    }
}
