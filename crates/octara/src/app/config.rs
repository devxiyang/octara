//! Owned resources and platform conventions prepared before an App mounts any view.

use super::{App, TargetPlatform, Window};
use crate::platform::{Platform, WindowOptions};
use crate::view::{IntoView, View};
use octara_primitives::theme::Theme;
use octara_text::{ParleyEngine, TextEngine};
use std::panic::Location;

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

/// The host that is asking an application to define its initial state and window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AppMode {
    /// A standalone application with native windows and system capabilities.
    Native,
    /// A headless inspection or check. Applications can supply explicit fixtures here.
    Headless,
}

type OpenWindow = Box<dyn FnOnce(&App, WindowOptions) -> Result<Window, String>>;

/// The initial resources and window for one run of an application.
///
/// Build a fresh definition for each run, customising the host's [`AppConfig`] before any
/// view mounts. The root is built in its own window scope, including when that window reloads.
pub struct AppDefinition {
    pub config: AppConfig,
    pub window: WindowOptions,
    open: OpenWindow,
}

impl AppDefinition {
    #[track_caller]
    pub fn new<V: IntoView>(config: AppConfig, window: WindowOptions, root: impl Fn() -> V + 'static) -> Self {
        Self::new_at(config, window, root, Location::caller())
    }

    pub(crate) fn new_at<V: IntoView>(
        config: AppConfig,
        window: WindowOptions,
        root: impl Fn() -> V + 'static,
        source: &'static Location<'static>,
    ) -> Self {
        let open = Box::new(move |app: &App, options| app.open_window(options, move || RootContent { content: root(), source }));
        Self { config, window, open }
    }

    /// Starts this definition on a host. `prepare` installs services before the window mounts.
    pub fn start(self, platform: impl Platform + 'static, prepare: impl FnOnce(&App)) -> Result<Window, String> {
        let app = App::with_config(platform, self.config);
        prepare(&app);
        let window = (self.open)(&app, self.window);
        if window.is_err() {
            app.shutdown();
        }
        window
    }
}

struct RootContent<V> {
    content: V,
    source: &'static Location<'static>,
}

impl<V: IntoView> IntoView for RootContent<V> {
    fn into_view(self, _: &'static Location<'static>) -> View {
        self.content.into_view(self.source)
    }
}
