//! Every text-provider callback has the same permissions, including cached layouts and native
//! queries outside an App operation.

use octara::app::{AppConfig, Event};
use octara::kernel::{TextDocument, TextInputLayout};
use octara::platform::{HeadlessPlatform, HeadlessWindow};
use octara::prelude::*;
use octara::view::canvas;
use octara_primitives::path::point;
use octara_text::{LaidText, MonoEngine, ShapedRun, StyleRun, TextEngine, TextLayout, TextPresentation, TextStyle, VisualLine};
use std::any::Any;
use std::cell::{Cell, RefCell};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::rc::Rc;

type Callback = Rc<dyn Fn(&'static str)>;

#[derive(Clone, Default)]
struct Probe {
    callback: Rc<RefCell<Option<Callback>>>,
    entries: Rc<RefCell<Vec<&'static str>>>,
    reused: Rc<Cell<bool>>,
}

impl Probe {
    fn call(&self, entry: &'static str) {
        self.entries.borrow_mut().push(entry);
        let callback = self.callback.borrow().clone();
        if let Some(callback) = callback {
            callback(entry);
        }
    }

    fn set(&self, callback: impl Fn(&'static str) + 'static) {
        *self.callback.borrow_mut() = Some(Rc::new(callback));
    }

    fn clear(&self) {
        self.callback.borrow_mut().take();
    }
}

impl TextEngine for Probe {
    fn layout(&mut self, text: &str, style: &TextStyle, runs: &[StyleRun], previous: Option<&TextLayout>, clean: usize) -> TextLayout {
        self.call("layout");
        if let Some(previous) = previous {
            self.reused.set(previous.as_any().is::<ProbeLayout>());
        }
        Rc::new(ProbeLayout { layout: MonoEngine.layout(text, style, runs, previous, clean), probe: self.clone() })
    }

    fn ellipsis(&mut self, style: &TextStyle) -> Option<(Vec<ShapedRun>, f32)> {
        self.call("ellipsis");
        MonoEngine.ellipsis(style)
    }

    fn register_font(&mut self, _: &'static [u8]) {
        self.call("font");
    }

    fn register_fallback_font(&mut self, _: &'static [u8], _: &[[u8; 4]]) {
        self.call("fallback");
    }

    fn shaped(&self) -> u64 {
        self.call("shaped");
        7
    }
}

struct ProbeLayout {
    layout: TextLayout,
    probe: Probe,
}

impl LaidText for ProbeLayout {
    fn measure(&self, max_width: Option<f32>) -> (f32, u32, f32) {
        self.probe.call("measure");
        self.layout.measure(max_width)
    }

    fn present(
        &self,
        max_width: Option<f32>,
        max_lines: Option<u16>,
        fit: Option<f32>,
        ellipsis: Option<&(Vec<ShapedRun>, f32)>,
    ) -> TextPresentation {
        self.probe.call("present");
        self.layout.present(max_width, max_lines, fit, ellipsis)
    }

    fn visual_lines(&self, max_width: Option<f32>) -> Vec<VisualLine> {
        self.probe.call("visual_lines");
        self.layout.visual_lines(max_width)
    }

    fn as_any(&self) -> &dyn Any {
        self.probe.call("as_any");
        self
    }
}

fn app() -> (App, Probe) {
    let probe = Probe::default();
    let app = App::with_config(HeadlessPlatform::default(), AppConfig { text_engine: Box::new(probe.clone()), ..Default::default() });
    (app, probe)
}

fn input_layout(window: &Window) -> TextInputLayout {
    window.platform_window().as_any().downcast_ref::<HeadlessWindow>().unwrap().text_input.as_ref().unwrap().layout.clone().unwrap()
}

fn assert_denied(app: &App) {
    assert!(!app.is_shutdown() && !app.is_failed());
    assert_eq!(app.theme(), Theme::light());
    assert!(app.diagnostics().iter().any(|diagnostic| diagnostic.code == "WRITE_IN_DERIVE"));
    assert!(app.diagnostics().iter().all(|diagnostic| diagnostic.code == "WRITE_IN_DERIVE"), "{:?}", app.diagnostics());
}

#[test]
fn a_measurement_provider_cannot_change_the_model_after_bindings_were_sampled() {
    let (app, probe) = app();
    let value = app.run(|| state("value", || 0));
    let attempted = Rc::new(Cell::new(false));
    let called = attempted.clone();
    probe.set(move |entry| {
        if entry == "layout" && !called.replace(true) {
            value.set(1);
        }
    });
    let window = app.open_window(WindowOptions::new("Measurement"), move || text(move || value.get().to_string()).id("value")).unwrap();
    assert!(attempted.get());
    assert_eq!(app.run(|| value.get()), 0, "measurement cannot write application state");
    assert_eq!(window.kernel().text_of("value").as_deref(), Some("0"));
    assert_eq!(window.frames_presented(), 1, "no partial frame followed by a repair frame");
    assert_denied(&app);
    probe.clear();
    app.shutdown();
}

#[test]
fn measurement_paint_and_font_providers_cannot_write_or_reenter_resources() {
    let (app, probe) = app();
    let (protected, label) = app.run(|| (state("protected", || 0), state("label", || "Long caption".to_owned())));
    let owner = app.clone();
    probe.set(move |_| {
        protected.set(99);
        owner.set_theme(Theme::dark());
        owner.register_font(&[]);
        owner.shutdown();
    });
    let mut window = app
        .open_window(WindowOptions::new("Providers"), move || {
            col()
                .size_full()
                .child(text(label).id("label").w(40.0).truncate())
                .child(textarea().id("field").label("Field").value("one two three".to_owned()).rows(1, 4).w(40.0))
                .child(input().id("hint").label("Hint").placeholder("A long hint").w(40.0))
                .child(
                    canvas(|canvas| {
                        canvas.text("Canvas text", point(0.0, 0.0));
                    })
                    .size(40.0),
                )
        })
        .unwrap();
    window.display_list();
    window.dispatch(Event::Focus("field".into())).unwrap();
    assert_eq!(window.kernel().text_engine_shaped(), 7);
    app.run(|| label.set("Updated caption".to_owned()));
    app.register_font(&[]);
    app.register_fallback_font(&[], &[*b"Hani"]);
    assert_eq!(app.run(|| protected.get()), 0);
    assert!(probe.reused.get(), "previous layouts retain the provider's concrete downcast identity");
    for entry in ["layout", "measure", "present", "visual_lines", "ellipsis", "font", "fallback", "shaped", "as_any"] {
        assert!(probe.entries.borrow().contains(&entry), "provider entry {entry} was exercised");
    }
    assert_denied(&app);
    probe.clear();
    app.shutdown();
}

#[test]
fn native_multiline_queries_are_read_only_without_an_app_entry_and_restore_foreign_permissions() {
    let (app, probe) = app();
    let protected = app.run(|| state("protected", || 0));
    let mut window = app
        .open_window(WindowOptions::new("Native"), || textarea().id("field").label("Field").value("Initial".to_owned()).rows(1, 4).w(40.0))
        .unwrap();
    window.dispatch(Event::Focus("field".into())).unwrap();
    let layout = input_layout(&window);
    let owner = app.clone();
    probe.set(move |_| {
        protected.set(99);
        owner.set_theme(Theme::dark());
        owner.shutdown();
    });
    let document = TextDocument::new("one two three\nnext".into(), 18, 18);
    assert!(layout.layout(&document).is_some(), "native prediction does not need an ambient App");
    assert_eq!(app.run(|| protected.get()), 0);
    assert_denied(&app);

    let foreign = App::new(HeadlessPlatform::default());
    let other = foreign.run(|| state("other", || 0));
    foreign.run(|| {
        let layout = layout.clone();
        let derived = memo(move || {
            assert!(layout.layout(&document).is_some());
            other.set(1);
            7
        });
        assert_eq!(derived.get(), 7);
    });
    assert_eq!(foreign.run(|| other.get()), 0, "returning from a foreign query does not upgrade its caller's phase");
    assert!(foreign.diagnostics().iter().any(|diagnostic| diagnostic.code == "WRITE_IN_DERIVE"));
    probe.clear();
    app.shutdown();
    foreign.shutdown();
}

#[test]
fn provider_counter_reads_do_not_subscribe_the_calling_observer() {
    let (app, probe) = app();
    let source = app.run(|| state("source", || 0));
    let window = app.open_window(WindowOptions::new("Counter"), || text("Ready")).unwrap();
    probe.set(move |_| {
        source.get();
    });
    let calls = Rc::new(Cell::new(0));
    app.run(|| {
        let (window, calls) = (window.clone(), calls.clone());
        watch(
            move || {
                calls.set(calls.get() + 1);
                window.kernel().text_engine_shaped()
            },
            |_, _| {},
        );
    });
    assert_eq!(calls.get(), 1);
    app.run(|| source.set(1));
    assert_eq!(calls.get(), 1, "provider reads are samples, not observer dependencies");
    assert!(app.diagnostics().is_empty(), "{:?}", app.diagnostics());
    probe.clear();
    app.shutdown();
}

#[test]
fn provider_panics_fail_the_owner_before_publication_or_during_native_prediction() {
    for native in [false, true] {
        let (app, probe) = app();
        let value = app.run(|| state("value", || "Original".to_owned()));
        let mut window = app.open_window(WindowOptions::new("Failure"), move || input().id("field").label("Field").value(value)).unwrap();
        window.dispatch(Event::Focus("field".into())).unwrap();
        let layout = input_layout(&window);
        let frames = window.frames_presented();
        probe.set(|entry| {
            if entry == "layout" {
                panic!("provider fault");
            }
        });
        let result = catch_unwind(AssertUnwindSafe(|| {
            if native {
                layout.layout(&TextDocument::new("Native proposal".into(), 15, 15));
            } else {
                app.run(|| value.set("Changed".to_owned()));
            }
        }));
        assert!(result.is_err());
        assert!(app.is_failed());
        assert_eq!(window.frames_presented(), frames);
        for text in ["Later", ""] {
            assert!(
                layout.layout(&TextDocument::new(text.into(), text.len(), text.len())).is_none(),
                "native queries decline a failed provider owner"
            );
        }
        probe.clear();
        app.shutdown();
    }
}

#[test]
fn retained_native_layout_declines_after_shutdown_while_its_app_handle_is_alive() {
    let (app, probe) = app();
    let mut window =
        app.open_window(WindowOptions::new("Retired"), || input().id("field").label("Field").value("Original".to_owned())).unwrap();
    window.dispatch(Event::Focus("field".into())).unwrap();
    let layout = input_layout(&window);
    app.shutdown();
    let calls = probe.entries.borrow().len();
    for text in ["Later", ""] {
        assert!(layout.layout(&TextDocument::new(text.into(), text.len(), text.len())).is_none());
    }
    assert_eq!(probe.entries.borrow().len(), calls, "shutdown revokes native provider admission before releasing resources");
    assert!(app.is_shutdown() && window.is_closed());
    assert!(!app.is_failed(), "successful shutdown must not fail its App");
    assert!(app.pending().is_empty());
    app.shutdown();
    assert!(!app.is_failed());
}

#[test]
fn retained_native_layout_declines_after_its_owner_drops_without_retaining_a_runtime_cycle() {
    let (app, probe) = app();
    let mut window =
        app.open_window(WindowOptions::new("Retained"), || input().id("field").label("Field").value("Original".to_owned())).unwrap();
    window.dispatch(Event::Focus("field".into())).unwrap();
    let layout = input_layout(&window);
    drop(window);
    drop(app);
    let calls = probe.entries.borrow().len();
    for text in ["Later", ""] {
        assert!(layout.layout(&TextDocument::new(text.into(), text.len(), text.len())).is_none());
    }
    assert_eq!(probe.entries.borrow().len(), calls, "a dead owner does not invoke its retained provider");
}
