//! Device-free performance gates for settled native frames driven by the virtual clock.

use octara::model::paint::FontId;
use octara::{EditorEvent, EditorSession, EditorSessionId, Input, NodeId, PaintItem, Tree, WindowRuntime, prelude::*};
use octara_markdown::{StreamingEffects, StreamingOptions, markdown_stream};
use octara_pipeline::{NativeEngine, NativeEngineConfig, NativePublication};
use octara_render::plan::{TargetInfo, TextFramePlanner};
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::ops::Range;
use std::time::Duration;

const EXTENT: [u32; 2] = [480, 580];
const ROWS: usize = 10_000;
const ROW_HEIGHT: u16 = 20;
const HZ: u64 = 120;

struct CountingAllocator;

thread_local! {
    // Count only allocations on this thread during frame planning and scheduling.
    static ALLOCATIONS: Cell<Option<u64>> = const { Cell::new(None) };
}

fn count_allocation(ptr: *mut u8) -> *mut u8 {
    if !ptr.is_null() {
        let _ = ALLOCATIONS.try_with(|count| count.set(count.get().map(|n| n.saturating_add(1))));
    }
    ptr
}

// SAFETY: Each operation forwards the original arguments to the system allocator.
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        count_allocation(unsafe { System.alloc(layout) })
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        count_allocation(unsafe { System.alloc_zeroed(layout) })
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        count_allocation(unsafe { System.realloc(ptr, layout, size) })
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) };
    }
}

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

fn count_heap_allocs<R>(run: impl FnOnce() -> R) -> (R, u64) {
    struct Reset;

    impl Drop for Reset {
        fn drop(&mut self) {
            ALLOCATIONS.set(None);
        }
    }

    ALLOCATIONS.set(Some(0));
    let reset = Reset;
    let value = run();
    let allocations = ALLOCATIONS.get().unwrap();
    drop(reset);
    (value, allocations)
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Work {
    draw_calls: usize,
    uploaded_bytes: u64,
    shapes_built: u64,
    heap_allocs: u64,
}

#[derive(Debug, PartialEq)]
struct Sample {
    time: Duration,
    frame: u64,
    work: Work,
    drawing: Option<Vec<PaintItem>>,
}

struct Simulation {
    app: App,
    window: WindowId,
    publication: NativePublication,
    planner: TextFramePlanner,
    frame: u64,
    shapes_built: u64,
    trace: Vec<Sample>,
}

fn app(seed: u64, cjk: bool) -> (App, NativePublication) {
    let publication = NativePublication::default();
    let output = publication.clone();
    let app = App::with_namespace([0x6f63_7461_7261, seed], move |_, _| {
        let mut engine = NativeEngine::with_publication(NativeEngineConfig::default(), output.clone());
        if cjk {
            engine.register_font_data(octara_fonts::cjk::REGULAR.into(), &octara_fonts::cjk::SCRIPTS);
        }
        Box::new(engine)
    });
    (app, publication)
}

impl Simulation {
    fn mount(mut app: App, publication: NativePublication, view: impl FnOnce(&mut BuildCx<'_>) -> El + 'static) -> Self {
        let viewport =
            WindowViewport { size: Size::new(EXTENT[0] as f32, EXTENT[1] as f32), scale: 1.0, safe_area: None, keyboard_inset: 0.0 };
        let options = WindowOptions { viewport, ..WindowOptions::default() };
        let window = app.open_window(options, view).unwrap();
        let planner = TextFramePlanner::new(TargetInfo {
            extent: EXTENT,
            scale: 1.0,
            max_texture_dimension: 2048,
            copyable: true,
            encode_output: false,
        });
        let mut sim = Self { app, window, publication, planner, frame: 0, shapes_built: 0, trace: Vec::new() };
        sim.sample();
        sim
    }

    fn window(&self) -> &WindowRuntime {
        self.app.runtime().window(self.window).unwrap()
    }

    fn tree(&self) -> &Tree {
        self.window().tree()
    }

    fn node(&self, name: &str) -> NodeId {
        let [node] = self.tree().test_ids(name) else { panic!("expected one node named {name}") };
        *node
    }

    fn advance(&mut self, tick: u64) {
        let time = Duration::from_nanos(tick * 1_000_000_000 / HZ);
        self.app.advance_to(time).unwrap();
        assert_eq!(self.app.runtime().now(), time);
        self.sample();
    }

    fn update(&mut self, update: impl FnOnce(&mut WriteCx<'_>)) {
        self.app.update_window(self.window, update).unwrap();
        self.sample();
    }

    fn editor_event(&mut self, event: impl FnOnce(EditorSessionId, u64) -> EditorEvent) {
        let session = EditorSession::active(self.tree()).unwrap();
        let event = event(session.id(), session.editor().document_rev());
        let receipt = self.app.dispatch(self.window, Input::Editor(event)).unwrap();
        assert!(receipt.handled);
        self.sample();
    }

    fn sample(&mut self) {
        let frame = self.publication.latest().unwrap();
        let time = self.app.runtime().now();
        let mut work = Work::default();
        let drawing = if frame.number != self.frame {
            assert_eq!(frame.number, self.frame + 1);
            assert_eq!(frame.sampled_at, time);
            let (metrics, heap_allocs) = count_heap_allocs(|| self.planner.measure(&frame.draw));
            let metrics = metrics.expect("stress scene must fit the device-free frame planner");
            work.draw_calls = metrics.draw_calls;
            work.uploaded_bytes = metrics.uploaded_bytes;
            work.shapes_built = frame.text_work.shapes_built - self.shapes_built;
            work.heap_allocs = heap_allocs;
            self.frame = frame.number;
            self.shapes_built = frame.text_work.shapes_built;

            let mut items = frame.draw.items();
            for item in &mut items {
                if let PaintItem::Glyphs(run) = item {
                    // Font IDs are allocated process-wide. The bytes, face index and output must still match.
                    run.font.id = FontId(0);
                }
            }
            Some(items)
        } else {
            None
        };
        self.trace.push(Sample { time, frame: self.frame, work, drawing });
    }

    fn finish(mut self) -> Vec<Sample> {
        self.tree().validate_integrity().unwrap();
        self.app.close_window(self.window).unwrap();
        self.app.shutdown();
        let runtime = self.app.runtime_mut();
        assert_eq!(runtime.world().live_scopes(), 0);
        assert_eq!(runtime.world().live_nodes(), 0);
        assert!(runtime.take_diagnostics().is_empty());
        self.trace
    }
}

fn pick(seed: &mut u64, limit: usize) -> usize {
    *seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
    ((*seed >> 32) as usize) % limit
}

fn virtual_scroll(seed: u64) -> Vec<Sample> {
    let (mut app, publication) = app(seed, false);
    let rows = app.build(|b| b.list::<usize, u16>()).unwrap();
    app.update(|w| {
        for key in 0..ROWS {
            w.push_row(rows, key, ROW_HEIGHT);
        }
    })
    .unwrap();
    let mut sim = Simulation::mount(app, publication, move |_| {
        col().test_id("viewport").h(180).w(480).scroll_y().scrollbar(ScrollbarVisibility::Hidden).child(virtual_each(
            rows,
            VirtualOptions { estimated_row_height: f32::from(ROW_HEIGHT), overscan: 3, maintain_scroll_anchor: true },
            |_, &key, row| {
                el().test_id(format!("row-{key}"))
                    .style_with(move |r, style| style.h(i32::from(r.try_get(row).unwrap())))
                    .bg(if key.is_multiple_of(2) { Color::WHITE } else { rgb(0xe6e6e6) })
                    .child(text(format!("Message {key:04}")).w(160).text_color(Color::BLACK))
            },
        ))
    });
    let port = sim.node("viewport");
    let top = |sim: &Simulation| sim.window().hit_test(Point::new(400.0, 1.0)).unwrap().hit.node().unwrap();
    let offset = |sim: &Simulation| sim.tree().scroll(port).unwrap().offset.y;
    let mut rng = seed;
    let mut key = 0;
    for step in 0..64 {
        sim.advance(2 * step + 1);
        // Mix distant seeks and nearby scrolls so both the viewport and surviving rows get exercised.
        key = if step.is_multiple_of(4) { 16 + pick(&mut rng, ROWS - 128) } else { key + 1 };
        let scroll = (key as f32) * f32::from(ROW_HEIGHT) + 5.0;
        sim.update(|w| w.scroll_to(port, Vec2::new(0.0, scroll)));
        assert_eq!(offset(&sim), scroll);
        let visible = top(&sim);
        let name = sim.tree().provenance(visible).unwrap().test_id.as_deref().unwrap();
        let resized = name.strip_prefix("row-").unwrap().parse::<usize>().unwrap();
        let following = sim.node(&format!("row-{}", resized + 1));
        let following_y = sim.window().geometry(following).unwrap().rect.y;
        let height = sim.window().geometry(visible).unwrap().rect.h;
        let increase = 1 + pick(&mut rng, 12) as u16;

        sim.advance(2 * step + 2);
        sim.update(|w| w.update_row(rows, &resized, |height| *height += increase));
        assert_eq!(sim.window().geometry(visible).unwrap().rect.h, height + f32::from(increase));
        assert_eq!(sim.window().geometry(following).unwrap().rect.y, following_y + f32::from(increase));
        assert_eq!(offset(&sim), scroll);
        assert_eq!(top(&sim), visible);
        assert!(sim.tree().subtree(port).count() <= 40, "only the viewport and its overscan should be mounted");
    }
    sim.finish()
}

fn markdown_streaming(seed: u64) -> Vec<Sample> {
    const TOKENS: &[&str] = &["an ", "**active** ", "`tail` ", "with ", "UTF-8 ", "café ", "*styled* ", "changes ", "every ", "frame "];
    const HEADING: &str = "# Stream";
    const END: &str = "End of Markdown.";

    let (mut app, publication) = app(seed, false);
    let (source, finished) = app.build(|b| (b.text_buffer(format!("{HEADING}\n\n")), b.signal(false))).unwrap();
    let mut sim = Simulation::mount(app, publication, move |b| {
        markdown_stream(
            b,
            source,
            finished.read_only(),
            StreamingOptions { effects: Some(StreamingEffects::Fade { duration_secs: 0.1 }), pace: false, ..StreamingOptions::default() },
        )
        .test_id("document")
    });
    let document = sim.node("document");
    let heading = sim.tree().children(document)[0];
    let mut rng = seed;
    let mut sampled_reveal_frames = 0;
    for tick in 1..=HZ {
        sim.advance(tick);
        let previous = sim.frame;
        let word = if tick == HZ { END } else { TOKENS[pick(&mut rng, TOKENS.len())] };
        sim.update(|w| {
            if tick % 20 == 1 || tick == HZ {
                w.append_text(source, "\n\n");
            }
            w.append_text(source, word);
        });
        assert!(sim.frame > previous);
        assert_eq!(sim.tree().children(document)[0], heading);
        if sim
            .trace
            .last()
            .unwrap()
            .drawing
            .as_ref()
            .unwrap()
            .iter()
            .any(|item| matches!(item, PaintItem::Glyphs(run) if run.colors.is_some()))
        {
            sampled_reveal_frames += 1;
        }
    }
    assert!(sampled_reveal_frames >= 100, "the stream must exercise paint-time reveal on its committed frames");
    let tail = sim.tree().children(document).iter().copied().rfind(|&node| sim.tree().text(node).is_some()).unwrap();
    assert_eq!(sim.tree().text(tail).unwrap().text.as_ref(), END);
    let reveal = sim.tree().text_reveal(tail).unwrap();
    assert_eq!(reveal.revealed_bytes, END.len());
    assert!(!reveal.finished);
    sim.advance(HZ + 3);
    let animation = sim.trace.last().unwrap();
    assert_eq!(animation.work.shapes_built, 0, "clock-only reveal must reuse existing shaping");
    assert!(animation.drawing.as_ref().unwrap().iter().any(|item| {
        matches!(item, PaintItem::Glyphs(run) if run.source == tail && run.colors.as_ref().is_some_and(|colors| {
            colors.iter().any(|color| (0.0..1.0).contains(&color.components[3]) && color.components[3] > 0.0)
        }))
    }));
    sim.update(|w| w.set(finished, true));
    sim.advance(HZ + 24);
    assert_eq!(sim.tree().text(tail).unwrap().text.as_ref(), END);
    assert!(sim.tree().text_reveal(tail).is_none_or(|reveal| reveal.finished));
    assert!(sim.publication.latest().unwrap().draw.glyph_runs().all(|run| run.colors.is_none()));
    sim.finish()
}

#[derive(Debug, PartialEq, Eq)]
struct EditState {
    session: EditorSessionId,
    revision: u64,
    value: String,
    selection: (usize, usize),
    preedit: Option<Preedit>,
}

#[derive(Debug, PartialEq, Eq)]
struct Preedit {
    text: String,
    marked: Range<usize>,
    selection: Option<(usize, usize)>,
}

fn editor_state(sim: &Simulation) -> EditState {
    let session = EditorSession::active(sim.tree()).expect("the keyed editor should remain focused");
    let editor = session.editor();
    let preedit = editor.composition().map(|preedit| Preedit {
        text: preedit.text().to_owned(),
        marked: preedit.marked(),
        selection: preedit.selection(),
    });
    EditState {
        session: session.id(),
        revision: editor.document_rev(),
        value: editor.value().to_owned(),
        selection: editor.selection(),
        preedit,
    }
}

fn ime_reorder(seed: u64) -> Vec<Sample> {
    const EDITED_KEY: usize = 5;
    const FILES: usize = 12;
    const PHRASES: [(&str, &str); 3] = [("日ほ", "日本語"), ("文じ", "文書"), ("新ち", "新しい名前")];

    let (mut app, publication) = app(seed, true);
    let rows = app.build(|b| b.list::<usize, String>()).unwrap();
    app.update(|w| {
        for key in 0..FILES {
            w.push_row(rows, key, format!("File {key:02}"));
        }
    })
    .unwrap();
    let mut sim = Simulation::mount(app, publication, move |_| {
        col().test_id("files").child(each(rows, move |_, &key, row| {
            col().h(30).child(
                input()
                    .test_id(format!("file-{key}"))
                    .h(30)
                    .value(bind(move |r| r.try_get(row).unwrap()))
                    .on_input(move |w, value| w.update_row(rows, &key, |name| *name = value.into())),
            )
        }))
    });
    let list = sim.node("files");
    let editor = sim.node("file-5");
    sim.update(|w| w.focus("file-5"));
    let original = editor_state(&sim).session;
    let mut rank = EDITED_KEY;
    let mut rng = seed;
    let mut before = "File 05";
    for step in 0..24 {
        sim.advance(3 * step + 1);
        sim.editor_event(|session, expected_rev| EditorEvent::ProposeSelection { session, expected_rev, selection: (before.len(), 0) });
        let (preedit, committed) = PHRASES[pick(&mut rng, PHRASES.len())];
        sim.editor_event(|session, expected_rev| EditorEvent::Composition {
            session,
            expected_rev,
            text: preedit.into(),
            selection: Some((6, 3)),
            replacement: None,
        });
        let composed = editor_state(&sim);
        assert_eq!(composed.session, original);
        assert_eq!(composed.value, before);
        assert_eq!(composed.selection, (before.len(), 0));
        assert_eq!(composed.preedit, Some(Preedit { text: preedit.into(), marked: 0..preedit.len(), selection: Some((6, 3)) }));
        assert!(sim.publication.latest().unwrap().draw.glyph_runs().any(|run| {
            run.source == editor && run.font.data.as_ref() == octara_fonts::cjk::REGULAR && run.glyphs.iter().all(|glyph| glyph.id != 0)
        }));

        sim.advance(3 * step + 2);
        rank = (rank + 1 + pick(&mut rng, FILES - 1)) % FILES;
        sim.update(|w| w.move_row(rows, &EDITED_KEY, rank));
        assert_eq!(sim.tree().children(sim.tree().children(list)[rank]), &[editor]);
        assert_eq!(sim.tree().focus(), Some(editor));
        assert_eq!(editor_state(&sim), composed);

        sim.advance(3 * step + 3);
        sim.editor_event(|session, expected_rev| EditorEvent::Commit { session, expected_rev, text: committed.into() });
        let state = editor_state(&sim);
        assert_eq!(state.session, original);
        assert_eq!(state.value, committed);
        assert_eq!(state.selection, (committed.len(), committed.len()));
        assert!(state.preedit.is_none());
        before = committed;
    }
    sim.finish()
}

fn check_budget(name: &str, seed: u64, budget: Work, run: impl Fn(u64) -> Vec<Sample>) {
    let first = run(seed);
    let second = run(seed);
    assert_eq!(first.len(), second.len(), "{name}, seed {seed:#x}");
    let mut peak = Work::default();
    for (index, (a, b)) in first.iter().zip(&second).enumerate() {
        assert_eq!((a.time, a.frame, a.work), (b.time, b.frame, b.work), "{name}, seed {seed:#x}, sample {index}");
        assert!(a.drawing == b.drawing, "{name}, seed {seed:#x}, sample {index} produced different display lists");
        peak.draw_calls = peak.draw_calls.max(a.work.draw_calls);
        peak.uploaded_bytes = peak.uploaded_bytes.max(a.work.uploaded_bytes);
        peak.shapes_built = peak.shapes_built.max(a.work.shapes_built);
        peak.heap_allocs = peak.heap_allocs.max(a.work.heap_allocs);
    }
    eprintln!("{name}, seed {seed:#x}\n  peak {peak:?}\n  budget {budget:?}");
    assert!((1..=budget.draw_calls).contains(&peak.draw_calls), "{name}: draw_calls {} exceeds {}", peak.draw_calls, budget.draw_calls);
    assert!(
        (1..=budget.uploaded_bytes).contains(&peak.uploaded_bytes),
        "{name}: uploaded_bytes {} exceeds {}",
        peak.uploaded_bytes,
        budget.uploaded_bytes
    );
    assert!(peak.shapes_built <= budget.shapes_built, "{name}: shapes_built {} exceeds {}", peak.shapes_built, budget.shapes_built);
    assert!(peak.heap_allocs <= budget.heap_allocs, "{name}: heap_allocs {} exceeds {}", peak.heap_allocs, budget.heap_allocs);
}

#[test]
fn virtual_scroll_10k_budget() {
    check_budget(
        "10,000 variable-height rows",
        0x09d3,
        Work { draw_calls: 3, uploaded_bytes: 14_000, shapes_built: 44, heap_allocs: 225 },
        virtual_scroll,
    );
}

#[test]
fn markdown_streaming_120hz_budget() {
    check_budget(
        "120 Hz Markdown with TextReveal",
        0x6942,
        Work { draw_calls: 4, uploaded_bytes: 34_000, shapes_built: 2, heap_allocs: 560 },
        markdown_streaming,
    );
}

#[test]
fn cjk_ime_reorder_budget() {
    check_budget(
        "CJK IME during keyed reorder",
        0x822c,
        Work { draw_calls: 4, uploaded_bytes: 11_200, shapes_built: 16, heap_allocs: 215 },
        ime_reorder,
    );
}
