use super::{FrameTick, OsHost};
use octara::module::HostLifecycleEvent;
use octara::runtime::WindowId;
use octara_gpu::DeviceNotice;
use octara_model::geometry::Point;
use octara_model::identity::{NativeId, TargetGuard};
use octara_model::input::{Button, EditorEvent, HostReply, InteractionEvent, KeyEvent, Modifiers, PointerId, PointerKind};
use octara_model::native::NativeEvent;
use octara_model::semantics::A11yAction;
use octara_model::theme::ColorScheme;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

/// Wakes the UI event loop from a worker.
pub type HostWake = Arc<dyn Fn() + Send + Sync>;

/// Sends data to the host's main-thread event queue and wakes it.
#[derive(Clone)]
pub struct PlatformSender(Arc<dyn Fn(PlatformEvent) + Send + Sync>);

impl PlatformSender {
    pub fn new(send: impl Fn(PlatformEvent) + Send + Sync + 'static) -> Self {
        Self(Arc::new(send))
    }

    pub fn send_event(&self, event: PlatformEvent) {
        (self.0)(event);
    }

    pub fn wake(&self) -> HostWake {
        let sender = self.clone();
        Arc::new(move || sender.send_event(PlatformEvent::UserWake))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PointerPhase {
    Started,
    Moved,
    Ended,
    Cancelled,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FileDropPhase {
    Entered,
    Left,
    Dropped,
}

/// A native key press. Dead keys can be offered to the input method without a logical key.
pub struct PlatformKey {
    pub event: Option<KeyEvent>,
    pub text: Option<String>,
}

/// Input and lifecycle notifications translated by the native event loop.
/// Positions and wheel deltas are in logical window coordinates.
pub enum PlatformEvent {
    Launched,
    Resumed,
    Suspended,
    WillTerminate,
    UserWake,
    PlatformReady,
    Lifecycle(HostLifecycleEvent),
    WindowCloseRequested(WindowId),
    WindowDestroyed(WindowId),
    WindowResized(WindowId),
    WindowMoved(WindowId),
    WindowFocused {
        window: WindowId,
        active: bool,
    },
    WindowRedrawRequested(WindowId),
    CursorMoved {
        window: WindowId,
        position: Point,
        modifiers: Modifiers,
    },
    CursorLeft(WindowId),
    Pointer {
        window: WindowId,
        phase: PointerPhase,
        position: Option<Point>,
        id: PointerId,
        kind: PointerKind,
        button: Button,
        modifiers: Modifiers,
    },
    Key {
        window: WindowId,
        event: PlatformKey,
    },
    Editor {
        window: WindowId,
        event: EditorEvent,
    },
    Interaction {
        window: WindowId,
        event: InteractionEvent,
    },
    FileDrop {
        window: WindowId,
        phase: FileDropPhase,
        position: Option<Point>,
        paths: Vec<PathBuf>,
    },
    AccessibilityEnabledChanged {
        window: WindowId,
        enabled: bool,
    },
    AccessKitAction {
        window: WindowId,
        request: accesskit::ActionRequest,
    },
    PlatformAccessibilityAction {
        window: WindowId,
        target: TargetGuard,
        action: A11yAction,
    },
    HostReply {
        window: WindowId,
        reply: HostReply,
    },
    FrameClockTick {
        window: WindowId,
        tick: FrameTick,
    },
    Frame {
        window: WindowId,
        target: Instant,
    },
    RefreshViewport {
        window: WindowId,
    },
    RefreshSystemPreferences,
    SystemPreferencesChanged {
        color_scheme: Option<ColorScheme>,
        reduced_motion: Option<bool>,
        text_scale: Option<f32>,
        high_contrast: Option<bool>,
    },
    Native {
        window: WindowId,
        id: NativeId,
        event: NativeEvent,
    },
    Device {
        generation: u64,
        notice: DeviceNotice,
    },
}

/// Scheduling instructions after draining the platform event batch.
pub struct LoopState {
    pub exit: bool,
    pub deadline: Option<Instant>,
}

/// The platform loop invokes these callbacks serially on the UI thread.
pub trait ShellDriver {
    fn start(&mut self, host: &mut dyn OsHost, sender: PlatformSender) -> Result<(), String>;
    /// Returns whether a synchronously delivered input was handled.
    fn event(&mut self, host: &mut dyn OsHost, event: PlatformEvent) -> Result<bool, String>;
    fn idle(&mut self, host: &mut dyn OsHost) -> Result<LoopState, String>;
    fn shutdown(&mut self, host: &mut dyn OsHost) -> Result<(), String>;
}
