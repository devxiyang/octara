//! Input as data ([core](https://github.com/octoii-works/octara/blob/main/docs/design/core.md)):
//! every input a window gets is an [`InputEvent`], through one entry, [`Window::event`]. A
//! backend sends what its windows report; tests and agents drive by test id with helpers
//! (`tap`, `press_key`, `type_text`, …) that send events too. An app can record the events it
//! gets, with the time and the window, and replay them on another app built the same way: the
//! same snapshots and display lists on the virtual clock.

use super::{App, Event, KeyEvent, PointerEvent, Window, WindowId};
use crate::native::{NativeEvent, NativeId};
use serde::{Deserialize, Serialize};
use std::time::Duration;

/// One input to a window.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InputEvent {
    /// A mouse, touch or pen event, in logical pixels from the window's top-left corner.
    Pointer(PointerEvent),
    /// The pointer left the window: nothing is hovered.
    PointerLeave,
    /// A wheel or trackpad scroll of `(dx, dy)` at `(x, y)`.
    Wheel { x: f32, y: f32, dx: f32, dy: f32 },
    /// A key press. What nothing handles types its text only when the backend sends `Text`.
    Key(KeyEvent),
    /// Text typed, or committed by an input method, into the focused field (it ends any
    /// composition).
    Text(String),
    /// What an input method is composing in the focused field (empty ends it).
    Composition(String),
    /// The window's content size (logical pixels) and its scale (device pixels per logical one).
    Resize { width: f32, height: f32, scale: f32 },
    /// An assistive technology's request.
    Accessibility { target: accesskit::NodeId, action: accesskit::Action, data: Option<accesskit::ActionData> },
    /// What a native view reports.
    Native { id: NativeId, event: NativeEvent },
    /// A native view's natural size.
    NativeSize { id: NativeId, width: f32, height: f32 },
    /// A native view's snapshot (for a capture); a recording keeps that it came, not its pixels.
    NativeSnapshot {
        id: NativeId,
        #[serde(skip)]
        image: Option<crate::image::Image>,
    },
    /// A step by test id or by name (press, input, toggle, hover, focus, scroll, send).
    Semantic(Event),
    /// Starts or ends picking: a person pointing at an element to hand it to an agent (see
    /// [`Window::set_picking`]).
    Picking(bool),
}

impl Window {
    /// Delivers one input to this window and runs its frame. Returns whether something handled
    /// it (a key nothing took types nothing until the backend sends its text; a native event
    /// for a view gone, an assistive request for a node gone, go nowhere).
    pub fn event(&mut self, event: InputEvent) -> bool {
        self.app.record(self.w.id, &event);
        if self.picks(&event) {
            return true;
        }
        match event {
            InputEvent::Pointer(ev) => {
                self.on_pointer(ev);
                true
            }
            InputEvent::PointerLeave => {
                self.on_pointer_leave();
                true
            }
            InputEvent::Wheel { x, y, dx, dy } => {
                self.on_wheel(x, y, dx, dy);
                true
            }
            InputEvent::Key(ev) => self.on_key(ev),
            InputEvent::Text(text) => self.on_text(&text),
            InputEvent::Composition(text) => {
                self.on_composition(&text);
                true
            }
            InputEvent::Resize { width, height, scale } => {
                self.on_resize(width, height, scale);
                true
            }
            InputEvent::Accessibility { target, action, data } => self.on_accessibility(target, action, data),
            InputEvent::Native { id, event } => self.on_native_event(id, event),
            InputEvent::NativeSize { id, width, height } => self.on_native_size(id, width, height),
            InputEvent::NativeSnapshot { id, image } => self.on_native_snapshot(id, image),
            InputEvent::Semantic(ev) => self.on_semantic(ev).is_ok(),
            InputEvent::Picking(on) => {
                self.on_picking(on);
                true
            }
        }
    }
}

/// One recorded input: when on the app's clock, to which window, what.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Recorded {
    /// Milliseconds on the app's clock.
    pub at: f64,
    pub window: WindowId,
    pub event: InputEvent,
}

/// A session's inputs, in order (JSON with serde).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Recording {
    pub events: Vec<Recorded>,
}

impl App {
    /// Starts recording every input the app's windows get (a recording in progress starts
    /// over).
    pub fn start_recording(&self) {
        *self.app.recording.borrow_mut() = Some(Recording::default());
    }

    /// Stops recording and returns what was recorded.
    pub fn stop_recording(&self) -> Recording {
        self.app.recording.borrow_mut().take().unwrap_or_default()
    }

    /// Replays `recording` on this app: the clock moves to each input's time, then the input
    /// reaches its window. Built the same way (the same view, windows opened in the same order),
    /// the app ends where the recorded one did.
    pub fn replay(&self, recording: &Recording) -> Result<(), String> {
        for r in &recording.events {
            let at = Duration::from_secs_f64(r.at / 1000.0);
            if at > self.now() {
                self.advance_to(at);
            }
            let mut w =
                self.window(r.window).ok_or_else(|| format!("no window {:?} to replay into (open it as the recording did)", r.window))?;
            w.event(r.event.clone());
        }
        Ok(())
    }
}

impl super::AppState {
    /// Records `event` for window `window` when a recording is on.
    pub(crate) fn record(&self, window: WindowId, event: &InputEvent) {
        if let Some(r) = self.recording.borrow_mut().as_mut() {
            r.events.push(Recorded { at: self.sched.now_ms(), window, event: event.clone() });
        }
    }
}
