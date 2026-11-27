//! Example applications shared by headless tests and native launchers.

use octara::prelude::{Size, WindowOptions, WindowPresentation, WindowViewport};
use std::sync::Arc;

pub mod album_back;
pub mod apps;
pub mod failure;
pub mod panel_scroll;
pub mod rename;
pub mod showcase;

pub fn window_options(title: impl Into<Arc<str>>, width: f32, height: f32) -> WindowOptions {
    WindowOptions {
        presentation: WindowPresentation { title: title.into(), ..Default::default() },
        viewport: WindowViewport { size: Size::new(width, height), scale: 1.0, safe_area: None, keyboard_inset: 0.0 },
        touch: cfg!(target_os = "ios"),
        ..Default::default()
    }
}
