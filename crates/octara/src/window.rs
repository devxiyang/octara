//! Windows ([design](../../../docs/design/windows.md)): one runtime, any number of windows, each
//! with its own tree, focus and size; signals and lists are shared by all of them.
//!
//! ```ignore
//! button("Preferences").on_press(|| {
//!     let _ = open_window(WindowOptions::new("Preferences").size(480.0, 360.0), preferences);
//! })
//! ```
//!
//! A window opened from app code is built at the end of the frame that asked for it, in a scope
//! of its own; closing it disposes that scope (its state, watches and tasks) and nothing else.

pub use crate::app::WindowId;
pub use crate::platform::{WindowKind, WindowOptions};
use crate::view::IntoView;

/// Opens a window showing `root` (built at the end of the frame); `Err` when the platform
/// or the current execution phase refuses.
#[track_caller]
pub fn open_window<V: IntoView>(options: WindowOptions, root: impl Fn() -> V + 'static) -> Result<WindowId, String> {
    crate::app::app_state().request_open(options, root, std::panic::Location::caller())
}

/// Retires window `id` for input immediately and releases its scope at the next App update.
#[track_caller]
pub fn close_window(id: WindowId) {
    crate::app::app_state().request_close(id, std::panic::Location::caller());
}

/// The open windows, in the order they opened.
pub fn windows() -> Vec<WindowId> {
    crate::app::app_state().open_windows()
}

/// The window whose view is being built, or whose handler, binding, watch or task is running.
pub fn current_window() -> WindowId {
    crate::app::window().id
}
