//! The display list: quads, glyph runs and clips in drawing order, traced to their nodes.

use octara::paint::{DisplayList, Item};
use octara::prelude::*;

fn source(app: &App, id: &str) -> octara::kernel::NodeId {
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
            || text("menu").id("menu").bg(Token::Background).into(),
        ))
        .child(col().id("list").h(40).overflow_y_scroll().children((0..10).map(|i| text(format!("row {i}")).id(format!("r{i}")))))
        .child(text("one two three four five six").id("clamped").w(40).line_clamp(2))
        .into()
}

#[test]
fn quads_glyphs_and_clips_in_order() {
    let app = App::new(view);
    let dl: std::rc::Rc<DisplayList> = app.display_list();
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
    assert!(hello.glyphs.iter().all(|g| g.x >= r.0 && g.x < r.0 + r.2 && g.y > r.1 && g.y <= r.1 + r.3));
    assert!(hello.glyphs.windows(2).all(|w| w[0].y == w[1].y && w[0].x < w[1].x));

    // The scroll container clips its rows; rows outside it are not drawn.
    let list = source(&app, "list");
    let i = dl.items.iter().position(|it| matches!(it, Item::PushClip { source: s, .. } if *s == list));
    let i = i.expect("list clip");
    let drawn: Vec<_> = dl.glyph_runs().filter(|g| (0..10).any(|k| g.source == source(&app, &format!("r{k}")))).collect();
    assert_eq!(drawn.len(), 2, "two 20 px rows fit in 40 px");
    assert!(dl.items[i + 1..].iter().any(|it| matches!(it, Item::PopClip)));

    // line_clamp(2): only two lines of glyphs.
    let clamped = dl.glyph_runs().filter(|g| g.source == source(&app, "clamped"));
    let mut baselines: Vec<f32> = clamped.flat_map(|g| g.glyphs.iter().map(|x| x.y)).collect();
    baselines.dedup();
    assert_eq!(baselines.len(), 2, "{baselines:?}");

    // The overlay comes last: drawn over everything.
    let last_glyphs = dl.glyph_runs().last().unwrap();
    assert_eq!(last_glyphs.source, source(&app, "menu"));
}

#[test]
fn opacity_and_theme_reach_the_colors() {
    let mut app = App::new(|| el().id("box").size_8().bg(Token::Accent).opacity(0.5).into());
    let fill = app.display_list().quads().next().unwrap().fill.unwrap();
    assert!((fill.components[3] - 0.5).abs() < 1e-6);
    app.set_theme(Theme::dark());
    let fill = app.display_list().quads().next().unwrap().fill.unwrap();
    assert_eq!(fill.components[..3], Theme::dark().get(Token::Accent).components[..3]);
}

mod ellipsis {
    use octara::paint::Item;
    use octara::prelude::*;

    fn glyphs(app: &App, id: &str) -> Vec<(u32, f32, f32)> {
        let n = app.kernel().find(id).unwrap();
        let mut out = Vec::new();
        for item in app.display_list().items.iter() {
            if let Item::Glyphs(g) = item
                && g.source == n
            {
                out.extend(g.glyphs.iter().map(|g| (g.id, g.x, g.y)));
            }
        }
        out
    }

    fn app() -> App {
        App::new(|| {
            col()
                .child(text("A sentence far too long for its box").id("cut").w(px(120.0)).truncate())
                .child(text("Fits").id("fits").w(px(120.0)).truncate())
                .child(text("One two three four five six seven eight nine ten eleven twelve").id("clamped").w(px(120.0)).line_clamp(2))
                .into()
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
    use octara::paint::Item;
    use octara::prelude::*;

    #[test]
    fn shadows_come_before_their_box_offset_and_spread() {
        let mut app = App::new(|| {
            col()
                .p_8()
                .child(el().id("card").size_16().rounded_md().bg(Token::Surface).shadow_md())
                .child(el().id("own").size_8().shadow([BoxShadow::new(2.0, 3.0, rgb(0xff0000)).blur_radius(4.0).spread_radius(1.0)]))
                .into()
        });
        let n = app.kernel().find("card").unwrap();
        let dl = app.display_list();
        let items: Vec<_> = dl
            .items
            .iter()
            .filter(|i| matches!(i, Item::Shadow(s) if s.source == n) || matches!(i, Item::Quad(q) if q.source == n))
            .collect();
        // shadow-md is two shadows, drawn last-listed first, then the box.
        assert!(matches!(items.as_slice(), [Item::Shadow(a), Item::Shadow(b), Item::Quad(_)] if a.blur == 4.0 && b.blur == 6.0));
        let own = app.kernel().find("own").unwrap();
        let (x, y, w, _) = app.kernel().rect(own);
        let s = app
            .display_list()
            .items
            .clone().into_iter()
            .find_map(|i| if let Item::Shadow(s) = i { (s.source == own).then_some(s) } else { None })
            .unwrap();
        assert_eq!((s.rect.x, s.rect.y, s.rect.w), (x + 2.0 - 1.0, y + 3.0 - 1.0, w + 2.0));
        // A theme can change the scale.
        app.set_theme(Theme::light().shadow(ShadowStep::Md, vec![]));
        assert!(!app.display_list().items.iter().any(|i| matches!(i, Item::Shadow(s) if s.source == n)));
    }
}

mod gradients {
    use octara::prelude::*;

    #[test]
    fn gradients_resolve_tokens_and_follow_states_and_themes() {
        let mut app =
            App::new(|| {
                col()
                    .child(el().id("g").size_16().bg(Token::Surface).hover(|p| {
                        p.bg(linear_gradient(90.0, linear_color_stop(Token::Accent, 0.0), linear_color_stop(Token::Surface, 1.0)))
                    }))
                    .into()
            });
        let v = app.kernel().visual_of("g").unwrap();
        assert_eq!((v.background, v.gradient), (Some(Theme::light().get(Token::Surface)), None));
        app.dispatch(Event::Hover("g".into(), true)).unwrap();
        let v = app.kernel().visual_of("g").unwrap();
        assert_eq!(v.background, None);
        let g = v.gradient.unwrap();
        assert_eq!((g.angle, g.from, g.to), (90.0, (Theme::light().get(Token::Accent), 0.0), (Theme::light().get(Token::Surface), 1.0)));
        app.set_theme(Theme::dark());
        assert_eq!(app.kernel().visual_of("g").unwrap().gradient.unwrap().from.0, Theme::dark().get(Token::Accent));
        // The quad carries it; the color at the start and end of the line are the stops.
        let q = app.display_list().quads().find(|q| q.gradient.is_some()).copied().unwrap();
        let g = q.gradient.unwrap();
        let r = q.rect;
        let left = g.at(r.x, r.y + r.h / 2.0, r.x, r.y, r.w, r.h);
        assert_eq!(left, Theme::dark().get(Token::Accent).components);
    }
}

#[test]
fn an_unchanged_frame_is_reused() {
    let mut app = App::new(|| {
        let on = state("on", || false);
        col()
            .child(el().id("box").size_8().bg(move || if on.get() { Token::Accent } else { Token::Surface }).on_press(move || on.update(|v| *v = !*v)))
            .child(el().id("moving").size_8().bg(move || if on.get() { Token::Danger } else { Token::Surface }).motion(Spring::SNAP))
            .into()
    });
    let built = |app: &App| app.kernel().frames_built();
    let first = app.display_list();
    let n = built(&app);
    let again = app.display_list();
    assert!(std::rc::Rc::ptr_eq(&first, &again), "nothing changed: the same list");
    assert_eq!(built(&app), n);

    // A press changes a color (and starts a motion): built again, and not reused while it moves.
    app.tap("box").unwrap();
    let pressed = app.display_list();
    assert!(!std::rc::Rc::ptr_eq(&first, &pressed));
    let _ = app.display_list();
    assert_eq!(built(&app), n + 2, "rebuilt while something moves");
    app.advance(std::time::Duration::from_secs(2));
    let rest = app.display_list();
    assert!(std::rc::Rc::ptr_eq(&rest, &app.display_list()), "at rest: reused again");

    // A resize and a theme change invalidate it.
    app.resize(400.0, 300.0);
    assert!(!std::rc::Rc::ptr_eq(&rest, &app.display_list()));
    let before = app.display_list();
    app.set_theme(Theme::dark());
    assert!(!std::rc::Rc::ptr_eq(&before, &app.display_list()));
}
