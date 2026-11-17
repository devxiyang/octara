//! Mounting and bounded App/window frame preparation.

use super::*;

impl AppState {
    /// A frame for the whole app: every window settles (shared state may have changed what any
    /// of them shows), then windows asked for open and windows asked to close go.
    pub(super) fn frame(self: &Rc<Self>, at: &'static std::panic::Location<'static>) {
        self.frame_except(None, at);
    }

    /// Retain this frame's completed and incomplete attempts. Later windows can invalidate a
    /// completed window; an incomplete one waits for the next explicit update boundary.
    pub(super) fn frame_except(self: &Rc<Self>, settled: Option<(WindowId, WindowUpdate)>, at: &'static std::panic::Location<'static>) {
        self.assert_usable();
        if self.shutdown.get() {
            return;
        }
        enter_app(self, || {
            let mut generations = rustc_hash::FxHashMap::default();
            generations.extend(settled);
            for _ in 0..16 {
                let windows = self.order.borrow().clone();
                for id in windows {
                    if let Some(w) = self.handle(id) {
                        if generations
                            .get(&id)
                            .is_some_and(|outcome: &WindowUpdate| !outcome.invalidated(w.w.kernel.borrow().mutation_generation()))
                        {
                            continue;
                        }
                        let generation = w.enter(|| w.settle_window(at));
                        generations.insert(id, generation);
                    }
                }
                // The last window's focus or hover cleanup can queue reactive work too.
                self.rt.flush();
                let opening = std::mem::take(&mut *self.opening.borrow_mut());
                let closing = std::mem::take(&mut *self.closing.borrow_mut());
                if opening.is_empty() && closing.is_empty() {
                    let invalidated = self.order.borrow().iter().any(|id| {
                        self.handle(*id).is_some_and(|w| {
                            generations.get(id).is_none_or(|outcome| outcome.invalidated(w.w.kernel.borrow().mutation_generation()))
                        })
                    });
                    if !invalidated {
                        self.present_menus();
                        return;
                    }
                }
                for id in opening {
                    if let Some(w) = self.handle(id) {
                        w.enter(|| w.mount_root());
                    }
                }
                for id in closing {
                    self.close(id);
                }
            }
            // Keep every window's invalidation intact; another operation can settle it once the
            // feedback stops. Do not schedule an immediate retry of the same runaway frame.
            self.rt.diag(
                &codes::LAYOUT_LOOP,
                "window updates did not settle in 16 rounds: windows keep changing each other's layout or presentation".into(),
                at,
            );
            self.present_menus();
        });
    }
}

impl Window {
    pub(super) fn mount_root(&self) {
        // Named by the window: state is kept by its scope's path (across a hot reload), so two
        // windows' roots never share a key.
        let s = self.app.rt.create_scope(Some(self.app.root_scope), &format!("window{}", self.w.id.number()));
        self.w.root_scope.set(Some(s));
        let parent = Parent { node: self.w.kernel.borrow().root(), root: self.w.root_slots.clone() };
        let root = self.w.root.clone();
        self.app.rt.with_frame(Ctx::Build, None, Some(Some(s)), || {
            mount(root(), &parent, &self.w.root_slots);
            // What the window draws of its requests, above the app's view.
            mount(asks::layer(), &parent, &self.w.root_slots);
        });
        self.app.rt.flush();
        self.restore_interaction();
        self.settle_frame();
    }

    /// Applies pending updates and lays out until nothing changes: after a layout, virtual
    /// lists measure their rows and scroll containers report their state, which may change the
    /// view again. Gives up after a few rounds and reports `LAYOUT_LOOP`.
    /// This window's frame, then the app's: the other windows settle (shared state may have
    /// changed them) and windows asked for open or close.
    #[track_caller]
    pub(super) fn settle_frame(&self) {
        let at = std::panic::Location::caller();
        let generation = self.settle_window(at);
        self.app.frame_except(Some((self.w.id, generation)), at);
    }

    /// One window's part of a frame: tasks, layout, native views, text input, focus and hover.
    pub(super) fn settle_window(&self, at: &'static std::panic::Location<'static>) -> WindowUpdate {
        match self.prepare_window(at) {
            Ok(generation) => WindowUpdate::Ready(generation),
            Err(()) => {
                self.w.settled_generation.set(None);
                WindowUpdate::Incomplete
            }
        }
    }

    fn prepare_window(&self, at: &'static std::panic::Location<'static>) -> Result<u64, ()> {
        // Tasks woken (or spawned) by what just happened run to their next wait first; what they
        // change is laid out with the rest. They may wake or spawn more, so repeat (bounded).
        let mut command_ran = false;
        for _ in 0..16 {
            if self.app.shutdown.get() || self.w.closed.get() || self.app.closing.borrow().contains(&self.w.id) {
                self.w.interaction.borrow_mut().after_update.clear();
                return Ok(self.w.kernel.borrow().mutation_generation());
            }
            self.app.sched.run_ready();
            self.settle_layout(at)?;
            // Modals that opened or closed move focus (which restyles).
            if self.sync_modals() {
                self.settle_layout(at)?;
            }
            if self.settle_requested_focus(at)? || self.reconcile_focus() {
                // Focus callbacks can change visibility, open a modal or close the window.
                // Let those changes land before any queued command sees its focus path.
                command_ran = false;
                continue;
            }
            // Route one command, then let its updates and focus changes settle before the
            // next. This uses the same round bound as tasks and never nests an input frame.
            let command = self.w.interaction.borrow_mut().after_update.pop_front();
            if let Some((command, source)) = command {
                command_ran = self.run_after_update_command(&command, source);
            } else {
                command_ran = false;
            }
            if !command_ran
                && (!self.app.sched.has_ready() || self.app.sched.yielded())
                && self.w.interaction.borrow().after_update.is_empty()
            {
                break;
            }
        }
        // The last command may have changed state even when it queued no successor.
        if command_ran {
            self.settle_layout(at)?;
            if self.sync_modals() {
                self.settle_layout(at)?;
            }
            self.settle_requested_focus(at)?;
            self.reconcile_focus();
        }
        let pending = std::mem::take(&mut self.w.interaction.borrow_mut().after_update);
        if let Some((command, source)) = pending.front() {
            self.app.rt.diag(
                &codes::COMMAND_QUEUE_LIMIT,
                format!(
                    "deferred commands did not settle in 16 rounds; discarded {} pending command(s), starting with `{command}`",
                    pending.len()
                ),
                source,
            );
        }
        // A caret that moved shows: the scroll containers around its field follow it.
        let reveal = self.w.interaction.borrow_mut().reveal.take();
        if let Some(n) = reveal {
            let caret = self.w.kernel.borrow().caret(n);
            if let Some(r) = caret
                && self.w.kernel.borrow_mut().reveal(n, r)
            {
                self.settle_layout(at)?;
            }
        }
        // Highlighting an option can reveal it without moving keyboard focus from its field.
        let request = self.w.interaction.borrow_mut().reveal_request.take();
        if let Some(id) = request {
            let mut k = self.w.kernel.borrow_mut();
            let moved = k.find(&id).filter(|n| k.is_shown(*n)).is_some_and(|n| {
                let (x, y, w, h) = k.shown_rect(n);
                k.reveal(n, crate::paint::Rect { x, y, w, h })
            });
            drop(k);
            if moved {
                self.settle_layout(at)?;
            }
        }
        self.app.assert_usable();
        self.sync_native();
        self.present();
        let generation = self.w.kernel.borrow().mutation_generation();
        self.w.settled_generation.set(Some(generation));
        // An `instant` transaction covers the frame it was made in.
        self.w.kernel.borrow_mut().set_instant(false);
        // What is on its way out can no longer be focused or hovered.
        let (focused, hovered) = {
            let i = self.w.interaction.borrow();
            (i.focused, i.hovered)
        };
        let gone = |n: Option<NodeId>| n.is_some_and(|n| !self.w.kernel.borrow().is_shown(n));
        if gone(focused) {
            self.reconcile_focus();
        }
        // What leaves the tree lets go of hover at once (even while hover holds still).
        if gone(hovered) {
            self.set_hover(None);
        }
        if self.app.sched.yielded() { Err(()) } else { Ok(generation) }
    }

    fn settle_requested_focus(&self, at: &'static std::panic::Location<'static>) -> Result<bool, ()> {
        let request = self.w.interaction.borrow_mut().focus_request.take();
        if let Some(id) = request {
            let node = self.w.kernel.borrow().find(&id);
            if let Some(node) = node
                && self.set_focus(Some(node)).is_ok()
            {
                self.settle_layout(at)?;
            }
            return Ok(true);
        }
        Ok(false)
    }

    fn settle_layout(&self, at: &'static std::panic::Location<'static>) -> Result<(), ()> {
        for _ in 0..4 {
            if !self.feedback_active() {
                return Err(());
            }
            self.app.rt.flush();
            if !self.feedback_active() || self.app.rt.has_pending() {
                return Err(());
            }
            {
                let mut kernel = self.w.kernel.borrow_mut();
                if kernel.has_active_edit() {
                    return Err(());
                }
                kernel.reconcile_edits();
                kernel.layout(self.size().0, self.size().1);
            }
            let changed = self.app.rt.with_frame(Ctx::Handler, None, Some(None), || self.sync_after_layout());
            if !self.feedback_active() {
                return Err(());
            }
            if !changed {
                return Ok(());
            }
        }
        self.app.rt.diag(
            &codes::LAYOUT_LOOP,
            "layout is still pending after 4 rounds; size reads or virtual measurements need more work (a repeating feedback loop may be the cause)".into(),
            at,
        );
        Err(())
    }
}
