//! Owned resources and platform conventions prepared before an App mounts any view.

use super::TargetPlatform;
use octara_primitives::theme::Theme;
use octara_text::{ParleyEngine, TextEngine};

/// Initial resources shared by all windows of an App.
///
/// Configure before [`super::App::with_config`]. Font data can be registered on `text_engine`
/// here; no runtime scope, deferred task or first-frame correction is needed. Once consumed,
/// the App owns these resources. The text engine cannot be replaced on a running App.
pub struct AppConfig {
    pub theme: Theme,
    /// Bundled fonts by default, for deterministic headless output.
    pub text_engine: Box<dyn TextEngine>,
    /// Keyboard and touch conventions used from the first build.
    pub target_platform: TargetPlatform,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self { theme: Theme::default(), text_engine: Box::new(ParleyEngine::default()), target_platform: TargetPlatform::current() }
    }
}

impl AppConfig {
    /// Selects the platform's fonts and fallback before mounting. Any fonts registered with
    /// this config's previous engine are discarded; register custom fonts after this call.
    #[cfg(feature = "system-fonts")]
    pub fn with_system_fonts(mut self) -> Self {
        self.text_engine = Box::new(ParleyEngine::system());
        self
    }
}
