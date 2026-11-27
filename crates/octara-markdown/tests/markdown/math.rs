use super::*;
use octara::PaintItem;
use octara_markdown::{BuiltinMathRenderer, MathError, MathLayout, MathRenderStyle, MathRenderer};

#[test]
fn builtin_math_typesets_supported_tex_as_deterministic_paths() {
    let renderer = BuiltinMathRenderer;
    let style = MathRenderStyle { font_size: 16.0, color: ColorToken::Text.into(), display: true };
    for tex in [
        r"x_i^2 + \frac{\alpha}{\beta} = \sqrt[3]{\gamma}",
        r"\sum_{n=0}^\infty x_n + \int_0^1 \mathrm{d}x = \prod_{k=1}^{4} k",
        r"\left(\frac{\pi}{2}\right) \times \mathbf{V} \to \mathbb{R}",
        r"\begin{pmatrix} a & b \\ c & d \end{pmatrix}",
        r"\begin{cases} x^2 & \text{if } x \ge 0 \\ -x & \text{otherwise} \end{cases}",
    ] {
        let drawing = renderer.render(tex, style).unwrap_or_else(|error| panic!("{tex}: {error}"));
        assert!(drawing.size.width > 20.0 && drawing.size.height > 20.0, "{tex}: {:?}", drawing.size);
        assert_eq!(drawing, renderer.render(tex, style).unwrap());
        assert!(matches!(drawing.ops.as_ref(), [CanvasOp::Path { fill: Fill::Solid(color), path, .. }]
            if *color == style.color && !path.commands().is_empty()));
    }
    for tex in [r"\frac{a}", r"\unsupported{x}", r"\left(a", r"x^2^3", r"\begin{cases}a\end{matrix}"] {
        assert!(renderer.render(tex, style).is_err(), "{tex}");
    }
    let too_deep = format!("{}x{}", "{".repeat(100), "}".repeat(100));
    assert!(renderer.render(&too_deep, style).is_err());
}

fn formula_node(mounted: &Mounted, row: &str) -> NodeId {
    descendant(mounted, block(mounted, row), |node| mounted.tree().text(node).is_some_and(|text| !text.attachments.is_empty()))
}

fn paint_color(mounted: &Mounted, node: NodeId) -> Color {
    let display = mounted.window().last_commit().unwrap().draw.as_ref().unwrap();
    display
        .items()
        .iter()
        .find_map(|item| match item {
            PaintItem::Path(path) if path.source == node => Some(path.color),
            _ => None,
        })
        .expect("formula painted as a path")
}

#[test]
fn inline_math_keeps_exact_source_and_updates_vector_color() {
    let formula = r"$\alpha + x_1$";
    let source = format!("Before {formula} after.");
    let mut driver = HeadlessDriver::default();
    let green = rgb(0x007722);
    let red = rgb(0xaa3311);
    let color = driver.app_mut().build(|b| b.signal(green)).unwrap();
    let mut mounted = Mounted::new(driver, move |b| {
        markdown(b, source)
            .theme_patch(move |r: &mut ReadCx<'_>| ThemePatch::default().color(ColorToken::Text, r.get(color)))
            .test_id("markdown")
    });
    let node = formula_node(&mounted, "b0");
    let range = 7..7 + formula.len();
    let slot = mounted.tree().text(node).unwrap();
    assert_eq!(slot.text.get(range.clone()), Some(formula));
    assert_eq!(slot.attachments[0].range, range);
    assert_eq!(slot.attachments[0].align, InlineAlign::Baseline);
    assert!(matches!(slot.attachments[0].content, InlineContent::Vector { .. }));
    let before = slot.attachments[0].clone();
    assert_eq!(paint_color(&mounted, node), green);
    mounted.update(|w| w.set(color, red));
    assert_eq!(paint_color(&mounted, node), red);
    assert_eq!(mounted.tree().text(node).unwrap().attachments[0], before);
    assert_copied(&mut mounted, TextPosition { node, offset: range.start }, TextPosition { node, offset: range.end }, formula);
}

#[test]
fn inline_math_baseline_aligns_with_surrounding_text() {
    let mounted = Mounted::new(HeadlessDriver::default(), |b| markdown(b, "g $g_j$ y").test_id("markdown"));
    let node = formula_node(&mounted, "b0");
    let slot = mounted.tree().text(node).unwrap();
    let attachment = &slot.attachments[0];
    let baseline = attachment.baseline.unwrap();
    assert!(attachment.size.height - baseline > 1.0, "the formula has an actual descent");

    let frame = mounted.window().last_commit().unwrap().draw.as_ref().unwrap();
    let (mut path_baseline, mut text_baseline) = (None, None);
    for item in frame.items() {
        match item {
            PaintItem::Path(path) if path.source == node => path_baseline = Some(path.transform.y + baseline),
            PaintItem::Glyphs(run) if run.source == node => text_baseline = run.placed().next().map(|glyph| glyph.y),
            _ => {}
        }
    }
    assert!((path_baseline.unwrap() - text_baseline.unwrap()).abs() < 0.5);
}

#[test]
fn display_math_centers_narrow_content_and_scrolls_wide_content() {
    let short = "$$x+1$$";
    let wide = format!("$$\n{}\n$$", "x + ".repeat(160) + "x");
    let source = format!("{short}\n\n{wide}");
    let mut mounted = Mounted::new(HeadlessDriver::default(), move |b| markdown(b, source).w(280.0).test_id("markdown"));
    let row = block(&mounted, "b0");
    let formula = formula_node(&mounted, "b0");
    let parent = mounted.window().geometry(row).unwrap().rect;
    let formula_rect = mounted.window().geometry(formula).unwrap().rect;
    assert!((formula_rect.x + formula_rect.w * 0.5 - parent.w * 0.5).abs() < 1.0);
    let row = block(&mounted, "b1");
    let formula = formula_node(&mounted, "b1");
    assert_eq!(mounted.tree().layout_style(row).overflow_x, Overflow::Auto);
    let parent = mounted.window().geometry(row).unwrap().rect;
    let formula_rect = mounted.window().geometry(formula).unwrap().rect;
    assert!(formula_rect.x >= 0.0 && formula_rect.w > parent.w);
    let wheel = InteractionEvent::Wheel {
        pos: point(parent.x + parent.w * 0.5, parent.y + parent.h * 0.5),
        delta: Vec2::new(120.0, 0.0),
        phase: octara::WheelPhase::Discrete,
        mods: Modifiers::empty(),
    };
    assert!(mounted.driver.dispatch(mounted.id, Input::Interaction(wheel)).unwrap().handled);
    assert!(mounted.tree().scroll(row).unwrap().offset.x > 0.0);
    assert_eq!(mounted.tree().text(formula).unwrap().text.as_ref(), wide);
    assert_copied(&mut mounted, TextPosition { node: formula, offset: 0 }, TextPosition { node: formula, offset: wide.len() }, &wide);
}

#[test]
fn a_streaming_formula_copies_only_authored_bytes() {
    let mut driver = HeadlessDriver::default();
    let source = driver.app_mut().build(|b| b.text_buffer("Before $x")).unwrap();
    let mut mounted = Mounted::new(driver, move |b| markdown(b, source).test_id("markdown"));
    let node = formula_node(&mounted, "b0");
    let range = 7..9;
    assert_eq!(mounted.tree().text(node).unwrap().attachments[0].range, range);
    assert_copied(&mut mounted, TextPosition { node, offset: range.start }, TextPosition { node, offset: range.end }, "$x");
    mounted.update(|w| w.append_text(source, "$ after."));
    let range = 7..10;
    assert_eq!(mounted.tree().text(node).unwrap().attachments[0].range, range);
    assert_copied(&mut mounted, TextPosition { node, offset: range.start }, TextPosition { node, offset: range.end }, "$x$");
}

#[test]
fn progressive_display_math_preserves_trailing_source_whitespace() {
    let unfinished = "$$\nx + 1  \r\n";
    let mut driver = HeadlessDriver::default();
    let source = driver.app_mut().build(|b| b.text_buffer(unfinished)).unwrap();
    let mut mounted = Mounted::new(driver, move |b| markdown(b, source).test_id("markdown"));
    let node = formula_node(&mounted, "b0");
    let slot = mounted.tree().text(node).unwrap();
    assert_eq!(slot.text.as_ref(), unfinished);
    assert_eq!(slot.attachments[0].range, 0..unfinished.len());
    assert_copied(&mut mounted, TextPosition { node, offset: 0 }, TextPosition { node, offset: unfinished.len() }, unfinished);
}

struct CustomMath(Arc<Mutex<Vec<(String, bool)>>>);

impl MathRenderer for CustomMath {
    fn render(&self, tex: &str, style: MathRenderStyle) -> Result<MathLayout, MathError> {
        self.0.lock().unwrap().push((tex.to_owned(), style.display));
        BuiltinMathRenderer.render("q", style)
    }
}

#[test]
fn unchanged_display_math_does_not_typeset_on_an_unrelated_append() {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let log = calls.clone();
    let mut driver = HeadlessDriver::default();
    let initial = "> $$\n> x\n> $$\n\nTail.";
    let source = driver.app_mut().build(|b| b.text_buffer(initial)).unwrap();
    let mut mounted = Mounted::new(driver, move |b| {
        let config = MarkdownConfig::document().math_renderer(Arc::new(CustomMath(log)));
        markdown_with(b, source, config).test_id("markdown")
    });
    let node = formula_node(&mounted, "b0");
    assert_eq!(mounted.tree().text(node).unwrap().text.as_ref(), "$$\n> x\n> $$");
    assert_eq!(calls.lock().unwrap().len(), 1);
    mounted.update(|w| w.append_text(source, " More."));
    assert_eq!(calls.lock().unwrap().len(), 1);

    let quote = initial.find("> x").unwrap();
    mounted.update(|w| w.splice_text(source, TextBufferEdit::Splice { range: quote..quote + 3, insert: " >x".into() }));
    assert_eq!(formula_node(&mounted, "b0"), node);
    assert_eq!(mounted.tree().text(node).unwrap().text.as_ref(), "$$\n >x\n> $$");
    let calls = calls.lock().unwrap();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0], calls[1]);
}

#[test]
fn formula_renderer_overrides_are_scoped_and_errors_are_per_document() {
    let scoped = Arc::new(Mutex::new(Vec::new()));
    let overrides = Arc::new(Mutex::new(Vec::new()));
    let errors = Arc::new(Mutex::new(Vec::new()));
    let mut driver = HeadlessDriver::default();
    let source = driver.app_mut().build(|b| b.text_buffer(r"Before $\unknown$ then $\unknown$.")).unwrap();
    let (scope_log, override_log, error_log) = (scoped.clone(), overrides.clone(), errors.clone());
    let mut mounted = Mounted::new(driver, move |b| {
        let invalid = |b: &mut BuildCx<'_>, tag| {
            let log = error_log.clone();
            markdown_with(
                b,
                source,
                MarkdownConfig::document().on_math_error(move |tex, error| {
                    log.lock().unwrap().push((tex.to_owned(), error.to_string()));
                }),
            )
            .test_id(tag)
        };
        let a = invalid(b, "invalid-a");
        let b_view = invalid(b, "invalid-b");
        b.provide::<Arc<dyn MathRenderer>>(Arc::new(CustomMath(scope_log)));
        let default = markdown(b, r"$\scoped$");
        let over = markdown_with(b, r"$$\explicit$$", MarkdownConfig::document().math_renderer(Arc::new(CustomMath(override_log))));
        col().children([a, b_view, default, over])
    });
    assert_eq!(*scoped.lock().unwrap(), [(r"\scoped".to_owned(), false)]);
    assert_eq!(*overrides.lock().unwrap(), [(r"\explicit".to_owned(), true)]);
    assert_eq!(errors.lock().unwrap().len(), 2);
    mounted.update(|w| w.append_text(source, r" Another $\unknown$."));
    assert_eq!(errors.lock().unwrap().len(), 2);
    let node = descendant(&mounted, mounted.node("invalid-a"), |node| mounted.tree().text(node).is_some());
    let fallback = mounted.tree().text(node).unwrap();
    assert!(fallback.attachments.is_empty());
    assert!(fallback.runs.iter().all(|(_, span)| span.mono));
    assert!(fallback.text.contains(r"$\unknown$"));
}
