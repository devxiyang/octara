//! Host-independent window chrome, materials, and geometry.

use crate::geometry::{Point, Rect, Size};
use crate::theme::ColorScheme;
use serde::{Deserialize, Serialize};

/// Appearance and accessibility preferences supplied by the host.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SystemPreferences {
    pub color_scheme: ColorScheme,
    pub reduced_motion: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TitlebarStyle {
    #[default]
    System,
    /// Content extends under the titlebar. The optional offset positions the
    /// leading window button from the top-left of the content area.
    HiddenInset { traffic_light_offset: Option<Point> },
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VibrancyMaterial {
    #[default]
    Sidebar,
    UnderWindowBackground,
    Titlebar,
    Popover,
    Hud,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WindowMaterial {
    #[default]
    Opaque,
    Vibrancy(VibrancyMaterial),
}

/// Chrome occupied within the content area, measured in logical pixels.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct WindowChrome {
    pub titlebar_height: f32,
    pub traffic_light_rect: Option<Rect>,
    pub leading_inset: f32,
    pub trailing_inset: f32,
}

/// Live window bounds and presentation state.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct WindowGeometry {
    /// Top-left of the outer window in screen coordinates, when available.
    pub position: Option<Point>,
    /// Content size in logical pixels.
    pub size: Size,
    /// Device pixels per logical pixel.
    pub scale: f32,
    pub maximized: bool,
    pub fullscreen: bool,
}

/// Updates only the supplied window properties. Hosts report the accepted
/// values through `WindowGeometry`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct WindowGeometryRequest {
    pub position: Option<Point>,
    pub size: Option<Size>,
    pub maximized: Option<bool>,
    pub fullscreen: Option<bool>,
}
