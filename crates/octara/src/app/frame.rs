//! Mounting and bounded App/window frame preparation.

use super::*;

pub(super) const WINDOW_ROUNDS: u8 = 16;

pub(super) enum AfterUpdate {
    Command(String, &'static std::panic::Location<'static>),
    FocusActivation { node: NodeId, group: NodeId },
    TouchPress(pointer::TouchPress),
    TouchScroll(pointer::TouchScrollDelta),
}

impl AppState {
    /// One App drain owns window requests and publication. Later windows can invalidate a
    /// completed window; an incomplete one waits for the next explicit preparation checkpoint.
    pub(super) fn frame(self: &Rc<Self>, first: Option<WindowId>, at: &'static std::panic::Location<'static>) {
        self.assert_usable();
        if self.shutdown.get() || self.updating.get() {
            return;
        }
        enter_app(self, || {
            self.updating.set(true);
            let _update = std::mem::DropGuard::new(&self.updating, |updating| updating.set(false));
            let mut generations = rustc_hash::FxHashMap::default();
            let mut poll = false;
            for _ in 0..WINDOW_ROUNDS {
                poll |= self.poll_requested.replace(false);
                if poll {
                    self.sched.run_ready();
                    self.rt.flush();
                }
                self.apply_window_requests();
                if self.shutdown.get() {
                    return;
                }
                let windows = self.order.borrow().clone();
                for id in first.into_iter().chain(windows.into_iter().filter(|id| Some(*id) != first)) {
                    if let Some(w) = self.handle(id).filter(|w| w.needs_preparation(generations.get(&id))) {
                        let result = w.enter(|| w.settle_window(at));
                        generations.insert(id, result);
                    }
                }
                // The last window's focus or hover cleanup can queue reactive work too.
                self.rt.flush();
                if self.shutdown.get() {
                    return;
                }
                let invalidated =
                    self.order.borrow().iter().any(|id| self.handle(*id).is_some_and(|w| w.needs_preparation(generations.get(id))));
                let work = self.poll_requested.get() || (poll && self.sched.has_ready() && !self.sched.yielded() && !self.rt.has_pending());
                if !invalidated && !work && !self.has_window_requests() {
                    self.commit_frames(&generations);
                    self.present_menus();
                    return;
                }
                if !invalidated && !work && self.window_rounds_left.get() == 0 {
                    break;
                }
            }
            // Keep every window's invalidation intact; another operation can settle it once the
            // feedback stops. Do not schedule an immediate retry of the same runaway frame.
            self.rt.diag(
                &codes::LAYOUT_LOOP,
                "App update did not settle in 16 rounds; window lifecycle, layout feedback or ready work is still pending".into(),
                at,
            );
            self.commit_frames(&generations);
            self.present_menus();
        });
    }

    fn commit_frames(self: &Rc<Self>, prepared: &rustc_hash::FxHashMap<WindowId, WindowUpdate>) {
        let windows = self.order.borrow().clone();
        for id in windows {
            let Some(w) = self.handle(id).filter(Window::can_prepare) else { continue };
            if let Some(WindowUpdate::Ready(revision)) = prepared.get(&id)
                && !self.rt.has_pending()
                && !self.has_window_requests()
                && w.preparation_is_current(*revision)
            {
                w.enter(|| w.commit_frame());
            } else {
                w.w.settled_revision.set(None);
            }
            w.w.kernel.borrow_mut().set_instant(false);
        }
    }

    pub(super) fn has_window_requests(&self) -> bool {
        !self.opening.borrow().is_empty() || !self.closing.borrow().is_empty()
    }

    fn apply_window_requests(self: &Rc<Self>) {
        if self.shutdown.get() || self.window_rounds_left.get() == 0 || !self.has_window_requests() {
            return;
        }
        self.window_rounds_left.set(self.window_rounds_left.get() - 1);
        // Take one batch. Requests from mounting and cleanup belong to a later round.
        let opening = std::mem::take(&mut *self.opening.borrow_mut());
        let closing = std::mem::take(&mut *self.closing.borrow_mut());
        for id in closing {
            self.close(id);
        }
        for id in opening {
            if self.shutdown.get() {
                break;
            }
            if let Some(w) = self.handle(id).filter(|w| !w.is_closed()) {
                w.enter(|| w.mount_root());
            }
        }
    }
}

impl Window {
    pub(super) fn can_prepare(&self) -> bool {
        !self.is_closed() && self.w.root_scope.get().is_some()
    }

    pub(super) fn revision(&self) -> WindowRevision {
        let k = self.w.kernel.borrow();
        let (width, height) = self.size();
        WindowRevision {
            mutation: k.mutation_generation(),
            paint: k.paint_generation(),
            time: Some(k.time().to_bits()),
            viewport: [width.to_bits(), height.to_bits(), self.w.scale.get().to_bits()],
        }
    }

    fn preparation_is_current(&self, revision: WindowRevision) -> bool {
        let i = self.w.interaction.borrow();
        !self.w.rebuild_pending.get()
            && i.after_update.is_empty()
            && i.focus_request.is_none()
            && i.reveal.is_none()
            && i.reveal_request.is_none()
            && !i.refresh_hover
            && self.revision() == revision
    }

    fn needs_preparation(&self, prepared: Option<&WindowUpdate>) -> bool {
        self.can_prepare()
            && match prepared {
                None => true,
                Some(WindowUpdate::Ready(revision)) => !self.preparation_is_current(*revision),
                Some(WindowUpdate::Incomplete) => false,
            }
    }

    /// Whether this window can be observed using its current, fully prepared state.
    /// Waiting for a timer or external result does not invalidate a committed view.
    pub fn frame_is_settled(&self) -> bool {
        self.can_prepare()
            && !self.app.shutdown.get()
            && !self.app.failed.get()
            && !self.app.rt.has_pending()
            && self.w.settled_revision.get().is_some_and(|revision| self.preparation_is_current(revision))
    }

    fn commit_frame(&self) {
        self.sync_native();
        let revision = self.revision();
        self.present(revision);
        // Only mark the sampled revision. A host callback may itself invalidate it.
        self.w.settled_revision.set(Some(revision));
    }

    fn published_drawing(&self) -> Option<DrawingPublication> {
        self.w.drawing.get().filter(|_| self.frame_is_settled())
    }

    /// The latest settled drawing revision, without building its display list.
    pub fn published_revision(&self) -> Option<u64> {
        self.published_drawing().map(|drawing| drawing.number)
    }

    /// The latest settled drawing, assembled once per revision. A host can coalesce multiple
    /// prepared updates by fetching this on its presentation callback.
    pub fn published_frame(&self) -> Option<crate::platform::PublishedFrame> {
        self.sample_publication(None)
    }

    /// Platform rendering entry for a display opportunity. Only the paint tracks of a moving
    /// drawing sample the target time. Settled geometry, text and application time stay current.
    #[doc(hidden)]
    pub fn published_frame_for_display(&self, target: std::time::Duration) -> Option<crate::platform::PublishedFrame> {
        self.sample_publication(Some(target))
    }

    fn sample_publication(&self, target: Option<std::time::Duration>) -> Option<crate::platform::PublishedFrame> {
        let drawing = self.published_drawing()?;
        let seconds = f64::from_bits(drawing.prepared.time.expect("prepared frame has a sample time"));
        let prepared = std::time::Duration::from_secs_f64(seconds);
        let sampled_at = target.filter(|_| drawing.continuous).map_or(prepared, |target| target.max(prepared));
        let animated_sample = sampled_at != prepared;
        let list = if animated_sample {
            self.with_picking_overlay(self.w.kernel.borrow().display_list_at(sampled_at.as_secs_f64()))
        } else {
            if let Some(frame) = self.w.published_frame.borrow().as_ref().filter(|frame| frame.revision == drawing.number) {
                return Some(frame.clone());
            }
            self.display_list()
        };
        let paint_sites = Arc::new(crate::capture::paint_sites(&self.w.kernel.borrow(), &list));
        let frame = crate::platform::PublishedFrame {
            window: self.id(),
            revision: drawing.number,
            sampled_at,
            scale: f32::from_bits(drawing.prepared.viewport[2]),
            list,
            paint_sites,
        };
        if !animated_sample {
            *self.w.published_frame.borrow_mut() = Some(frame.clone());
        }
        Some(frame)
    }

    pub(super) fn mount_root(&self) {
        // Named by the window: state is kept by its scope's path (across a hot reload), so two
        // windows' roots never share a key.
        let rt = &self.app.rt;
        let Some(s) = rt.create_child_scope(self.app.root_scope, crate::identity::ScopeSegment::Window(self.w.id.number())) else { return };
        self.w.root_scope.set(Some(s));
        let parent = Parent { node: self.w.kernel.borrow().root(), root: self.w.root_slots.clone() };
        let root = self.w.root.clone();
        rt.with_frame(Ctx::Build, None, Some(Some(s)), || {
            mount(root(), &parent, &self.w.root_slots);
            if rt.scope_alive(s) {
                // What the window draws of its requests, above the app's view.
                mount(asks::layer(), &parent, &self.w.root_slots);
            }
        });
        if rt.scope_alive(s) {
            rt.flush();
        }
        if rt.scope_alive(s) {
            self.restore_interaction();
        }
    }

    /// Applies pending updates and lays out until nothing changes: after a layout, virtual
    /// lists measure their rows and scroll containers report their state, which may change the
    /// view again. Gives up after a few rounds and reports `LAYOUT_LOOP`.
    /// This window's frame, then the app's: the other windows settle (shared state may have
    /// changed them) and windows asked for open or close.
    #[track_caller]
    pub(super) fn settle_frame(&self) {
        self.app.frame(Some(self.w.id), std::panic::Location::caller());
    }

    /// Prepares layout and interaction before the App may publish the window.
    pub(super) fn settle_window(&self, at: &'static std::panic::Location<'static>) -> WindowUpdate {
        let result = self.prepare_window(at).map(|()| self.revision());
        if self.w.rebuild_pending.replace(false) {
            self.w.kernel.borrow_mut().set_quiet(false);
            // Retiring a value can reenter and rebuild the window again. The App compares
            // the sampled revision with the resulting tree before it uses this candidate.
            self.app.rt.settle_named_state(Owner(self.w.id.number()));
        }
        match result {
            Ok(revision) => WindowUpdate::Ready(revision),
            Err(()) => {
                // Continuations of an incomplete input frame must not be replayed later.
                let touch_press_discarded = {
                    let mut interaction = self.w.interaction.borrow_mut();
                    let discarded = interaction.after_update.iter().any(|work| matches!(work, AfterUpdate::TouchPress(_)));
                    interaction.after_update.retain(|work| matches!(work, AfterUpdate::Command(..)));
                    discarded
                };
                if touch_press_discarded {
                    self.w.interaction_changed();
                }
                self.w.lists.borrow_mut().clear_reveals();
                self.w.settled_revision.set(None);
                WindowUpdate::Incomplete
            }
        }
    }

    fn prepare_window(&self, at: &'static std::panic::Location<'static>) -> Result<(), ()> {
        // Tasks woken (or spawned) by what just happened run to their next wait first; what they
        // change is laid out with the rest. They may wake or spawn more, so repeat (bounded).
        let mut update_ran = false;
        let mut prepared = false;
        for _ in 0..16 {
            if self.app.shutdown.get() || self.w.closed.get() {
                self.w.interaction.borrow_mut().after_update.clear();
                return Err(());
            }
            self.app.sched.run_ready();
            if self.prepare_interaction(at)? {
                // Focus callbacks can change visibility, open a modal or close the window.
                // Let those changes land before any queued command sees its focus path.
                update_ran = false;
                continue;
            }
            // Deliver one operation against this layout and focus path. Any resulting changes
            // settle before the next operation or publication.
            let work = self.w.interaction.borrow_mut().after_update.pop_front();
            update_ran = match work {
                Some(AfterUpdate::Command(command, source)) => self.run_after_update_command(&command, source),
                Some(AfterUpdate::FocusActivation { node, group }) => self.activate_focused_member(node, group),
                Some(AfterUpdate::TouchPress(press)) => {
                    self.deliver_touch_press(press);
                    true
                }
                Some(AfterUpdate::TouchScroll(delta)) => self.apply_touch_scroll(delta, at)?,
                None => false,
            };
            let queue_empty = self.w.interaction.borrow().after_update.is_empty();
            if !update_ran && (!self.app.sched.has_ready() || self.app.sched.yielded()) && queue_empty && !self.prepare_observation() {
                prepared = true;
                break;
            }
        }
        // The last operation may have changed state even when it queued no successor.
        if update_ran {
            // Focus callbacks need another full preparation pass before their changes can be
            // published. The exhausted budget cannot prove that work settled here.
            prepared = !self.prepare_interaction(at)? && !self.prepare_observation();
        }
        let pending = std::mem::take(&mut self.w.interaction.borrow_mut().after_update);
        if pending.iter().any(|work| matches!(work, AfterUpdate::TouchPress(_))) {
            self.w.interaction_changed();
        }
        let mut commands = pending.iter().filter_map(|work| match work {
            AfterUpdate::Command(command, source) => Some((command, source)),
            AfterUpdate::FocusActivation { .. } | AfterUpdate::TouchPress(_) | AfterUpdate::TouchScroll(_) => None,
        });
        if let Some((command, source)) = commands.next() {
            self.app.rt.diag(
                &codes::COMMAND_QUEUE_LIMIT,
                format!(
                    "deferred commands did not settle in 16 rounds; discarded {} pending command(s), starting with `{command}`",
                    1 + commands.count()
                ),
                source,
            );
        } else if !pending.is_empty() {
            prepared = false;
        }
        if !prepared {
            self.app.rt.diag(
                &codes::LAYOUT_LOOP,
                "window preparation is still pending after 16 rounds; focus or callback feedback needs more work".into(),
                at,
            );
            return Err(());
        }
        self.app.assert_usable();
        self.mark_selection();
        if self.app.sched.yielded() { Err(()) } else { Ok(()) }
    }

    fn prepare_interaction(&self, at: &'static std::panic::Location<'static>) -> Result<bool, ()> {
        self.settle_layout(at)?;
        if self.sync_modals() {
            self.settle_layout(at)?;
            // The new focus can open another modal from its focus callback.
            let shown = self.w.kernel.borrow().modals();
            let i = self.w.interaction.borrow();
            if !i.modals.iter().map(|modal| modal.node).eq(shown) {
                return Ok(true);
            }
        }
        let reveals = self.w.lists.borrow_mut().activate_reveals();
        if !reveals.is_empty() {
            for revision in reveals {
                revision.set_internal(untrack(|| revision.get()).wrapping_add(1));
            }
            return Ok(true);
        }
        Ok(self.settle_requested_focus() || self.reconcile_focus())
    }

    /// Updates that can affect layout need another preparation round before publication.
    fn prepare_observation(&self) -> bool {
        let before_motion = self.revision();
        self.w.kernel.borrow_mut().in_motion();
        if self.revision() != before_motion {
            return true;
        }
        // A caret that moved shows: the scroll containers around its field follow it.
        let reveal = self.w.interaction.borrow_mut().reveal.take();
        if let Some(n) = reveal {
            let caret = self.w.kernel.borrow().caret(n);
            if let Some(r) = caret
                && self.w.kernel.borrow_mut().reveal(n, r)
            {
                return true;
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
                return true;
            }
        }
        // What leaves the tree lets go of hover at once (even while hover holds still).
        let hovered = self.w.interaction.borrow().hovered;
        if hovered.is_some_and(|n| !self.w.kernel.borrow().is_shown(n)) {
            self.set_hover(None);
            return true;
        }
        let refresh_hover = std::mem::take(&mut self.w.interaction.borrow_mut().refresh_hover);
        refresh_hover && self.refresh_hover()
    }

    fn settle_requested_focus(&self) -> bool {
        let request = self.w.interaction.borrow_mut().focus_request.take();
        if let Some(id) = request {
            let node = self.w.kernel.borrow().find(&id);
            if let Some(node) = node {
                let _ = self.set_focus(Some(node));
            }
            return true;
        }
        false
    }

    pub(super) fn settle_layout(&self, at: &'static std::panic::Location<'static>) -> Result<(), ()> {
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
