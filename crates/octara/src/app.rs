//! Application and window entry points for author-built views.

use crate::view::IntoView;
use octara_model::ask::Menu;
use octara_model::input::Chord;
use octara_runtime::media::{MediaCacheBackend, VideoSurfaceProvider};
use octara_runtime::turn::{TurnError, TurnReceipt};
use octara_runtime::window::WindowRoot;
use octara_runtime::{
    AppRuntime, BuildCx, HostEngine, Input, MountCx, ReadCx, RestartState, WindowId, WindowOptions, WindowRuntime, WriteCx,
};
use std::sync::Arc;
use std::time::Duration;

pub struct App {
    runtime: AppRuntime,
}

impl App {
    pub fn new(engine: impl Fn(WindowId, &WindowOptions) -> Box<dyn HostEngine> + 'static) -> Self {
        Self { runtime: AppRuntime::new(engine) }
    }

    pub fn with_namespace(namespace: [u64; 2], engine: impl Fn(WindowId, &WindowOptions) -> Box<dyn HostEngine> + 'static) -> Self {
        Self { runtime: AppRuntime::with_namespace(namespace, engine) }
    }

    pub fn runtime(&self) -> &AppRuntime {
        &self.runtime
    }
    pub fn runtime_mut(&mut self) -> &mut AppRuntime {
        &mut self.runtime
    }

    pub fn open_window<V: IntoView>(
        &mut self,
        options: WindowOptions,
        build: impl FnOnce(&mut BuildCx<'_>) -> V + 'static,
    ) -> Result<WindowId, TurnError> {
        self.runtime.open_window(options, move |mount| mount_view(mount, build))
    }

    pub fn window(&self, id: WindowId) -> Option<Window<'_>> {
        self.runtime.window(id).map(Window)
    }

    pub fn close_window(&mut self, id: WindowId) -> Result<(), TurnError> {
        self.runtime.close_window(id)
    }

    pub fn build<R>(&mut self, build: impl FnOnce(&mut BuildCx<'_>) -> R) -> Result<R, TurnError> {
        self.runtime.build(build)
    }

    pub fn update<R>(&mut self, update: impl FnOnce(&mut WriteCx<'_>) -> R) -> Result<R, TurnError> {
        self.runtime.update(None, update)
    }

    pub fn update_window<R>(&mut self, id: WindowId, update: impl FnOnce(&mut WriteCx<'_>) -> R) -> Result<R, TurnError> {
        self.runtime.update(Some(id), update)
    }

    pub fn dispatch(&mut self, id: WindowId, input: Input) -> Result<TurnReceipt, TurnError> {
        self.runtime.dispatch(id, input)
    }

    pub fn advance_to(&mut self, time: Duration) -> Result<TurnReceipt, TurnError> {
        self.runtime.advance_to(time)
    }

    pub fn export_persisted(&self) -> RestartState {
        self.runtime.export_persisted()
    }

    pub fn import_persisted(&mut self, state: RestartState) -> bool {
        self.runtime.import_persisted(state)
    }

    pub fn shutdown(&mut self) {
        self.runtime.shutdown();
    }
}

pub struct Window<'a>(&'a WindowRuntime);

impl<'a> Window<'a> {
    pub fn id(&self) -> WindowId {
        self.0.id()
    }
    pub fn runtime(&self) -> &'a WindowRuntime {
        self.0
    }

    /// Return true to allow a host close request or false to keep the window open.
    /// A deferred action can close the window with `WriteCx::close_window`.
    pub fn on_close_request(&self, run: impl Fn(&mut WriteCx<'_>) -> bool + 'static) {
        self.0.on_close_request(run);
    }
}

type Startup = Box<dyn FnOnce(&mut App) -> Result<(), TurnError>>;

#[derive(Default)]
pub struct AppDefinition {
    state_schema: String,
    startup: Option<Startup>,
    windows: Vec<WindowDefinition>,
    media_cache: Option<Arc<dyn MediaCacheBackend>>,
    video: Option<Arc<dyn VideoSurfaceProvider>>,
}

impl AppDefinition {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn state_schema(mut self, schema: impl Into<String>) -> Self {
        self.state_schema = schema.into();
        self
    }

    pub fn startup(mut self, startup: impl FnOnce(&mut App) -> Result<(), TurnError> + 'static) -> Self {
        self.startup = Some(Box::new(startup));
        self
    }

    pub fn window(mut self, window: WindowDefinition) -> Self {
        self.windows.push(window);
        self
    }

    pub fn with_media_cache_backend(mut self, backend: Arc<dyn MediaCacheBackend>) -> Self {
        self.media_cache = Some(backend);
        self
    }

    pub fn with_video_surface_provider(mut self, provider: Arc<dyn VideoSurfaceProvider>) -> Self {
        self.video = Some(provider);
        self
    }

    pub fn launch(self, app: &mut App, restart: Option<RestartState>) -> Result<Vec<WindowId>, TurnError> {
        self.launch_with_window_options(app, restart, |_| {})
    }

    /// Launch with host overrides applied before each declared window builds its view.
    pub fn launch_with_window_options(
        self,
        app: &mut App,
        restart: Option<RestartState>,
        mut configure: impl FnMut(&mut WindowOptions),
    ) -> Result<Vec<WindowId>, TurnError> {
        app.runtime.set_state_schema(self.state_schema);
        if let Some(backend) = self.media_cache {
            app.runtime.set_media_cache_backend(backend);
        }
        if let Some(video) = self.video {
            app.build(|build| build.provide::<Arc<dyn VideoSurfaceProvider>>(video))?;
        }
        if let Some(restart) = restart {
            app.import_persisted(restart);
        }
        if let Some(startup) = self.startup {
            startup(app)?;
        }
        self.windows
            .into_iter()
            .map(|mut window| {
                configure(&mut window.options);
                let WindowDefinition { options, root, declarations } = window;
                app.runtime.open_window(options, move |mount| {
                    if !declarations.is_empty() {
                        mount.build(|build| {
                            for declare in declarations {
                                declare(build);
                            }
                        });
                    }
                    root(mount);
                })
            })
            .collect()
    }
}

type Declare = Box<dyn FnOnce(&mut BuildCx<'_>)>;

pub struct WindowDefinition {
    options: WindowOptions,
    root: WindowRoot,
    declarations: Vec<Declare>,
}

impl WindowDefinition {
    pub fn new<V: IntoView>(options: WindowOptions, build: impl FnOnce(&mut BuildCx<'_>) -> V + 'static) -> Self {
        Self { options, root: Box::new(move |mount| mount_view(mount, build)), declarations: Vec::new() }
    }

    /// Add reactive menus to this window's root scope before building its view.
    pub fn menus(mut self, source: impl FnMut(&mut ReadCx<'_>) -> Vec<Menu> + 'static) -> Self {
        self.declarations.push(Box::new(move |build| build.menus(source)));
        self
    }

    /// Register a shortcut in this window's root scope.
    pub fn on_chord(mut self, chord: Chord, run: impl Fn(&mut WriteCx<'_>) + 'static) -> Self {
        self.declarations.push(Box::new(move |build| build.on_chord(chord, run)));
        self
    }

    /// Intercept host close requests for this window. Return true to allow closing.
    pub fn on_close_request(mut self, run: impl Fn(&mut WriteCx<'_>) -> bool + 'static) -> Self {
        self.declarations.push(Box::new(move |build| build.on_close_request(run)));
        self
    }
}

/// Open another window from an event, watch or task write context.
pub trait WindowActions {
    fn open_window<V: IntoView>(&mut self, options: WindowOptions, build: impl FnOnce(&mut BuildCx<'_>) -> V + 'static) -> WindowId;
}

impl WindowActions for WriteCx<'_> {
    fn open_window<V: IntoView>(&mut self, options: WindowOptions, build: impl FnOnce(&mut BuildCx<'_>) -> V + 'static) -> WindowId {
        self.request_window(options, move |mount| mount_view(mount, build))
    }
}

fn mount_view<V: IntoView>(mount: &mut MountCx<'_>, build: impl FnOnce(&mut BuildCx<'_>) -> V) {
    let root = mount.root();
    let view = mount.build(build).into_view();
    view.mount(mount, root, 0);
}
