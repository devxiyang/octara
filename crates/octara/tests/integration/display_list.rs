//! The display list: quads, glyph runs and clips in drawing order, traced to their nodes.

use octara::prelude::*;
use octara_primitives::paint::{DisplayList, Item, Part};
use std::sync::Arc;

fn source(app: &Window, id: &str) -> octara_primitives::identity::NodeId {
    app.kernel().find(id).unwrap()
}

fn view() -> View {
    let open = state("open", || true);
    col()
        .size_full()
        .p_4()
        .gap_2()
        .child(row().id("card").p_3().rounded_lg().border().bg(Token::Surface).child(text("Hello").id("hello")).overlay(
            move || open.get(),
            Placement::BELOW,
            || text("menu").id("menu").bg(Token::Background),
        ))
        .child(col().id("list").h(40).overflow_y_scroll().children((0..10).map(|i| text(format!("row {i}")).id(format!("r{i}")))))
        .child(text("one two three four five six").id("clamped").w(40).line_clamp(2))
        .into()
}

#[test]
fn quads_glyphs_and_clips_in_order() {
    let app = Window::headless(view);
    let dl: Arc<DisplayList> = app.display_list();
    assert_eq!(dl.size, (800.0, 600.0));
    assert_eq!(dl.background, Theme::light().get(Token::Background));

    // The card: one quad with its fill, border and radius, traced to its node.
    let card = dl.quads().find(|q| q.source == source(&app, "card")).expect("card quad");
    assert_eq!(card.fill, Some(Theme::light().get(Token::Surface)));
    assert_eq!((card.border, card.radius), ([1.0; 4], 8.0));

    // Its text: Inter glyphs inside the card's content box, on one baseline.
    let hello = dl.glyph_runs().find(|g| g.source == source(&app, "hello")).expect("hello glyphs");
    assert_eq!(hello.glyphs.len(), 5);
    assert_eq!(hello.size, 16.0);
    let r = app.kernel().rect(source(&app, "hello"));
    assert!(hello.placed().all(|g| g.x >= r.0 && g.x < r.0 + r.2 && g.y > r.1 && g.y <= r.1 + r.3));
    assert!(hello.glyphs.windows(2).all(|w| w[0].y == w[1].y && w[0].x < w[1].x));

    // The scroll container clips its rows; rows outside it are not drawn.
    let list = source(&app, "list");
    let i = dl.items().iter().position(|it| matches!(it, Item::PushClip { source: s, .. } if *s == list));
    let i = i.expect("list clip");
    let drawn: Vec<_> = dl.glyph_runs().filter(|g| (0..10).any(|k| g.source == source(&app, &format!("r{k}")))).collect();
    assert_eq!(drawn.len(), 2, "two 20 px rows fit in 40 px");
    assert!(dl.items()[i + 1..].iter().any(|it| matches!(it, Item::PopClip)));

    // line_clamp(2): only two lines of glyphs.
    let clamped = dl.glyph_runs().filter(|g| g.source == source(&app, "clamped"));
    let mut baselines: Vec<f32> = clamped.flat_map(|g| g.placed().map(|x| x.y).collect::<Vec<_>>()).collect();
    baselines.dedup();
    assert_eq!(baselines.len(), 2, "{baselines:?}");

    // The overlay comes last: drawn over everything.
    let last_glyphs = dl.glyph_runs().last().unwrap();
    assert_eq!(last_glyphs.source, source(&app, "menu"));
}

#[test]
fn opacity_and_theme_reach_the_colors() {
    let app = Window::headless(|| el().id("box").size_8().bg(Token::Accent).opacity(0.5));
    let fill = app.display_list().quads().next().unwrap().fill.unwrap();
    assert!((fill.components[3] - 0.5).abs() < 1e-6);
    app.app().set_theme(Theme::dark());
    let fill = app.display_list().quads().next().unwrap().fill.unwrap();
    assert_eq!(fill.components[..3], Theme::dark().get(Token::Accent).components[..3]);
}

#[test]
fn custom_paint_params_update_only_its_drawing() {
    let app = App::new(octara::platform::HeadlessPlatform::default());
    let params = app.run(|| state("paint-params", || [0.25, 0.0, 0.0, 1.0]));
    let window = app
        .open_window(WindowOptions::new("Custom paint"), move || {
            let program = PaintProgram::new("test program");
            col().p_4().child(
                el().id("box")
                    .role(Role::Button)
                    .label("Box")
                    .focusable()
                    .size_8()
                    .rounded_md()
                    .border()
                    .bg(Token::Surface)
                    .opacity(0.5)
                    .paint(program, params),
            )
        })
        .unwrap();
    let paint = |list: &DisplayList| {
        let items = list.items();
        let [Item::Custom(paint)] = items.as_slice() else { panic!("expected one custom paint item, got {items:?}") };
        paint.clone()
    };
    let first_frame = window.display_list();
    let first = paint(&first_frame);
    assert_eq!(first.fallback.rect, Rect::new(16.0, 16.0, 32.0, 32.0));
    assert_eq!(first.fallback.border, [1.0; 4]);
    assert_eq!(first.fallback.fill, Some(Theme::light().get(Token::Surface).multiply_alpha(0.5)));
    assert_eq!(first.opacity, 0.5);
    assert_eq!(first.program.downcast_ref::<&str>(), Some(&"test program"));
    assert_eq!(first.params, [0.25, 0.0, 0.0, 1.0]);
    let tree = window.inspect().root;

    window.reset_counters();
    app.run(|| params.set([0.75, 0.0, 0.0, 1.0]));
    let next = paint(&window.display_list());
    assert_eq!(next.params, [0.75, 0.0, 0.0, 1.0]);
    assert_eq!(paint(&first_frame).params, first.params);
    assert_eq!(next.fallback, first.fallback);
    assert!(std::sync::Weak::ptr_eq(&next.program.downgrade(), &first.program.downgrade()));
    assert_eq!(window.inspect().root, tree);
    let work = window.kernel().counters();
    assert_eq!(work.dirty_paint, 1);
    assert_eq!((work.layout_runs, work.dirty_text, work.dirty_semantics, work.structural_ops), (0, 0, 0, 0));
}

mod paint_transition {
    use octara::capture::{PaintOrigin, PaintOutcome, PaintReceipt};
    use octara::platform::{HeadlessPlatform, HeadlessWindow};
    use octara::prelude::*;
    use octara::task::WorkKind;
    use octara_primitives::paint::{DisplayList, Item, TransitionPaint};
    use std::sync::Weak;
    use std::time::{Duration, Instant};

    fn transition(list: &DisplayList) -> TransitionPaint {
        list.items().into_iter().find_map(|item| if let Item::PushTransition(paint) = item { Some(paint) } else { None }).unwrap()
    }

    #[test]
    fn stable_region_samples_motion_and_pins_a_run_through_reversal() {
        let app = App::new(HeadlessPlatform::default());
        let selected = app.run(|| state("selected", || 0_u64));
        let mut window = app
            .open_window(WindowOptions::new("Transition"), move || {
                el().opacity(0.5).child(
                    el().id("region").opacity(0.75).paint_transition(PaintProgram::new("test program"), selected, Curve::Fade(0.2)).child(
                        keyed(
                            move || selected.get(),
                            |key| {
                                el().id("choice")
                                    .role(Role::Button)
                                    .label(if *key == 0 { "First" } else { "Second" })
                                    .size_8()
                                    .bg(if *key == 0 { Token::Surface } else { Token::Accent })
                                    .on_press(|| {})
                            },
                        ),
                    ),
                )
            })
            .unwrap();
        let first = window.published_frame().unwrap();
        let items = first.list.items();
        let [Item::PushLayer { opacity, .. }, Item::PushTransition(initial), Item::Quad(quad), Item::PopLayer, Item::PopLayer] =
            items.as_slice()
        else {
            panic!("expected an outer opacity group and the region's unattenuated contents");
        };
        assert_eq!((*opacity, initial.opacity, quad.fill.unwrap().components[3]), (0.5, 0.75, 1.0));
        assert_eq!((initial.revision, initial.progress), (0, 1.0));
        assert_eq!(app.next_deadline(), None);
        let old_target = window.inspect().find("choice").unwrap().target.clone();

        app.run(|| selected.set(1));
        let replaced = transition(&window.display_list());
        assert_eq!((replaced.revision, replaced.progress), (1, 0.0));
        assert!(Weak::ptr_eq(&initial.mount.downgrade(), &replaced.mount.downgrade()));
        assert!(!Weak::ptr_eq(&initial.run.downgrade(), &replaced.run.downgrade()));
        assert!(app.pending().iter().any(|work| work.kind == WorkKind::Motion && work.scope == "region"));
        let tree = window.inspect();
        let choice = tree.find("choice").unwrap();
        assert_ne!(choice.target, old_target);
        assert_eq!(choice.name.as_deref(), Some("Second"));
        let stale = serde_json::json!({ "op": "tap", "id": "choice", "target": old_target });
        assert_eq!(octara::inspect::handle_line(&mut window, &stale.to_string()).code.as_deref(), Some("STALE_TARGET"));

        let halfway = Duration::from_millis(100);
        let preview = window.published_frame_for_display(halfway).unwrap();
        assert!((transition(&preview.list).progress - 0.5).abs() < 1e-6);
        assert_eq!(transition(&window.display_list()).progress, 0.0);
        app.advance(halfway);
        let mid = window.published_frame().unwrap();
        let outcome = PaintOutcome::Transition { old: first.paint_sample() };
        let report =
            mid.paint_frame(PaintOrigin::Rerecorded { device_generation: None }, &[PaintReceipt { source: replaced.source, outcome }]);
        assert_eq!(report.effects[0].site.id.as_deref(), Some("region"));
        assert_eq!(report.effects[0].outcome, outcome);
        let capture = window.capture_begin(Instant::now() + Duration::from_secs(10)).unwrap();

        app.run(|| selected.set(0));
        let reversed = transition(&window.display_list());
        assert_eq!((reversed.revision, reversed.progress), (2, 0.0));
        assert!(Weak::ptr_eq(&reversed.mount.downgrade(), &replaced.mount.downgrade()));
        assert!(!Weak::ptr_eq(&reversed.run.downgrade(), &replaced.run.downgrade()));
        let prepared = window.capture_prepare(capture).unwrap();
        assert_eq!(prepared.paint_sample(), mid.paint_sample());
        let pinned = transition(&prepared.display_list());
        assert_eq!(pinned.revision, 1);
        assert!((pinned.progress - 0.5).abs() < 1e-6);
        assert!(Weak::ptr_eq(&pinned.run.downgrade(), &replaced.run.downgrade()));

        let old_run = pinned.run.downgrade();
        drop((replaced, mid, preview, pinned, prepared));
        assert!(old_run.upgrade().is_none());
        let last = window.published_frame().unwrap();
        let run = reversed.run.downgrade();
        drop(reversed);
        app.run_until_idle(Duration::from_secs(1)).unwrap();
        assert_eq!(transition(&window.display_list()).progress, 1.0);
        assert_eq!(app.next_deadline(), None);
        assert!(run.upgrade().is_some());
        drop(last);
        assert!(run.upgrade().is_none());
    }

    #[test]
    fn key_change_retires_exits_inside_the_region() {
        let mut window = Window::headless(|| {
            let shown = state("shown", || true);
            col()
                .child(el().id("toggle").size_8().on_press(move || shown.set(false)))
                .child(
                    el().id("region")
                        .paint_transition(PaintProgram::new(()), move || u64::from(shown.get()), Spring::SNAP)
                        .child(show(move || shown.get(), || el().id("inside").size_8().bg(Token::Accent).opacity_motion(Curve::Fade(1.0)))),
                )
                .child(show(move || shown.get(), || el().id("outside").size_8().bg(Token::Accent).opacity_motion(Curve::Fade(1.0))))
        });
        let inside = super::source(&window, "inside");
        let outside = super::source(&window, "outside");
        window.tap("toggle").unwrap();
        assert_eq!(transition(&window.display_list()).progress, 0.0);
        assert!(!window.kernel().is_alive(inside));
        assert!(window.kernel().is_exiting(outside));
        assert!(window.display_list().quads().any(|quad| quad.source == outside));
    }

    #[test]
    fn native_region_keeps_ordinary_exit_and_the_capture_slot() {
        let app = App::new(HeadlessPlatform::with_window(HeadlessWindow { shows_native: true, ..HeadlessWindow::default() }));
        let mut window = app
            .open_window(WindowOptions::new("Native fallback"), || {
                let key = state("key", || 0_u64);
                col().child(el().id("show-native").size_8().on_press(move || key.set(1))).child(el().opacity(0.5).child(
                    el().id("region").opacity(0.5).paint_transition(PaintProgram::new(()), key, Spring::SNAP).child(keyed(
                        move || key.get(),
                        |key| {
                            if *key == 0 {
                                el().id("ordinary").size_8().bg(Token::Accent).opacity_motion(Spring::SNAP)
                            } else {
                                octara::view::native_view("map")
                                    .slot(octara_primitives::native::Slot::Underlay)
                                    .id("map")
                                    .label("Map")
                                    .size_16()
                            }
                        },
                    )),
                ))
            })
            .unwrap();
        let ordinary = super::source(&window, "ordinary");
        let initial = transition(&window.display_list());
        window.tap("show-native").unwrap();
        assert!(window.kernel().is_exiting(ordinary));
        let native = super::source(&window, "map");
        let items = window.display_list().items();
        let [Item::SkipTransition { source, mount, revision: 1 }, Item::Native { source: cut, cut: true, .. }, Item::Quad(exit)] =
            items.as_slice()
        else {
            panic!("the native region should keep ordinary drawing order and opacity: {items:?}");
        };
        assert_eq!((*source, *cut, exit.source), (initial.source, native, ordinary));
        assert!(Weak::ptr_eq(&initial.mount.downgrade(), &mount.downgrade()));
        assert_eq!(exit.fill.unwrap().components[3], 0.25);

        let capture = window.capture_begin(Instant::now() + Duration::from_secs(10)).unwrap();
        let prepared = window.capture_prepare(capture).unwrap();
        let captured = prepared.display_list().items();
        assert!(captured.iter().any(|item| matches!(item, Item::SkipTransition { source, .. } if *source == initial.source)));
        assert!(captured.iter().any(|item| matches!(item, Item::Native { source, cut: false, .. } if *source == native)));
    }

    #[test]
    fn reduce_motion_lands_an_active_run_and_later_keys_paint_without_motion() {
        let app = App::new(HeadlessPlatform::default());
        let key = app.run(|| state("key", || 0_u64));
        let window = app
            .open_window(WindowOptions::new("Transition"), move || {
                el().id("region").size_8().bg(Token::Accent).paint_transition(PaintProgram::new(()), key, Spring::POP)
            })
            .unwrap();
        let tree = window.inspect().root;
        app.run(|| key.set(1));
        app.advance(Duration::from_millis(100));
        let active = transition(&window.display_list());
        assert!(active.progress > 0.0 && active.progress < 1.0);
        assert_eq!(window.inspect().root, tree);
        let run = active.run.downgrade();
        drop(active);

        app.set_reduced_motion(true);
        let landed = transition(&window.display_list());
        assert_eq!((landed.revision, landed.progress), (1, 1.0));
        assert!(run.upgrade().is_none());
        assert_eq!(app.next_deadline(), None);

        app.run(|| key.set(2));
        let instant = transition(&window.display_list());
        assert_eq!((instant.revision, instant.progress), (2, 1.0));
        assert_eq!(app.next_deadline(), None);
    }
}

mod ellipsis {
    use octara::prelude::*;
    use octara_primitives::paint::Item;

    fn glyphs(app: &Window, id: &str) -> Vec<(u32, f32, f32)> {
        let n = app.kernel().find(id).unwrap();
        let mut out = Vec::new();
        for item in app.display_list().items().iter() {
            if let Item::Glyphs(g) = item
                && g.source == n
            {
                out.extend(g.glyphs.iter().map(|g| (g.id, g.x, g.y)));
            }
        }
        out
    }

    fn app() -> Window {
        Window::headless(|| {
            col()
                .child(text("A sentence far too long for its box").id("cut").w(px(120.0)).truncate())
                .child(text("Fits").id("fits").w(px(120.0)).truncate())
                .child(text("One two three four five six seven eight nine ten eleven twelve").id("clamped").w(px(120.0)).line_clamp(2))
        })
    }

    #[test]
    fn cut_text_ends_in_an_ellipsis_inside_its_box() {
        let app = app();
        let dots = glyphs(&app, "cut").last().copied().unwrap();
        let k = app.kernel();
        let (x, _, w, _) = k.rect(k.find("cut").unwrap());
        drop(k);
        // Every glyph starts inside the box, and the last one is the ellipsis.
        assert!(glyphs(&app, "cut").iter().all(|g| g.1 < x + w));
        let fits = glyphs(&app, "fits");
        assert!(fits.iter().all(|g| g.0 != dots.0), "text that fits has no ellipsis");
        assert_eq!(fits.len(), 4);
        // `line_clamp(2)`: two lines, the second ending in the ellipsis.
        let clamped = glyphs(&app, "clamped");
        let ys: std::collections::BTreeSet<i32> = clamped.iter().map(|g| g.2 as i32).collect();
        assert_eq!(ys.len(), 2);
        assert_eq!(clamped.last().unwrap().0, dots.0);
        assert_eq!(clamped.last().unwrap().2, clamped.iter().map(|g| g.2).fold(0.0, f32::max), "on the last line");
    }
}

mod shadows {
    use octara::prelude::*;
    use octara_primitives::paint::Item;

    #[test]
    fn shadows_come_before_their_box_offset_and_spread() {
        let app = Window::headless(|| {
            col()
                .p_8()
                .child(el().id("card").size_16().rounded_md().bg(Token::Surface).shadow_md())
                .child(el().id("own").size_8().shadow([BoxShadow::new(2.0, 3.0, rgb(0xff0000)).blur_radius(4.0).spread_radius(1.0)]))
        });
        let n = app.kernel().find("card").unwrap();
        let dl = app.display_list();
        let all = dl.items();
        let items: Vec<_> =
            all.iter().filter(|i| matches!(i, Item::Shadow(s) if s.source == n) || matches!(i, Item::Quad(q) if q.source == n)).collect();
        // shadow-md is two shadows, drawn last-listed first, then the box.
        assert!(matches!(items.as_slice(), [Item::Shadow(a), Item::Shadow(b), Item::Quad(_)] if a.blur == 4.0 && b.blur == 6.0));
        let own = app.kernel().find("own").unwrap();
        let (x, y, w, _) = app.kernel().rect(own);
        let s = app
            .display_list()
            .items()
            .clone()
            .into_iter()
            .find_map(|i| if let Item::Shadow(s) = i { (s.source == own).then_some(s) } else { None })
            .unwrap();
        assert_eq!((s.rect.x, s.rect.y, s.rect.w), (x + 2.0 - 1.0, y + 3.0 - 1.0, w + 2.0));
        // A theme can change the scale.
        app.app().set_theme(Theme::light().shadow(ShadowStep::Md, vec![]));
        assert!(!app.display_list().items().iter().any(|i| matches!(i, Item::Shadow(s) if s.source == n)));
    }
}

mod gradients {
    use octara::app::Event;
    use octara::prelude::*;

    #[test]
    fn gradients_resolve_tokens_and_follow_states_and_themes() {
        let mut app = Window::headless(|| {
            col().child(
                el().id("g")
                    .size_16()
                    .bg(Token::Surface)
                    .hover(|p| p.bg(linear_gradient(90.0, linear_color_stop(Token::Accent, 0.0), linear_color_stop(Token::Surface, 1.0)))),
            )
        });
        let v = app.kernel().visual_of("g").unwrap();
        assert_eq!((v.background, v.gradient), (Some(Theme::light().get(Token::Surface)), None));
        app.dispatch(Event::Hover("g".into(), true)).unwrap();
        let v = app.kernel().visual_of("g").unwrap();
        assert_eq!(v.background, None);
        let g = v.gradient.unwrap();
        assert_eq!((g.angle, g.from, g.to), (90.0, (Theme::light().get(Token::Accent), 0.0), (Theme::light().get(Token::Surface), 1.0)));
        app.app().set_theme(Theme::dark());
        assert_eq!(app.kernel().visual_of("g").unwrap().gradient.unwrap().from.0, Theme::dark().get(Token::Accent));
        // The quad carries it; the color at the start and end of the line are the stops.
        let q = app.display_list().quads().find(|q| q.gradient.is_some()).unwrap();
        let g = q.gradient.unwrap();
        let r = q.rect;
        let left = g.at(r.x, r.y + r.h / 2.0, r.x, r.y, r.w, r.h);
        assert_eq!(left, Theme::dark().get(Token::Accent).components);
    }
}

#[test]
fn an_unchanged_frame_is_reused() {
    let mut app = Window::headless(|| {
        let on = state("on", || false);
        col()
            .child(
                el().id("box")
                    .size_8()
                    .bg(move || if on.get() { Token::Accent } else { Token::Surface })
                    .on_press(move || on.update(|v| *v = !*v)),
            )
            .child(el().id("moving").size_8().bg(move || if on.get() { Token::Danger } else { Token::Surface }).motion(Spring::SNAP))
            .child(el().id("fixed").size_8().bg(Token::Surface))
    });
    let built = |app: &Window| app.kernel().frames_built();
    let first = app.display_list();
    let n = built(&app);
    let again = app.display_list();
    assert!(Arc::ptr_eq(&first, &again), "nothing changed: the same list");
    assert_eq!(built(&app), n);

    // A press changes a color (and starts a motion): built again, and not reused while it moves.
    app.tap("box").unwrap();
    let pressed = app.display_list();
    assert!(!Arc::ptr_eq(&first, &pressed));
    let fixed = source(&app, "fixed");
    let fixed_drawing = |list: &DisplayList| {
        list.parts
            .iter()
            .find_map(|part| match part {
                Part::Drawing { items, .. } if items.iter().any(|item| matches!(item, Item::Quad(quad) if quad.source == fixed)) => {
                    Some(items.clone())
                }
                _ => None,
            })
            .unwrap()
    };
    assert!(Arc::ptr_eq(&fixed_drawing(&first), &fixed_drawing(&pressed)), "a new frame shares unchanged drawing data");
    let _ = app.display_list();
    assert_eq!(built(&app), n + 2, "rebuilt while something moves");
    app.app().advance(std::time::Duration::from_secs(2));
    let rest = app.display_list();
    assert!(Arc::ptr_eq(&rest, &app.display_list()), "at rest: reused again");

    // A resize and a theme change invalidate it.
    app.resize(400.0, 300.0);
    assert!(!Arc::ptr_eq(&rest, &app.display_list()));
    let before = app.display_list();
    app.app().set_theme(Theme::dark());
    assert!(!Arc::ptr_eq(&before, &app.display_list()));
}

#[test]
fn closing_a_window_releases_its_cached_paint_program() {
    let window = Window::headless(|| el().size_8().paint(octara_primitives::paint::PaintProgram::new(()), [0.0; 4]));
    let frame = window.published_frame().unwrap();
    let program = frame
        .list
        .items()
        .into_iter()
        .find_map(|item| match item {
            Item::Custom(paint) => Some(paint.program.downgrade()),
            _ => None,
        })
        .unwrap();

    window.app().close_window(window.id());
    assert!(window.is_closed());
    assert!(program.upgrade().is_some(), "the retained capture still owns its drawing");
    drop(frame);
    assert!(program.upgrade().is_none(), "a closed window handle must release cached drawings");
}

#[test]
fn retiring_paint_keeps_its_mount_while_a_reopening_gets_a_new_one() {
    use octara_primitives::motion::Curve;
    use std::sync::Weak;
    use std::time::Duration;

    fn painted(window: &Window) -> Vec<octara_primitives::paint::CustomPaint> {
        window
            .published_frame()
            .unwrap()
            .list
            .items()
            .into_iter()
            .filter_map(|item| match item {
                Item::Custom(paint) => Some(paint),
                _ => None,
            })
            .collect()
    }

    let mut window = Window::headless(|| {
        let open = state("open", || true);
        let program = PaintProgram::new(());
        col().child(el().id("toggle").size_8().on_press(move || open.update(|value| *value = !*value))).child(show(
            move || open.get(),
            move || el().id("surface").size_8().opacity_motion(Curve::Fade(0.8)).paint(program.clone(), [0.0; 4]),
        ))
    });
    let initial = painted(&window).pop().unwrap();
    assert!(!initial.exiting);
    let old = initial.mount.downgrade();
    drop(initial);

    window.tap("toggle").unwrap();
    window.app().advance(Duration::from_millis(100));
    assert!(window.kernel().find("surface").is_none());
    let exit = painted(&window).pop().unwrap();
    assert!(exit.exiting);
    assert!(Weak::ptr_eq(&old, &exit.mount.downgrade()));
    drop(exit);

    window.tap("toggle").unwrap();
    window.app().advance(Duration::from_millis(50));
    let drawings = painted(&window);
    assert_eq!(drawings.len(), 2);
    let live = drawings.iter().find(|paint| !paint.exiting).unwrap();
    assert!(!Weak::ptr_eq(&old, &live.mount.downgrade()));
    assert!(drawings.iter().any(|paint| paint.exiting && Weak::ptr_eq(&old, &paint.mount.downgrade())));
    drop(drawings);

    window.app().advance(Duration::from_secs(1));
    let drawings = painted(&window);
    assert_eq!(drawings.len(), 1);
    assert!(!drawings[0].exiting);
    assert!(old.upgrade().is_none());
}
