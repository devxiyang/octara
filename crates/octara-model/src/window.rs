//! Host-independent window chrome, materials, and geometry.

use crate::geometry::{Point, Rect, Size};
use crate::theme::{ColorScheme, Theme};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Appearance and accessibility preferences supplied by the host.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SystemPreferences {
    pub color_scheme: ColorScheme,
    pub reduced_motion: bool,
    /// Preferred language as a BCP 47 tag.
    pub locale: Arc<str>,
    /// Preferred text size relative to the platform's default.
    pub text_scale: f32,
    pub high_contrast: bool,
}

impl Default for SystemPreferences {
    fn default() -> Self {
        Self {
            color_scheme: ColorScheme::default(),
            reduced_motion: false,
            locale: Arc::from("en-US"),
            text_scale: 1.0,
            high_contrast: false,
        }
    }
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

/// Appearance of the iOS status bar's foreground content.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StatusBarStyle {
    /// Choose a contrasting foreground from the window theme's color scheme.
    #[default]
    Auto,
    Light,
    Dark,
    Hidden,
}

/// The presentation shared by the retained tree, renderer and host window.
#[derive(Clone, Debug, PartialEq)]
pub struct WindowPresentation {
    pub title: Arc<str>,
    pub theme: Theme,
    pub material: WindowMaterial,
    pub status_bar: StatusBarStyle,
}

impl Default for WindowPresentation {
    fn default() -> Self {
        Self { title: "Octara".into(), theme: Theme::default(), material: WindowMaterial::default(), status_bar: StatusBarStyle::default() }
    }
}

/// Urgency of a request to draw attention to the application.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UserAttention {
    Informational,
    Critical,
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
