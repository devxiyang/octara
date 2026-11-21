//! App-owned resource writes: admission, mutation, invalidation and the outer frame boundary.

use super::*;
use crate::reactive::graph::{ExecutionOwner, Operation};
use octara_text::{LaidText, ShapedRun, StyleRun, TextEngine, TextLayout, TextPresentation, TextStyle};
use std::any::Any;
use std::panic::Location;
use std::rc::Weak;

/// Provider permissions travel with the text resources, including native snapshots queried
/// without an App entry. A weak reference avoids a cycle through window-owned bindings.
#[derive(Clone)]
struct Provider {
    runtime: Weak<Runtime>,
}

impl Provider {
    #[track_caller]
    fn run<R>(&self, f: impl FnOnce() -> R) -> R {
        let runtime = self.runtime.upgrade().expect("text provider belongs to a dropped App");
        runtime.assert_healthy();
        let at = Location::caller();
        let _failure = std::mem::DropGuard::new(&*runtime, move |runtime| {
            if std::thread::panicking() {
                runtime.fail("panic escaped a text provider call; shut down this App and create a new one", at);
            }
        });
        let value =
            runtime.enter(|| runtime.with_execution(ExecutionOwner { scope: None, window: None }, Ctx::Observe, None, || untrack(f)));
        runtime.assert_healthy();
        value
    }
}

pub(super) fn text_resources(
    theme: Theme,
    engine: Box<dyn TextEngine>,
    touch: bool,
    runtime: &Rc<Runtime>,
) -> Rc<crate::kernel::Resources> {
    let provider = Provider { runtime: Rc::downgrade(runtime) };
    let available = provider.runtime.clone();
    let failed = runtime.failure_flag();
    let engine = Box::new(GuardedTextEngine { engine, provider });
    let mut resources = crate::kernel::Resources::new(theme, engine, touch);
    resources.text.set_available(move || !failed.get() && available.upgrade().is_some());
    Rc::new(resources)
}

struct GuardedTextEngine {
    engine: Box<dyn TextEngine>,
    provider: Provider,
}

impl TextEngine for GuardedTextEngine {
    fn layout(&mut self, text: &str, style: &TextStyle, runs: &[StyleRun], previous: Option<&TextLayout>, clean: usize) -> TextLayout {
        let layout = self.provider.run(|| self.engine.layout(text, style, runs, previous, clean));
        Rc::new(GuardedLayout { layout, provider: self.provider.clone() })
    }

    fn ellipsis(&mut self, style: &TextStyle) -> Option<(Vec<ShapedRun>, f32)> {
        self.provider.run(|| self.engine.ellipsis(style))
    }

    fn register_font(&mut self, data: &'static [u8]) {
        self.provider.run(|| self.engine.register_font(data));
    }

    fn register_fallback_font(&mut self, data: &'static [u8], scripts: &[[u8; 4]]) {
        self.provider.run(|| self.engine.register_fallback_font(data, scripts));
    }

    fn shaped(&self) -> u64 {
        self.provider.run(|| self.engine.shaped())
    }

    fn kept_bytes(&self) -> usize {
        self.provider.run(|| self.engine.kept_bytes())
    }
}

/// Text layouts call back into the provider after shaping, during measurement, painting and
/// native document queries. Keep the same boundary and preserve provider cache downcasts.
struct GuardedLayout {
    layout: TextLayout,
    provider: Provider,
}

impl LaidText for GuardedLayout {
    fn measure(&self, max_width: Option<f32>) -> (f32, u32, f32) {
        self.provider.run(|| self.layout.measure(max_width))
    }

    fn present(
        &self,
        max_width: Option<f32>,
        max_lines: Option<u16>,
        fit: Option<f32>,
        ellipsis: Option<&(Vec<ShapedRun>, f32)>,
    ) -> TextPresentation {
        self.provider.run(|| self.layout.present(max_width, max_lines, fit, ellipsis))
    }

    fn as_any(&self) -> &dyn Any {
        self.provider.run(|| self.layout.as_any())
    }
}

enum Change<'a> {
    Theme(Box<Theme>),
    Font { data: &'static [u8], scripts: Option<&'a [[u8; 4]]> },
    Platform(TargetPlatform),
    ReducedMotion(bool),
}

impl App {
    /// Changes keyboard/touch conventions for every window, including their touch styles.
    #[track_caller]
    pub fn set_target_platform(&self, platform: TargetPlatform) {
        self.change_resources(Change::Platform(platform), Location::caller());
    }

    /// Switches every window's theme. Rejected during construction and pure computations;
    /// provide the initial theme through AppConfig instead.
    #[track_caller]
    pub fn set_theme(&self, theme: Theme) {
        self.change_resources(Change::Theme(Box::new(theme)), Location::caller());
    }

    /// Adds a font for every window and invalidates their text measurements.
    /// Custom engine registration runs without dependency collection or UI write permission.
    #[track_caller]
    pub fn register_font(&self, data: &'static [u8]) {
        self.change_resources(Change::Font { data, scripts: None }, Location::caller());
    }

    /// Adds a fallback font for ISO 15924 script tags, for example `*b"Hani"`.
    #[track_caller]
    pub fn register_fallback_font(&self, data: &'static [u8], scripts: &[[u8; 4]]) {
        self.change_resources(Change::Font { data, scripts: Some(scripts) }, Location::caller());
    }

    /// Changes the shared Reduce Motion preference, including before any window is open.
    /// Spatial motion created afterwards lands immediately; colors and opacity fade briefly.
    #[track_caller]
    pub fn set_reduced_motion(&self, on: bool) {
        self.change_resources(Change::ReducedMotion(on), Location::caller());
    }

    fn change_resources(&self, change: Change<'_>, at: &'static Location<'static>) {
        // Check the source App before restoring the destination, so cross-App calls cannot
        // turn an observation or a failed caller into an authorized resource write.
        if !Runtime::caller_allows(Operation::Write, at) {
            return;
        }
        if self.app.shutdown.get() {
            self.app.rt.diag(&codes::WRITE_DISPOSED, "cannot change resources of a shut down App".into(), at);
            return;
        }
        self.enter(|| {
            if !self.app.rt.allows(Operation::Write, at) {
                return;
            }
            let res = &self.app.res;
            match change {
                Change::Theme(theme) => {
                    if *theme == *res.theme.borrow() {
                        return;
                    }
                    *res.theme.borrow_mut() = *theme;
                    for w in self.each() {
                        w.w.kernel.borrow_mut().theme_changed();
                        let bp = res.theme.borrow().breakpoint_of(w.w.size.get().0);
                        if untrack(|| w.w.breakpoint.get()) != bp {
                            w.w.breakpoint.set_internal(bp);
                        }
                    }
                }
                Change::Font { data, scripts } => {
                    // The engine is temporarily taken out, not held behind a RefCell borrow
                    // across provider code. Native queries return None while it is unavailable.
                    res.text.with_detached(|engine| match scripts {
                        Some(scripts) => engine.register_fallback_font(data, scripts),
                        None => engine.register_font(data),
                    });
                    self.app.rt.assert_healthy();
                    if self.app.shutdown.get() {
                        return;
                    }
                    for w in self.each() {
                        w.w.kernel.borrow_mut().fonts_changed();
                    }
                }
                Change::Platform(platform) => {
                    if self.app.platform.replace(platform) == platform {
                        return;
                    }
                    res.touch.set(platform.is_touch());
                    for w in self.each() {
                        w.w.kernel.borrow_mut().touch_changed();
                    }
                }
                Change::ReducedMotion(on) => {
                    if res.reduced_motion.replace(on) == on {
                        return;
                    }
                    if on {
                        for w in self.each() {
                            w.w.kernel.borrow_mut().land_transition_motion();
                        }
                    }
                }
            }
            // A nested handler/watch/cleanup belongs to its enclosing update. In particular,
            // do not flush bindings inside a provider callback or a running watch callback.
            if self.app.depth.get() == 1 {
                self.app.frame(None, at);
            }
        });
    }
}
