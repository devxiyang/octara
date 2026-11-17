//! Window-owned layout feedback. Views register work and read projections; the frame loop
//! executes admitted hooks, samples geometry, then publishes it without borrowing the tree.

use super::*;
use crate::reactive::graph::{ReadSignal, rt};

pub(super) type AfterLayout = Rc<dyn Fn() -> bool>;

#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub(crate) struct ScrollState {
    pub offset: (f32, f32),
    pub client: (f32, f32),
}

impl WindowState {
    pub(crate) fn scroll_state(&self, node: NodeId) -> Signal<ScrollState> {
        *self.scrolls.borrow_mut().entry(node).or_insert_with(|| Signal::detached(ScrollState::default()))
    }

    pub(crate) fn size_signal(&self, id: &str) -> ReadSignal<(f32, f32)> {
        let mut sizes = self.sizes.borrow_mut();
        if let Some(signal) = sizes.get(id) {
            return signal.read_only();
        }
        let size = {
            let k = self.kernel.borrow();
            k.find(id).map_or((0.0, 0.0), |n| {
                let r = k.rect(n);
                (r.2, r.3)
            })
        };
        let signal = Signal::detached(size);
        sizes.insert(id.to_owned(), signal);
        signal.read_only()
    }

    pub(crate) fn tail_signal(&self, id: &str) -> ReadSignal<bool> {
        let mut tails = self.tails.borrow_mut();
        if let Some(signal) = tails.get(id) {
            return signal.read_only();
        }
        let following = {
            let k = self.kernel.borrow();
            k.find(id).is_some_and(|n| k.is_following_tail(n))
        };
        let signal = Signal::detached(following);
        tails.insert(id.to_owned(), signal);
        signal.read_only()
    }

    /// Registered work belongs to the current execution owner and is removed with that scope.
    pub(crate) fn after_layout(self: &Rc<Self>, f: AfterLayout) -> u64 {
        let id = self.next.get() + 1;
        self.next.set(id);
        let owner = rt(|rt| rt.capture_owner());
        let hook = Rc::new(move || rt(|rt| rt.admits(owner) && rt.with_execution(owner, Ctx::Handler, None, || f())));
        self.after_layout.borrow_mut().push((id, hook));
        let window = Rc::downgrade(self);
        rt(|rt| {
            rt.on_cleanup(Box::new(move || {
                if let Some(window) = window.upgrade() {
                    window.remove_after_layout(id);
                }
            }))
        });
        id
    }

    pub(crate) fn remove_after_layout(&self, id: u64) {
        let removed = {
            let mut hooks = self.after_layout.borrow_mut();
            hooks.iter().position(|(key, _)| *key == id).map(|index| hooks.remove(index))
        };
        drop(removed);
    }

    /// Take registries before releasing callbacks or reactive nodes; teardown can reenter.
    pub(super) fn release_feedback(&self) {
        let hooks = std::mem::take(&mut *self.after_layout.borrow_mut());
        let sizes = std::mem::take(&mut *self.sizes.borrow_mut());
        let tails = std::mem::take(&mut *self.tails.borrow_mut());
        let scrolls = std::mem::take(&mut *self.scrolls.borrow_mut());
        for signal in sizes.into_values() {
            signal.dispose();
        }
        for signal in tails.into_values() {
            signal.dispose();
        }
        for signal in scrolls.into_values() {
            signal.dispose();
        }
        drop(hooks);
    }
}

impl Window {
    pub(super) fn feedback_active(&self) -> bool {
        self.app.assert_usable();
        !self.app.shutdown.get() && !self.w.closed.get()
    }

    pub(super) fn sync_after_layout(&self) -> bool {
        if !self.feedback_active() {
            return false;
        }
        let hooks = self.w.after_layout.borrow().clone();
        let mut changed = false;
        for (id, hook) in hooks {
            // An earlier callback may remove a later callback in this detached batch.
            if self.w.after_layout.borrow().iter().any(|(live, _)| *live == id) {
                changed |= hook();
            }
            if !self.feedback_active() {
                return changed;
            }
        }
        // Read one layout result, releasing the kernel and registries before signal writes.
        let (sizes, tails, scrolls, dead) = {
            let k = self.w.kernel.borrow();
            let sizes: Vec<_> = self
                .w
                .sizes
                .borrow()
                .iter()
                .map(|(id, sig)| {
                    let size = k.find(id).map_or((0.0, 0.0), |n| {
                        let r = k.rect(n);
                        (r.2, r.3)
                    });
                    (*sig, size)
                })
                .collect();
            let tails: Vec<_> =
                self.w.tails.borrow().iter().map(|(id, sig)| (*sig, k.find(id).is_some_and(|n| k.is_following_tail(n)))).collect();
            let (mut scrolls, mut dead) = (Vec::new(), Vec::new());
            for (&node, sig) in self.w.scrolls.borrow().iter() {
                if k.is_alive(node) {
                    scrolls.push((*sig, ScrollState { offset: k.scroll_offset(node), client: k.client_size(node) }));
                } else {
                    dead.push(node);
                }
            }
            (sizes, tails, scrolls, dead)
        };
        for (signal, value) in sizes {
            changed |= publish(signal, value);
        }
        for (signal, value) in tails {
            changed |= publish(signal, value);
        }
        for (signal, value) in scrolls {
            changed |= publish(signal, value);
        }
        for node in dead {
            let signal = self.w.scrolls.borrow_mut().remove(&node);
            if let Some(signal) = signal {
                signal.dispose();
            }
        }
        changed
    }
}

fn publish<T: Copy + PartialEq + 'static>(signal: Signal<T>, value: T) -> bool {
    if untrack(|| signal.get()) == value {
        return false;
    }
    signal.set_internal(value);
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::HeadlessPlatform;
    use crate::prelude::*;

    #[test]
    fn after_layout_does_not_run_a_hook_removed_by_an_earlier_hook() {
        let app = App::new(HeadlessPlatform::default());
        let window = app.open_window(Default::default(), || el().size_full().into()).unwrap();
        let called = Rc::new(Cell::new(false));
        let later = Rc::new(Cell::new(0));
        let window_state = window.enter(crate::app::window);
        let first = app.run(|| {
            let (state, next) = (window_state.clone(), later.clone());
            let first = window_state.after_layout(Rc::new(move || {
                state.remove_after_layout(next.get());
                false
            }));
            let out = called.clone();
            later.set(window_state.after_layout(Rc::new(move || {
                out.set(true);
                false
            })));
            first
        });
        assert!(!called.get());
        app.run(|| window_state.remove_after_layout(first));
        app.shutdown();
    }

    #[test]
    fn feedback_hooks_are_removed_with_their_component_scope() {
        let app = App::new(HeadlessPlatform::default());
        let visible = app.run(|| state("visible", || true));
        let calls = Rc::new(Cell::new(0));
        let counted = calls.clone();
        let window = app
            .open_window(Default::default(), move || {
                let counted = counted.clone();
                show(
                    move || visible.get(),
                    move || {
                        let counted = counted.clone();
                        crate::app::window().after_layout(Rc::new(move || {
                            counted.set(counted.get() + 1);
                            false
                        }));
                        el().into()
                    },
                )
            })
            .unwrap();
        assert!(!window.w.after_layout.borrow().is_empty());
        let before = calls.get();
        app.run(|| visible.set(false));
        assert!(window.w.after_layout.borrow().is_empty());
        assert_eq!(calls.get(), before, "retired component feedback does not run from a detached batch");
        app.run(|| visible.set(true));
        assert_eq!(window.w.after_layout.borrow().len(), 1);
        assert!(calls.get() > before);
        app.shutdown();
        assert!(window.w.after_layout.borrow().is_empty());
    }

    #[test]
    fn closing_from_a_hook_stops_later_hooks_and_geometry_publication() {
        for shutdown in [false, true] {
            let app = App::new(HeadlessPlatform::default());
            let window = app
                .open_window(Default::default(), || {
                    let _ = size_of("box");
                    let _ = following_tail("box");
                    el().id("box").size_8().into()
                })
                .unwrap();
            let frames = window.frames_presented();
            let called = Rc::new(Cell::new(false));
            let state = window.w.clone();
            let id = window.id();
            app.run(|| {
                state.after_layout(Rc::new(move || {
                    if shutdown {
                        App::current().shutdown();
                    } else {
                        App::current().close_window(id);
                    }
                    true
                }));
                let called = called.clone();
                state.after_layout(Rc::new(move || {
                    called.set(true);
                    false
                }));
            });
            assert!(window.is_closed());
            assert!(!called.get());
            assert_eq!(window.frames_presented(), frames);
            assert!(state.sizes.borrow().is_empty() && state.tails.borrow().is_empty() && state.after_layout.borrow().is_empty());
            assert!(app.diagnostics().is_empty(), "{:?}", app.diagnostics());
            app.shutdown();
        }
    }

    #[test]
    fn removing_a_hook_drops_its_capture_outside_the_registry_borrow() {
        struct RemoveOnDrop(Rc<WindowState>, u64);
        impl Drop for RemoveOnDrop {
            fn drop(&mut self) {
                self.0.remove_after_layout(self.1);
            }
        }
        let app = App::new(HeadlessPlatform::default());
        let window = app.open_window(Default::default(), || el().into()).unwrap();
        app.run(|| {
            let later = window.w.after_layout(Rc::new(|| false));
            let captured = RemoveOnDrop(window.w.clone(), later);
            let first = window.w.after_layout(Rc::new(move || {
                let _ = &captured;
                false
            }));
            window.w.remove_after_layout(first);
            assert!(window.w.after_layout.borrow().is_empty());
        });
        app.shutdown();
    }
}
