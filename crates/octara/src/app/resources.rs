//! App-owned resource writes: admission, mutation, invalidation and the outer frame boundary.

use super::*;
use crate::reactive::graph::Operation;
use std::panic::Location;

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
                    res.text.with_detached(|engine| {
                        self.app.rt.with_frame(Ctx::Observe, None, None, || {
                            untrack(|| match scripts {
                                Some(scripts) => engine.register_fallback_font(data, scripts),
                                None => engine.register_font(data),
                            })
                        });
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
                    self.app.keymap_changed();
                    for w in self.each() {
                        w.w.kernel.borrow_mut().touch_changed();
                    }
                }
                Change::ReducedMotion(on) => {
                    if res.reduced_motion.replace(on) == on {
                        return;
                    }
                }
            }
            // A nested handler/watch/cleanup belongs to its enclosing update. In particular,
            // do not flush bindings inside a provider callback or a running watch callback.
            if self.app.depth.get() == 1 {
                self.app.frame(at);
            }
        });
    }
}
