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
use crate::view::View;

/// Opens a window showing `root` (built at the end of the frame); `Err` when the platform
/// refuses.
pub fn open_window(options: WindowOptions, root: impl Fn() -> View + 'static) -> Result<WindowId, String> {
    crate::app::app_state().request_open(options, std::rc::Rc::new(root))
}

/// Closes window `id` at the end of the frame: its scope (state, watches, tasks) is disposed.
pub fn close_window(id: WindowId) {
    crate::app::app_state().request_close(id);
}

/// The open windows, in the order they opened.
pub fn windows() -> Vec<WindowId> {
    crate::app::app_state().open_windows()
}

/// The window whose view is being built, or whose handler, binding, watch or task is running.
pub fn current_window() -> WindowId {
    crate::app::window().id
}
