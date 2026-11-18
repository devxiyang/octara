//! Native view synchronization, capture and platform presentation.

use super::*;
use crate::capture::{Capture, CaptureId, Capturing, NativeShot, NativeShotRequest, Painter, Pixels, Shown, SnapshotStatus};
use crate::image::Image;
use crate::native::NativeId;
use crate::platform::NativeSnapshotRequest;
use rustc_hash::FxHashMap;
use std::time::Instant;

// ---- native views ----

impl Window {
    /// Tells the native host what changed this frame: views created (with their props), props
    /// changed, placements changed, views gone.
    pub(super) fn sync_native(&self) {
        let k = self.w.kernel.borrow();
        let mut sent = self.w.native_sent.borrow_mut();
        if k.native_nodes().is_empty() && sent.is_empty() {
            return;
        }
        let window = self.w.platform.borrow();
        let mut changes = self.w.native_changes.borrow_mut();
        let placements = k.native_placements();
        let mut moved = Vec::new();
        let mut live = Vec::new();
        for (n, p) in &placements {
            let Some(v) = k.native(*n) else { continue };
            match sent.get_mut(n) {
                None => {
                    live.push((*n, window.shows_native(&v.kind, v.slot)));
                    changes.push(crate::native::NativeChange::Create {
                        id: p.id,
                        kind: v.kind.clone(),
                        slot: v.slot,
                        props: v.props.clone(),
                    });
                    sent.insert(*n, (v.version, None));
                }
                Some((version, _)) if *version != v.version => {
                    changes.push(crate::native::NativeChange::Update { id: p.id, props: v.props.clone() });
                    *version = v.version;
                }
                Some(_) => {}
            }
            let entry = sent.get_mut(n).expect("sent");
            if entry.1.as_ref() != Some(p) {
                entry.1 = Some(p.clone());
                moved.push(p.clone());
            }
        }
        if !moved.is_empty() {
            changes.push(crate::native::NativeChange::Place { placements: moved });
        }
        let mounted: Vec<NodeId> = placements.iter().map(|(n, _)| *n).collect();
        sent.retain(|n, _| {
            let keep = mounted.contains(n);
            if !keep {
                changes.push(crate::native::NativeChange::Destroy { id: crate::native::NativeId(crate::kernel::a11y::a11y_id(*n).0) });
            }
            keep
        });
        drop((k, window, sent, changes));
        // Painted next as the platform shows them: no placeholder, a cutout for the underlay.
        for (n, shown) in live {
            self.w.kernel.borrow_mut().set_native_live(n, shown);
        }
    }

    /// The node of a native view's id, if it still exists.
    pub(super) fn native_node(&self, id: crate::native::NativeId) -> Option<NodeId> {
        let k = self.w.kernel.borrow();
        k.node_of_a11y(accesskit::NodeId(id.0)).filter(|n| k.native(*n).is_some())
    }

    /// An event from native view `id` (`InputEvent::Native`): runs its `on_native`.
    /// Files dragged from the system: the element under them that takes them (`on_drop`, the
    /// nearest at or above the point) is `DRAG_OVER`; dropped, it gets their paths.
    pub(crate) fn on_file_drag(&mut self, ev: crate::app::FileDrag) -> bool {
        let target = |x: f32, y: f32| {
            let k = self.w.kernel.borrow();
            k.hit_test(x, y).and_then(|n| k.handler_above(Some(n), HandlerKind::Drop))
        };
        let (over, dropped) = match ev {
            crate::app::FileDrag::Over { x, y, .. } => (target(x, y), None),
            crate::app::FileDrag::Drop { x, y, paths } => (None, target(x, y).map(|t| (t, paths))),
            crate::app::FileDrag::Leave => (None, None),
        };
        let handler = dropped.as_ref().and_then(|(t, _)| {
            let k = self.w.kernel.borrow();
            k.handler(*t, HandlerKind::Drop).and_then(|h| self.w.handlers.borrow().get(&h.0).cloned())
        });
        self.input_frame(|| {
            let was = std::mem::replace(&mut self.w.interaction.borrow_mut().drop_target, over);
            if was != over {
                let mut k = self.w.kernel.borrow_mut();
                if let Some(n) = was.filter(|n| k.is_alive(*n)) {
                    k.set_state(n, State::DRAG_OVER, false);
                }
                if let Some(n) = over {
                    k.set_state(n, State::DRAG_OVER, true);
                }
            }
            if let (Some(handler), Some((_, paths))) = (handler, &dropped) {
                handler.run(&self.app.rt, |handler| {
                    if let Handler::Drop(f) = handler {
                        f(paths);
                    }
                });
            }
        });
        true
    }

    pub(crate) fn on_native_event(&mut self, id: crate::native::NativeId, event: crate::native::NativeEvent) -> bool {
        let Some(n) = self.native_node(id) else { return false };
        let handler = {
            let k = self.w.kernel.borrow();
            k.handler(n, HandlerKind::Native).and_then(|h| self.w.handlers.borrow().get(&h.0).cloned())
        };
        let Some(handler) = handler else { return false };
        self.input_frame(|| {
            handler.run(&self.app.rt, |handler| {
                if let Handler::Native(f) = handler {
                    f(&event);
                }
            });
        });
        true
    }

    /// The natural size native view `id` reports (`InputEvent::NativeSize`): used where no size
    /// is authored.
    pub(crate) fn on_native_size(&mut self, id: crate::native::NativeId, width: f32, height: f32) -> bool {
        let Some(n) = self.native_node(id) else { return false };
        self.input_frame(|| self.w.kernel.borrow_mut().set_native_size(n, (width, height)));
        true
    }

    /// What keeps the app from being settled now: tasks, timers and motion (not subscriptions,
    /// which run for good), pending reactive updates, and native views that are busy (a page
    /// loading). A capture waits for this to be empty.
    pub fn unsettled(&self) -> Vec<String> {
        let mut out: Vec<String> = self
            .app()
            .pending()
            .into_iter()
            .filter(|p| p.kind != crate::task::WorkKind::Subscription)
            .map(|p| if p.at.is_empty() { p.label } else { format!("{} ({})", p.label, p.at) })
            .collect();
        let placements = self.w.kernel.borrow().native_placements();
        let window = self.w.platform.borrow();
        for (n, p) in placements.into_iter().filter(|(_, p)| p.visible) {
            if let Some(what) = window.native_busy(p.id) {
                out.push(format!("{}: {what}", self.native_name(n)));
            }
        }
        out
    }

    /// A native view as reports name it: its test id, else its kind.
    pub(super) fn native_name(&self, n: NodeId) -> String {
        let k = self.w.kernel.borrow();
        let kind = k.native(n).map(|v| v.kind.clone()).unwrap_or_default();
        match k.test_id(n) {
            Some(id) => format!("{kind} view #{id}"),
            None => format!("{kind} view"),
        }
    }

    /// The topmost element at `(x, y)` (logical pixels), with its number, test id, source and
    /// the elements around it (the inspector's `pick`).
    pub fn pick(&self, x: f32, y: f32) -> Option<crate::inspect::Picked> {
        crate::inspect::pick_for_window(&self.w.kernel.borrow(), x, y, self.submit_target())
    }

    /// Starts a painted capture, replacing any previous one for this window. Requests a
    /// snapshot of each visible native view and accepts its response until `deadline`.
    pub fn capture_begin(&mut self, deadline: Instant) -> Result<CaptureId, String> {
        self.check_running()?;
        self.w.capturing.borrow_mut().take();
        let id = CaptureId::new();
        let placements = self.w.kernel.borrow().native_placements();
        let mut window = self.w.platform.borrow_mut();
        let shots = placements
            .into_iter()
            .filter(|(_, p)| p.visible)
            .map(|(_, p)| {
                let request = NativeSnapshotRequest::new(p.id);
                let (status, misses) = match window.snapshot_native(request) {
                    Ok(misses) => (SnapshotStatus::Waiting, misses),
                    Err(reason) => (SnapshotStatus::Placeholder(reason), Vec::new()),
                };
                (p.id, NativeShotRequest { request, status, misses })
            })
            .collect();
        *self.w.capturing.borrow_mut() = Some(Capturing { id, deadline, shots });
        Ok(id)
    }

    /// The native views this capture is still waiting for.
    pub fn capture_awaiting(&self, id: CaptureId) -> Vec<NativeId> {
        let capturing = self.w.capturing.borrow();
        let Some(capture) = capturing.as_ref().filter(|c| c.id == id && Instant::now() < c.deadline) else { return Vec::new() };
        let mut awaiting: Vec<_> = capture
            .shots
            .iter()
            .filter_map(|(&id, shot)| (matches!(shot.status, SnapshotStatus::Waiting) && self.native_node(id).is_some()).then_some(id))
            .collect();
        awaiting.sort_unstable();
        awaiting
    }

    /// Discards this capture if it is still active.
    pub fn capture_cancel(&self, id: CaptureId) {
        self.w.capturing.borrow_mut().take_if(|capture| capture.id == id);
    }

    pub(super) fn capture_resized(&self) {
        if let Some(capture) = self.w.capturing.borrow_mut().as_mut() {
            for shot in capture.shots.values_mut() {
                if matches!(shot.status, SnapshotStatus::Waiting | SnapshotStatus::Captured(_)) {
                    shot.status = SnapshotStatus::Failed("the window resized or changed scale while taking its snapshot");
                }
            }
        }
    }

    /// Consumes the active capture, paints its snapshots into the frame, and writes a PNG.
    /// `pending` is what [`unsettled`](Self::unsettled) reported when settling ended. `fallback`
    /// describes why a `display` request was painted.
    pub fn capture_finish(
        &mut self,
        id: CaptureId,
        painter: &mut dyn Painter,
        path: &str,
        scale: f32,
        pending: Vec<String>,
        fallback: Option<String>,
    ) -> Result<Capture, String> {
        let capture = self.w.capturing.borrow_mut().take_if(|capture| capture.id == id);
        self.check_running()?;
        let mut shots = capture.ok_or_else(|| "the painted capture is no longer active".to_owned())?.shots;
        let k = self.w.kernel.borrow();
        let mut snapshots = FxHashMap::default();
        let native = k
            .native_placements()
            .into_iter()
            .filter(|(_, p)| p.visible)
            .filter_map(|(n, p)| {
                let v = k.native(n)?;
                let (status, misses) = shots.remove(&p.id).map_or_else(
                    || (SnapshotStatus::Failed("the view became visible after snapshots were requested"), Vec::new()),
                    |shot| (shot.status, shot.misses),
                );
                let (shown, reason) = match status {
                    SnapshotStatus::Waiting => (Shown::Failed, Some("its snapshot did not come in time".to_owned())),
                    SnapshotStatus::Failed(why) => (Shown::Failed, Some(why.to_owned())),
                    SnapshotStatus::Placeholder(why) => (Shown::Placeholder, Some(why)),
                    SnapshotStatus::Captured(image) => {
                        snapshots.insert(n, image);
                        (Shown::Snapshot, None)
                    }
                };
                let r = p.rect;
                Some(NativeShot {
                    id: k.test_id(n).map(str::to_owned),
                    kind: v.kind.clone(),
                    rect: [r.x, r.y, r.w, r.h],
                    shown,
                    reason,
                    misses,
                })
            })
            .collect();
        let list = k.capture_display_list(&snapshots);
        drop(k);
        let list = self.with_picking_overlay(Rc::new(list));
        let picture = painter.paint(&list, scale)?;
        picture.save_png(path).map_err(|e| format!("{path}: {e}"))?;
        Ok(Capture {
            path: path.to_owned(),
            width: picture.width,
            height: picture.height,
            scale,
            pixels: Pixels::Painted,
            fallback,
            renderer: painter.renderer().to_owned(),
            settled: pending.is_empty(),
            pending,
            native,
        })
    }

    /// A `display` capture: the window server's `picture` of the window, written to `path` as a
    /// PNG, with the native views reported live.
    pub fn capture_display(
        &mut self,
        picture: &crate::capture::Picture,
        path: &str,
        scale: f32,
        pending: Vec<String>,
    ) -> Result<Capture, String> {
        if self.app.failed.get() {
            return Err(format!("[{}] cannot capture a failed App", codes::APP_FAILED));
        }
        picture.save_png(path).map_err(|e| format!("{path}: {e}"))?;
        let placements = self.w.kernel.borrow().native_placements();
        let k = self.w.kernel.borrow();
        let native = placements
            .into_iter()
            .filter(|(_, p)| p.visible)
            .filter_map(|(n, p)| {
                let v = k.native(n)?;
                let r = p.rect;
                Some(NativeShot {
                    id: k.test_id(n).map(str::to_owned),
                    kind: v.kind.clone(),
                    rect: [r.x, r.y, r.w, r.h],
                    shown: Shown::Live,
                    reason: None,
                    misses: Vec::new(),
                })
            })
            .collect();
        Ok(Capture {
            path: path.to_owned(),
            width: picture.width,
            height: picture.height,
            scale,
            pixels: Pixels::Display,
            fallback: None,
            renderer: "display".to_owned(),
            settled: pending.is_empty(),
            pending,
            native,
        })
    }

    /// Delivers a platform response only to its waiting request in the active capture.
    pub(crate) fn on_native_snapshot(&self, request: NativeSnapshotRequest, image: Option<Image>) -> bool {
        if self.check_running().is_err() {
            return false;
        }
        let mut capturing = self.w.capturing.borrow_mut();
        let Some(capture) = capturing.as_mut().filter(|c| Instant::now() < c.deadline) else { return false };
        let Some(shot) = capture.shots.get_mut(&request.id) else { return false };
        if shot.request != request || !matches!(shot.status, SnapshotStatus::Waiting) || self.native_node(request.id).is_none() {
            return false;
        }
        shot.status = image.map_or(SnapshotStatus::Failed("the platform returned no snapshot"), SnapshotStatus::Captured);
        true
    }

    /// Read the backend adapter (downcast with `as_any`; headless, a
    /// [`HeadlessWindow`](crate::platform::HeadlessWindow)). Mutation belongs to the runtime
    /// and backend; tests configure a headless host before opening its window.
    pub fn platform_window(&self) -> std::cell::Ref<'_, dyn crate::platform::PlatformWindow> {
        std::cell::Ref::map(self.w.platform.borrow(), |w| &**w)
    }

    /// Tells the window's platform about the focused text field when it changed: where its
    /// caret is, whether it is secure or multi-line.
    /// How many frames the window presented to its platform window (each operation that changed
    /// something presents one).
    pub fn frames_presented(&self) -> u64 {
        self.w.frames.get()
    }

    /// Whether the window is the active one (key on macOS; GPUI's `is_window_active`): its
    /// focused field shows a caret. Headless windows are active.
    pub fn is_active(&self) -> bool {
        self.w.kernel.borrow().window_active()
    }

    /// How the pointer looks now: over the element it is on (an arrow off the window).
    pub fn cursor(&self) -> octara_primitives::input::CursorStyle {
        let hovered = self.w.interaction.borrow().hovered;
        let k = self.w.kernel.borrow();
        hovered.filter(|n| k.is_alive(*n)).map_or(octara_primitives::input::CursorStyle::Arrow, |n| k.cursor_over(n))
    }

    /// Presents what changed to the window's platform: native view changes, the text input,
    /// whether to draw again (after every settle; nothing when nothing changed).
    pub(super) fn present(&self) {
        self.mark_selection();
        let text_input = self.focused_input().and_then(|(n, _)| {
            let k = self.w.kernel.borrow();
            let caret = k.caret(n)?;
            let secure = k.is_secure(n);
            let document = (!secure).then(|| k.text_document(n)).flatten();
            let (geometry, layout) = if secure {
                (None, None)
            } else {
                let field = k.field(n)?;
                let layout = k.input_layout(n);
                (Some(field.into_geometry()), Some(layout))
            };
            Some(crate::platform::TextInput { id: n, caret, secure, multiline: k.is_multiline(n), document, geometry, layout })
        });
        let generation = self.w.kernel.borrow().paint_generation();
        let (picking, picked) = self.picking_frame();
        let redraw = picking || generation != self.w.presented.get() || self.w.kernel.borrow_mut().in_motion();
        let native = std::mem::take(&mut *self.w.native_changes.borrow_mut());
        let cursor = self.cursor();
        let asks = std::mem::take(&mut *self.w.asks_new.borrow_mut());
        let withdrawn = std::mem::take(&mut *self.w.asks_withdrawn.borrow_mut());
        if native.is_empty()
            && !redraw
            && picked.is_none()
            && text_input == *self.w.text_input.borrow()
            && cursor == self.w.cursor.get()
            && asks.is_empty()
            && withdrawn.is_empty()
        {
            return;
        }
        let published_input = text_input.clone();
        self.w.platform.borrow_mut().present(crate::platform::Frame { native, text_input, redraw, cursor, picked, asks, withdrawn });
        // Only a successful host handoff advances the publication record. A host panic
        // fails the App; it must not make a partially submitted frame look completed.
        *self.w.text_input.borrow_mut() = published_input;
        self.w.cursor.set(cursor);
        self.w.presented.set(generation);
        self.w.frames.set(self.w.frames.get() + 1);
    }
}

impl App {
    /// Installs what paints `capture` requests headless (`octara-render`'s reference renderer).
    pub fn set_painter(&self, painter: Box<dyn crate::capture::Painter>) {
        let retired = self.app.painter.borrow_mut().replace(painter);
        drop(retired);
    }

    /// Takes the installed painter out while it paints (see [`set_painter`](Self::set_painter)).
    pub(crate) fn take_painter(&self) -> Option<Box<dyn crate::capture::Painter>> {
        self.app.painter.borrow_mut().take()
    }

    /// Read the system adapter (downcast with `as_any`; headless, a
    /// [`HeadlessPlatform`](crate::platform::HeadlessPlatform)). Clipboard operations use
    /// [`crate::view::read_from_clipboard`] / [`crate::view::write_to_clipboard`] inside
    /// [`Self::run`] or a handler; open windows through [`Self::open_window`].
    pub fn platform(&self) -> std::cell::Ref<'_, dyn crate::platform::Platform> {
        std::cell::Ref::map(self.app.system.borrow(), |p| &**p)
    }
}
