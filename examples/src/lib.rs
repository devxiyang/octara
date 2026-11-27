//! Example applications and their shared native launch options.

use octara::prelude::{Size, WindowOptions, WindowViewport};

pub mod album_back;
pub mod apps;
pub mod failure;
pub mod gpu_lab;
pub mod native;
pub mod panel_scroll;
pub mod rename;
pub mod showcase;

pub fn window_options(title: impl Into<String>, width: f32, height: f32) -> WindowOptions {
    WindowOptions {
        title: title.into(),
        viewport: WindowViewport { size: Size::new(width, height), scale: 1.0, safe_area: None },
        touch: cfg!(target_os = "ios"),
        ..Default::default()
    }
}
