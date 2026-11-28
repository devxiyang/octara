//! Drives an Octara application in a browser element.
#![cfg(all(target_arch = "wasm32", target_os = "unknown"))]

mod css;
mod dom;
mod drag_out;
mod edit;
mod engine;
mod gestures;
#[cfg(feature = "webgpu")]
mod gpu;
mod image;
mod measure;
mod media;
mod native;
mod navigation;
mod page;
mod rich;
mod services;
mod undo;

pub use engine::DomEngine;
pub use measure::MeasureMode;
pub use navigation::RouteCodec;
#[cfg(feature = "webgpu")]
pub use octara_render::{SurfaceFrame, SurfaceRenderer, SurfaceRendererFactory};

use dom::DomHandle;
use edit::Editor;
use gestures::Gestures;
use js_sys::Array;
use native::NativeViews;
use navigation::{BrowserRoutes, HistoryBridge};
use octara::module::{HostLifecycleEvent, HostSurfaceKind};
use octara::runtime::cycle::{AppError, CycleReceipt};
use octara::runtime::engine::{HostEngine, ProjectError, ProjectFeedback};
use octara::runtime::{BuildCx, HostCommand, Input, RestartState, WindowId, WindowOptions, WriteCx};
use octara::tree::{NodeFlags, Tree, TreeDelta};
use octara::{App, AppDefinition, IntoView, Navigator};
use octara_model::geometry::{LayoutBox, Point, Size, WindowViewport};
use octara_model::identity::{NodeId, TargetGuard};
use octara_model::input::{Button, ContactTarget, DroppedFile, HostReply, InteractionEvent, Modifiers, PointerId, PointerKind};
use octara_model::semantics::A11yAction;
use octara_model::theme::{ColorScheme, ColorToken};
use octara_model::window::{SystemPreferences, WindowGeometry};
use page::Page;
use services::Services;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use wasm_bindgen::{JsCast, JsValue, closure::Closure};
use wasm_bindgen_futures::spawn_local;
use web_sys::{
    AddEventListenerOptions, Document, DragEvent, Element, Event, EventTarget, HtmlElement, MediaQueryList, MessageChannel, MessageEvent,
    MessagePort, MouseEvent, PageTransitionEvent, Performance, PointerEvent, PopStateEvent, ResizeObserver, ResizeObserverEntry,
    Window as Browser,
};

/// Owns one Octara window and its browser event sources. Dropping it unmounts the window.
pub struct WebApp(Rc<Driver>);

impl WebApp {
    /// Mounts a view into a connected browser element with a size defined by its page.
    pub fn mount<V: IntoView>(mount: HtmlElement, build: impl FnOnce(&mut BuildCx<'_>) -> V + 'static) -> Result<Self, JsValue> {
        Self::mount_with_options(mount, WindowOptions::default(), build)
    }

    /// Mounts a view with application window options and the element's browser viewport.
    pub fn mount_with_options<V: IntoView>(
        mount: HtmlElement,
        mut options: WindowOptions,
        build: impl FnOnce(&mut BuildCx<'_>) -> V + 'static,
    ) -> Result<Self, JsValue> {
        Self::start(mount, DomEngine::new, |app, viewport| {
            options.viewport = viewport;
            app.open_window(options, build).map(|_| ())
        })
    }

    /// Launches a shared single-window application definition, optionally restoring its persisted state.
    pub fn launch(mount: HtmlElement, definition: AppDefinition, restart: Option<RestartState>) -> Result<Self, JsValue> {
        Self::start(mount, DomEngine::new, |app, viewport| {
            definition.launch_with_window_options(app, restart, |options| options.viewport = viewport).map(|_| ())
        })
    }

    /// Launches an application with a backend for its local WebGPU canvases.
    #[cfg(feature = "webgpu")]
    pub fn launch_with_surface_renderer(
        mount: HtmlElement,
        definition: AppDefinition,
        restart: Option<RestartState>,
        factory: SurfaceRendererFactory,
    ) -> Result<Self, JsValue> {
        Self::start(
            mount,
            |mount| DomEngine::with_surface_renderer(mount, factory),
            |app, viewport| definition.launch_with_window_options(app, restart, |options| options.viewport = viewport).map(|_| ()),
        )
    }

    fn start(
        mount: HtmlElement,
        engine: impl FnOnce(HtmlElement) -> Result<DomEngine, JsValue>,
        open: impl FnOnce(&mut App, WindowViewport) -> Result<(), AppError>,
    ) -> Result<Self, JsValue> {
        let document = mount.owner_document().ok_or("Octara needs a browser document")?;
        let browser = document.default_view().ok_or("Octara needs a browser window")?;
        let clock = browser.performance().ok_or("Octara needs a browser monotonic clock")?;
        if !mount.is_connected() {
            return Err("the Octara mount element must be connected".into());
        }
        let engine = engine(mount.clone())?;
        let dom = engine.dom();
        let channel = MessageChannel::new()?;
        let receiver = channel.port1();
        let sender = channel.port2();
        let pending = Arc::new(AtomicBool::new(false));
        let wake = {
            let sender = sender.clone();
            let pending = pending.clone();
            move || {
                if !pending.swap(true, Ordering::Relaxed) && sender.post_message(&JsValue::UNDEFINED).is_err() {
                    pending.store(false, Ordering::Relaxed);
                }
            }
        };
        let dark_mode = browser.match_media("(prefers-color-scheme: dark)")?;
        let reduced_motion = browser.match_media("(prefers-reduced-motion: reduce)")?;
        let high_contrast = browser.match_media("(prefers-contrast: more), (forced-colors: active)")?;
        let host = Self(Rc::new_cyclic(|owner: &std::rc::Weak<Driver>| {
            let weak = owner.clone();
            let on_wake = Closure::new(move |_: MessageEvent| {
                pending.store(false, Ordering::Relaxed);
                if let Some(driver) = weak.upgrade() {
                    driver.tick();
                }
            });
            receiver.set_onmessage(Some(on_wake.as_ref().unchecked_ref()));
            let weak = owner.clone();
            let on_alarm = Closure::new(move || {
                if let Some(driver) = weak.upgrade() {
                    driver.alarm.set(None);
                    driver.tick();
                }
            });
            let weak = owner.clone();
            let on_frame = Closure::new(move |_: f64| {
                if let Some(driver) = weak.upgrade() {
                    driver.frame.set(None);
                    driver.tick();
                }
            });
            Driver {
                services: RefCell::new(Services::new(browser.clone(), document.clone(), mount.clone(), owner.clone())),
                page: Page::new(browser.clone(), owner.clone()),
                browser,
                document,
                mount,
                dom,
                clock_start: clock.now(),
                clock,
                app: RefCell::default(),
                window: Cell::default(),
                attached: Cell::default(),
                visible: Cell::default(),
                native_views: RefCell::default(),
                dark_mode,
                reduced_motion,
                high_contrast,
                refresh_preferences: Cell::default(),
                stopped: Cell::default(),
                editor: Editor::default(),
                pointer: Cell::default(),
                gestures: Gestures::default(),
                refresh: Cell::default(),
                history: RefCell::default(),
                listeners: RefCell::default(),
                resize: Cell::default(),
                alarm: Cell::default(),
                frame: Cell::default(),
                receiver,
                sender,
                _on_wake: on_wake,
                on_alarm,
                on_frame,
            }
        }));
        let window = {
            let mut slot = host.0.app.borrow_mut();
            let app = slot.insert(App::new({
                let engine = Cell::new(Some(engine));
                move |_, _| match engine.take() {
                    Some(engine) => Box::new(engine),
                    None => Box::new(ExtraWindow),
                }
            }));
            app.runtime_mut().set_module_host_kind(HostSurfaceKind::Web);
            app.runtime_mut().set_media_loader(media::BrowserMediaLoader);
            app.runtime_mut().set_wake(Some(Arc::new(wake)));
            app.runtime_mut().set_system_preferences(host.0.system_preferences()).map_err(js_error)?;
            open(app, host.0.viewport()).map_err(js_error)?;
            let mut windows = app.runtime().windows();
            let Some((&window, _)) = windows.next() else { return Err("an Octara web app needs one window".into()) };
            if windows.next().is_some() {
                return Err("an Octara web app owns one browser mount element".into());
            }
            window
        };
        host.0.window.set(Some(window));
        host.0.connect()?;
        host.0
            .cycle(|app, id| {
                let urls = host.0.browser.location().href().into_iter().collect();
                app.runtime_mut().dispatch_lifecycle(HostLifecycleEvent::AppLaunched { urls, files: Vec::new() })?;
                if app.runtime().window(id).is_none() {
                    return Ok(());
                }
                host.0.attached.set(true);
                app.runtime_mut().dispatch_lifecycle(HostLifecycleEvent::WindowAttached { window: id })?;
                if app.runtime().window(id).is_none() {
                    return Ok(());
                }
                if !host.0.document.has_focus().unwrap_or(true) {
                    app.dispatch(id, Input::Interaction(InteractionEvent::WindowActive(false)))?;
                }
                host.0.set_visibility(app, !host.0.document.hidden())
            })
            .map_err(js_error)?;
        Ok(host)
    }

    pub fn window_id(&self) -> WindowId {
        self.0.window.get().expect("a mounted web application has a window identity")
    }

    pub fn with_app<R>(&self, read: impl FnOnce(&App) -> R) -> Option<R> {
        let slot = self.0.app.try_borrow().ok()?;
        let app = slot.as_ref()?;
        let value = read(app);
        // A Window handle can register a new close guard without running a cycle.
        let watching = self.0.window.get().is_some_and(|id| app.runtime().has_window_close_request(id));
        drop(slot);
        self.0.watch_page_close(watching);
        Some(value)
    }

    pub fn update<R>(&self, write: impl FnOnce(&mut WriteCx<'_>) -> R) -> Result<R, AppError> {
        self.0.cycle(|app, id| app.update_window(id, write))
    }

    pub fn dispatch(&self, input: Input) -> Result<CycleReceipt, AppError> {
        self.0.cycle(|app, id| app.dispatch(id, input))
    }

    /// Synchronizes the window's navigator with browser URLs and same-document history.
    /// A recognized initial URL replaces the current route subject to its leave guards.
    /// This binding owns the history state for the lifetime of this web application.
    pub fn bind_history<R: Clone + 'static>(&self, navigator: Navigator<R>, codec: RouteCodec<R>) -> Result<(), JsValue> {
        if self.0.history.borrow().is_some() {
            return Err("Octara already owns this web application's history".into());
        }
        let mut history = BrowserRoutes::new(&self.0.browser, navigator, codec)?;
        let initial = self.with_app(|app| history.initial_route(app)).ok_or("the Octara web app is unavailable")??;
        if let Some(route) = initial {
            self.update(|w| navigator.replace(w, route)).map_err(js_error)?;
        }
        self.with_app(|app| history.seed(app)).ok_or("the Octara web app is unavailable")??;
        self.0.history.replace(Some(Box::new(history)));
        Ok(())
    }

    pub fn viewport(&self) -> WindowViewport {
        self.0.viewport()
    }

    pub fn set_measure_mode(&self, mode: MeasureMode) {
        self.0.dom.set_measure_mode(mode);
        self.refresh();
    }

    /// Schedules a layout refresh after changes made outside the Octara tree.
    pub fn refresh(&self) {
        self.0.dom.request_full_measure();
        self.0.refresh.set(true);
        self.0.schedule_frame();
    }
}

impl Drop for WebApp {
    fn drop(&mut self) {
        self.0.stop();
    }
}

struct Driver {
    browser: Browser,
    document: Document,
    mount: HtmlElement,
    dom: DomHandle,
    services: RefCell<Services>,
    page: Page,
    clock: Performance,
    clock_start: f64,
    app: RefCell<Option<App>>,
    window: Cell<Option<WindowId>>,
    attached: Cell<bool>,
    visible: Cell<Option<bool>>,
    native_views: RefCell<NativeViews>,
    dark_mode: Option<MediaQueryList>,
    reduced_motion: Option<MediaQueryList>,
    high_contrast: Option<MediaQueryList>,
    refresh_preferences: Cell<bool>,
    stopped: Cell<bool>,
    editor: Editor,
    pointer: Cell<Option<PointerId>>,
    gestures: Gestures,
    refresh: Cell<bool>,
    history: RefCell<Option<Box<dyn HistoryBridge>>>,
    listeners: RefCell<Vec<Listener>>,
    resize: Cell<Option<Resize>>,
    alarm: Cell<Option<(Duration, i32)>>,
    frame: Cell<Option<i32>>,
    receiver: MessagePort,
    sender: MessagePort,
    _on_wake: Closure<dyn FnMut(MessageEvent)>,
    on_alarm: Closure<dyn FnMut()>,
    on_frame: Closure<dyn FnMut(f64)>,
}

impl Driver {
    fn connect(self: &Rc<Self>) -> Result<(), JsValue> {
        for query in [&self.dark_mode, &self.reduced_motion, &self.high_contrast].into_iter().flatten() {
            self.listen(query.as_ref(), "change", false, |driver, _| {
                driver.refresh_preferences.set(true);
                driver.schedule_frame();
            })?;
        }
        for name in ["languagechange", "resize"] {
            self.listen(self.browser.as_ref(), name, false, |driver, _| {
                driver.refresh_preferences.set(true);
                driver.schedule_frame();
            })?;
        }
        if let Some(viewport) = self.browser.visual_viewport() {
            for name in ["resize", "scroll"] {
                self.listen(viewport.as_ref(), name, false, |driver, _| driver.schedule_frame())?;
            }
        }
        for name in ["pointerdown", "pointermove", "pointerup", "pointercancel", "pointerleave"] {
            // Track release outside the mount and use the physical hit for captured touch pointers.
            let target =
                if matches!(name, "pointerup" | "pointermove" | "pointercancel") { self.browser.as_ref() } else { self.mount.as_ref() };
            self.listen(target, name, matches!(name, "pointerup" | "pointercancel"), move |driver, event| driver.pointer(name, &event))?;
        }
        self.connect_gestures()?;
        self.listen(self.mount.as_ref(), "click", false, |driver, event| {
            if let Some(input) = event.target().and_then(|target| target.dyn_into::<web_sys::HtmlInputElement>().ok())
                && input.type_() == "checkbox"
                && driver.dom.node(input.as_ref()).is_some()
            {
                event.prevent_default();
                if event.dyn_ref::<MouseEvent>().is_some_and(|mouse| mouse.detail() == 0) {
                    // Canceled native activation restores the pre-click properties.
                    // Reapply the authored state after the browser finishes that step.
                    spawn_local(async move {
                        let checked = input.get_attribute("aria-checked");
                        input.set_checked(checked.as_deref() == Some("true"));
                        input.set_indeterminate(checked.as_deref() == Some("mixed"));
                    });
                }
            }
            let Some(mouse) = event.dyn_ref::<MouseEvent>() else { return };
            if mouse.detail() != 0 {
                if let Some(link) = event_element(&event).and_then(|element| element.closest("a").ok().flatten())
                    && driver.dom.node(link.as_ref()).is_some()
                {
                    // The window's pointer cycle already decided the link activation.
                    event.prevent_default();
                }
                return;
            }
            // Keyboard, assistive-technology and programmatic clicks can arrive without a contact.
            let Ok(consumed) = driver.cycle(|app, id| {
                let action = driver.contact(app, id, &event).and_then(|contact| contact.action);
                let Some(target) = action else { return Ok(false) };
                app.dispatch(id, Input::Host(HostReply::AccessibilityAction { target, action: A11yAction::Press }))
                    .map(|receipt| receipt.handled)
            }) else {
                return;
            };
            if consumed {
                event.prevent_default();
            }
        })?;
        self.listen(self.mount.as_ref(), "contextmenu", false, |driver, event| {
            if let Some(element) = event_element(&event)
                && let Ok(app) = driver.app.try_borrow()
                && let Some(window) = app.as_ref().and_then(|app| app.runtime().window(driver.window.get()?))
                && let Some(hit) = driver.dom.target(window.tree(), element.as_ref()).and_then(|hit| hit.hit.node())
                && std::iter::successors(Some(hit), |&node| window.tree().logical_parent(node)).any(|node| {
                    window.tree().is_interactive(node)
                        && [octara::HandlerKind::SecondaryPress, octara::HandlerKind::ContextMenu]
                            .into_iter()
                            .any(|kind| window.tree().handler(node, kind).is_some())
                })
            {
                event.prevent_default();
            }
        })?;
        self.listen(self.mount.as_ref(), "dragstart", false, |driver, event| driver.drag_out(&event))?;
        for name in ["focus", "blur"] {
            self.listen(self.browser.as_ref(), name, false, move |driver, _| {
                if name == "blur" {
                    driver.cancel_gestures();
                    driver.pointer.set(None);
                }
                driver.send(Input::Interaction(InteractionEvent::WindowActive(name == "focus")));
            })?;
        }
        self.listen(self.document.as_ref(), "visibilitychange", false, |driver, _| {
            driver.report_visibility(!driver.document.hidden());
        })?;
        self.listen(self.browser.as_ref(), "pagehide", false, |driver, event| {
            driver.report_visibility(false);
            if !event.dyn_ref::<PageTransitionEvent>().is_some_and(PageTransitionEvent::persisted) {
                driver.stop();
            }
        })?;
        self.listen(self.browser.as_ref(), "pageshow", false, |driver, _| {
            driver.report_visibility(!driver.document.hidden());
        })?;
        self.listen(self.document.as_ref(), "fullscreenchange", false, |driver, _| driver.schedule_frame())?;
        self.listen(self.browser.as_ref(), "popstate", false, |driver, event| {
            let Some(event) = event.dyn_ref::<PopStateEvent>() else { return };
            let url = driver.browser.location().href();
            if let Err(error) = driver.cycle(|app, id| {
                if let Some(history) = driver.history.borrow_mut().as_mut() {
                    history.popstate(app, id, event.state())?;
                }
                if let Ok(url) = url {
                    app.runtime_mut().dispatch_lifecycle(HostLifecycleEvent::OpenUrls(vec![url]))?;
                }
                Ok(())
            }) {
                driver.report_error(error);
            }
        })?;
        self.listen(self.mount.as_ref(), "scroll", true, |driver, event| {
            if let Some(target) = event.target()
                && let Some(node) = driver.dom.node(target.as_ref())
                && let Some(element) = target.dyn_ref::<HtmlElement>()
            {
                let offset = undo::scroll_offset(element);
                let at_end = offset.y >= (element.scroll_height() - element.client_height()) as f32 - 0.5;
                match driver.cycle(|app, id| {
                    let tree = app.runtime().window(id).ok_or(AppError::UnknownWindow(id))?.tree();
                    if tree.scroll(node).is_none() {
                        return Ok(false);
                    }
                    let target = tree.guard(node);
                    app.dispatch(id, Input::Host(HostReply::ScrollChanged { target, offset, at_end }))?;
                    Ok(true)
                }) {
                    Ok(true) => {}
                    Ok(false) => {
                        driver.dom.resize_nodes(&[node]);
                        driver.refresh.set(true);
                        driver.schedule_frame();
                    }
                    Err(error) => driver.report_error(error),
                }
            }
        })?;
        self.connect_editor()?;
        self.listen(self.mount.as_ref(), "dragover", false, |driver, event| driver.file_drop(&event, false))?;
        self.listen(self.mount.as_ref(), "drop", false, |driver, event| driver.file_drop(&event, true))?;

        // A rem-sized box tracks computed root font changes even when the mount keeps its size.
        let text_scale_probe = dom::html(&self.document, "span")?;
        text_scale_probe.set_attribute("aria-hidden", "true")?;
        text_scale_probe.set_attribute(
            "style",
            "all:initial!important;display:block!important;position:fixed!important;bottom:100%!important;left:0!important;\
             width:0!important;height:1rem!important;overflow:hidden!important;visibility:hidden!important;pointer-events:none!important;",
        )?;
        let probe_parent = self.document.body().map(Element::from).or_else(|| self.document.document_element());
        let probe_parent = probe_parent.ok_or("Octara needs a document element to observe text scale")?;
        let weak = Rc::downgrade(self);
        let probe = text_scale_probe.clone();
        let callback = Closure::new(move |entries: Array| {
            let Some(driver) = weak.upgrade().filter(|driver| !driver.stopped.get()) else { return };
            let mut nodes = Vec::new();
            for entry in entries.iter() {
                let target = entry.unchecked_into::<ResizeObserverEntry>().target();
                if let Some(node) = driver.dom.node(target.as_ref()) {
                    nodes.push(node);
                } else if target == *driver.mount || target == *probe {
                    driver.refresh_preferences.set(true);
                }
            }
            if !nodes.is_empty() {
                driver.dom.resize_nodes(&nodes);
                driver.refresh.set(true);
            }
            // Delivery happens during browser layout. Publish any resulting DOM changes on a new frame.
            driver.schedule_frame();
        });
        let observer = ResizeObserver::new(callback.as_ref().unchecked_ref())?;
        probe_parent.append_child(&text_scale_probe)?;
        observer.observe(&text_scale_probe);
        observer.observe(&self.mount);
        self.dom.install_observer(observer.clone());
        self.resize.replace(Some(Resize { observer, text_scale_probe, _callback: callback }));
        Ok(())
    }

    fn listen(
        self: &Rc<Self>,
        target: &EventTarget,
        name: &'static str,
        capture: bool,
        action: impl Fn(&Rc<Self>, Event) + 'static,
    ) -> Result<(), JsValue> {
        let weak = Rc::downgrade(self);
        // Focus can dispatch another event synchronously while publishing a projection.
        let callback = Closure::<dyn Fn(Event)>::new(move |event: Event| {
            if let Some(driver) =
                weak.upgrade().filter(|driver| !driver.stopped.get() && !driver.dom.is_projecting() && driver.services.try_borrow().is_ok())
            {
                action(&driver, event);
            }
        });
        let options = AddEventListenerOptions::new();
        options.set_capture(capture);
        if matches!(name, "wheel" | "touchstart" | "touchmove") {
            options.set_passive(false);
        }
        target.add_event_listener_with_callback_and_add_event_listener_options(name, callback.as_ref().unchecked_ref(), &options)?;
        self.listeners.borrow_mut().push(Listener { target: target.clone(), name, capture, callback });
        Ok(())
    }

    fn contact(&self, app: &App, id: WindowId, event: &Event) -> Option<ContactTarget> {
        let tree = app.runtime().window(id)?.tree();
        event.target().and_then(|target| self.dom.target(tree, target.as_ref())).or_else(|| {
            event_element(event).filter(|element| self.mount.is_same_node(Some(element)))?;
            Some(ContactTarget { hit: TargetGuard::for_window(tree.id()), action: None })
        })
    }

    fn pointer(&self, name: &str, event: &Event) {
        let Some(pointer) = event.dyn_ref::<PointerEvent>() else { return };
        let id = PointerId(pointer.pointer_id() as u32 as u64);
        let kind = match pointer.pointer_type().as_str() {
            "touch" => PointerKind::Touch,
            "pen" => PointerKind::Pen,
            _ => PointerKind::Mouse,
        };
        if (kind == PointerKind::Touch && self.pinch_pointer(name, id, pointer)) || !pointer.is_primary() {
            return;
        }
        if matches!(name, "pointerup" | "pointercancel") && self.pointer.get() != Some(id) {
            return;
        }
        if name == "pointermove"
            && self.pointer.get() != Some(id)
            && event_element(event).is_none_or(|target| !self.mount.contains(Some(&target)))
        {
            return;
        }
        let pos = self.window_point(pointer.client_x() as f32, pointer.client_y() as f32);
        if kind == PointerKind::Mouse {
            self.gestures.cursor.set((name != "pointerleave").then_some(pos));
        }
        let mods = modifiers(pointer.shift_key(), pointer.ctrl_key(), pointer.alt_key(), pointer.meta_key());
        let result = self.cycle(|app, window| {
            let input = match name {
                "pointerdown" => {
                    let button = match pointer.button() {
                        0 => Button::Primary,
                        1 => Button::Middle,
                        2 => Button::Secondary,
                        _ => return Ok(false),
                    };
                    let Some(target) = self.contact(app, window, event) else { return Ok(false) };
                    let tree = app.runtime().window(window).expect("browser window").tree();
                    let preserve_focus = target
                        .action
                        .and_then(|guard| guard.node())
                        .and_then(|node| tree.node(node))
                        .is_some_and(|node| node.flags.contains(NodeFlags::PRESERVE_FOCUS));
                    let blocked_by_modal =
                        tree.front_modal().is_some_and(|modal| target.hit.node().is_none_or(|node| !tree.is_in_modal_scope(node, modal)));
                    if preserve_focus || blocked_by_modal {
                        event.prevent_default();
                    }
                    if let Some(previous) = self.pointer.replace(Some(id)) {
                        app.dispatch(window, Input::Interaction(InteractionEvent::PointerCancel { id: previous }))?;
                    }
                    let keyboard_open = kind == PointerKind::Touch && self.active_field().is_some();
                    app.runtime_mut().set_keyboard_open(window, keyboard_open);
                    InteractionEvent::PointerDown { id, kind, target, pos, button, mods }
                }
                "pointermove" => InteractionEvent::PointerMove { id, pos, mods },
                "pointerup" => {
                    self.pointer.set(None);
                    let tree = app.runtime().window(window).expect("browser window").tree();
                    let target = if event_element(event).is_some_and(|element| element.has_pointer_capture(pointer.pointer_id())) {
                        self.document
                            .element_from_point(pointer.client_x() as f32, pointer.client_y() as f32)
                            .and_then(|element| self.dom.target(tree, element.as_ref()))
                    } else {
                        self.contact(app, window, event)
                    };
                    InteractionEvent::PointerUp { id, target, pos, mods }
                }
                "pointercancel" => {
                    self.pointer.set(None);
                    InteractionEvent::PointerCancel { id }
                }
                "pointerleave" => InteractionEvent::PointerLeave,
                _ => return Ok(false),
            };
            app.dispatch(window, Input::Interaction(input)).map(|receipt| receipt.handled)
        });
        // Keep the page's own context menu when no app interaction consumed the event.
        if name == "pointerup" && pointer.button() == 2 && result == Ok(true) {
            event.prevent_default();
        }
    }

    fn file_drop(&self, event: &Event, dropped: bool) {
        let Some(drag) = event.dyn_ref::<DragEvent>() else { return };
        let Some(transfer) = drag.data_transfer() else { return };
        if !transfer.types().iter().any(|kind| kind.as_string().as_deref() == Some("Files")) {
            return;
        }
        let rect = self.mount.get_bounding_client_rect();
        let pos = Point::new(drag.client_x() as f32 - rect.x() as f32, drag.client_y() as f32 - rect.y() as f32);
        if !dropped {
            let target =
                self.app.try_borrow().ok().and_then(|app| app.as_ref()?.runtime().window(self.window.get()?)?.file_drop_target(pos));
            if target.is_some() {
                transfer.set_drop_effect("copy");
                event.prevent_default();
            }
            return;
        }
        let Some(files) = transfer.files() else { return };
        let files: Vec<_> = (0..files.length()).filter_map(|idx| files.get(idx)).map(|file| DroppedFile::Name(file.name())).collect();
        if files.is_empty() {
            return;
        }
        if let Err(error) = self.cycle(|app, window| {
            let Some(target) = app.runtime().window(window).and_then(|runtime| runtime.file_drop_target(pos)) else { return Ok(()) };
            event.prevent_default();
            app.dispatch(window, Input::Host(HostReply::FilesDropped { target, files }))?;
            Ok(())
        }) {
            self.report_error(error);
        }
    }

    fn viewport(&self) -> WindowViewport {
        let keyboard_inset = self
            .browser
            .visual_viewport()
            .and_then(|viewport| {
                let inner_height = self.browser.inner_height().ok()?.as_f64()?;
                let inset = (inner_height - viewport.height() - viewport.offset_top()) as f32;
                inset.is_finite().then_some(inset.max(0.0))
            })
            .unwrap_or_default();
        WindowViewport {
            size: Size::new(self.mount.client_width() as f32, self.mount.client_height() as f32),
            scale: self.browser.device_pixel_ratio() as f32,
            safe_area: None,
            keyboard_inset,
        }
    }

    fn system_preferences(&self) -> SystemPreferences {
        let dark = self.dark_mode.as_ref().is_some_and(MediaQueryList::matches);
        SystemPreferences {
            color_scheme: if dark { ColorScheme::Dark } else { ColorScheme::Light },
            reduced_motion: self.reduced_motion.as_ref().is_some_and(MediaQueryList::matches),
            locale: self.browser.navigator().language().filter(|tag| !tag.is_empty()).unwrap_or_else(|| "en-US".into()).into(),
            text_scale: self.root_text_scale(),
            high_contrast: self.high_contrast.as_ref().is_some_and(MediaQueryList::matches),
        }
    }

    fn root_text_scale(&self) -> f32 {
        self.document
            .document_element()
            .and_then(|root| self.browser.get_computed_style(&root).ok().flatten())
            .and_then(|style| style.get_property_value("font-size").ok())
            .and_then(|size| size.strip_suffix("px")?.parse::<f32>().ok())
            .map_or(1.0, |size| size / 16.0)
    }

    fn elapsed(&self) -> Duration {
        Duration::from_secs_f64((self.clock.now() - self.clock_start).max(0.0) / 1000.0)
    }

    fn set_visibility(&self, app: &mut App, visible: bool) -> Result<(), AppError> {
        if self.visible.replace(Some(visible)) != Some(visible) {
            let event = if visible { HostLifecycleEvent::AppForeground } else { HostLifecycleEvent::AppBackground };
            app.runtime_mut().dispatch_lifecycle(event)?;
        }
        Ok(())
    }

    fn report_visibility(&self, visible: bool) {
        if self.visible.get() != Some(visible)
            && let Err(error) = self.cycle(|app, _| self.set_visibility(app, visible))
        {
            self.report_error(error);
        }
    }

    fn check_page_close(&self) -> Result<bool, AppError> {
        let id = self.window.get().ok_or(AppError::AppFailed)?;
        let mut slot = self.app.try_borrow_mut().map_err(|_| AppError::AppFailed)?;
        let app = slot.as_mut().ok_or(AppError::AppFailed)?;
        let decision = app.runtime_mut().check_window_close(id);
        drop(slot);
        // Deliver effects on a regular frame if the page stays open.
        self.schedule_frame();
        decision
    }

    fn watch_page_close(&self, watching: bool) {
        if let Err(error) = self.page.watch_close(watching) {
            web_sys::console::error_1(&error);
        }
    }

    fn cycle<R>(&self, action: impl FnOnce(&mut App, WindowId) -> Result<R, AppError>) -> Result<R, AppError> {
        self.cycle_with_drag(None, action)
    }

    fn cycle_with_drag<R>(
        &self,
        drag: Option<&drag_out::DragTransfer>,
        action: impl FnOnce(&mut App, WindowId) -> Result<R, AppError>,
    ) -> Result<R, AppError> {
        if self.stopped.get() {
            return Err(AppError::AppFailed);
        }
        let mut slot = self.app.try_borrow_mut().map_err(|_| AppError::AppFailed)?;
        let app = slot.as_mut().ok_or(AppError::AppFailed)?;
        let id = self.window.get().ok_or(AppError::AppFailed)?;
        let result = (|| {
            let viewport = self.viewport();
            let window = app.runtime().window(id).ok_or(AppError::UnknownWindow(id))?;
            let current = window.window_geometry();
            let current_viewport = window.options().viewport;
            let geometry = WindowGeometry {
                size: viewport.size,
                scale: viewport.scale,
                fullscreen: self.document.fullscreen_element().is_some(),
                ..Default::default()
            };
            if self.refresh.replace(false) {
                app.runtime_mut().invalidate_projection(id)?;
            }
            if geometry != current {
                app.dispatch(id, Input::Host(HostReply::GeometryChanged(geometry)))?;
            }
            if viewport != current_viewport {
                app.dispatch(id, Input::Host(HostReply::ViewportChanged(viewport)))?;
            }
            app.advance_to(self.elapsed())?;
            self.finish_wheel_pinch(app, id)?;
            if self.refresh_preferences.replace(false) {
                let preferences = self.system_preferences();
                if app.runtime().system_preferences() != preferences {
                    app.dispatch(id, Input::Host(HostReply::SystemPreferencesChanged(preferences)))?;
                }
            }
            action(app, id)
        })();
        if matches!(&result, Err(AppError::Project { window, .. }) if *window != id) {
            // The extra windows have no mount element. Remove them so their rejected
            // projections do not block future cycles for the existing browser window.
            let extra: Vec<_> = app.runtime().windows().map(|(&window, _)| window).filter(|&window| window != id).collect();
            for window in extra {
                let _ = app.close_window(window);
            }
        }
        let dispatcher = app.runtime().module_dispatcher();
        let result = if app.runtime().is_failed() {
            result
        } else {
            let projection = self.native_views.borrow_mut().sync(app, id, &self.dom, &self.mount, &dispatcher);
            result.and_then(|value| projection.map(|()| value))
        };
        self.reconcile_editor(app, id);
        if result.is_ok()
            && let Some(history) = self.history.borrow_mut().as_mut()
            && let Err(error) = history.sync(app)
        {
            web_sys::console::error_1(&error);
        }
        let commands = app.runtime_mut().take_host_commands();
        let deadline = app.runtime().next_deadline().into_iter().chain(self.wheel_pinch_deadline()).min();
        let closed = app.runtime().window(id).is_none() || app.runtime().is_failed();
        let background = app.runtime().window(id).map(|window| window.presentation().theme.get(ColorToken::Background));
        let close_guard = app.runtime().has_window_close_request(id);
        drop(slot);
        if !closed {
            self.watch_page_close(close_guard);
            for command in commands {
                if matches!(command, HostCommand::UpdateWindowPresentation { window, .. } if window == id)
                    && let Some(background) = background
                    && let Err(error) = self.page.set_theme_color(&self.document, background)
                {
                    web_sys::console::error_1(&error);
                }
                match command {
                    HostCommand::TypedNativeCommand { window, command } => {
                        if window == id {
                            let _guard = self.dom.project_guard();
                            self.native_views.borrow_mut().command(command, &dispatcher);
                        }
                    }
                    HostCommand::StartDragOut { window, items, .. } => {
                        if window == id
                            && let Some(drag) = drag
                        {
                            drag.offer(items);
                        }
                    }
                    command => self.services.borrow_mut().apply(command),
                }
            }
            self.arm(deadline);
        } else {
            self.stop();
        }
        result
    }

    fn send(&self, input: Input) {
        if let Err(error) = self.cycle(|app, id| app.dispatch(id, input)) {
            self.report_error(error);
        }
    }

    fn tick(&self) {
        if let Err(error) = self.cycle(|_, _| Ok(())) {
            self.report_error(error);
        }
    }

    fn report_error(&self, error: AppError) {
        if !self.stopped.get() {
            web_sys::console::error_1(&js_error(error));
        }
    }

    fn defer(self: &Rc<Self>, action: impl FnOnce(&Self) + 'static) {
        let weak = Rc::downgrade(self);
        spawn_local(async move {
            if let Some(driver) = weak.upgrade().filter(|driver| !driver.stopped.get()) {
                action(&driver);
            }
        });
    }

    fn schedule_frame(&self) {
        if self.frame.get().is_none() {
            match self.browser.request_animation_frame(self.on_frame.as_ref().unchecked_ref()) {
                Ok(id) => self.frame.set(Some(id)),
                Err(_) => self.tick(),
            }
        }
    }

    fn arm(&self, deadline: Option<Duration>) {
        if self.alarm.get().map(|(deadline, _)| deadline) == deadline {
            return;
        }
        if let Some((_, alarm)) = self.alarm.take() {
            self.browser.clear_timeout_with_handle(alarm);
        }
        if let Some(deadline) = deadline {
            let delay = (deadline.saturating_sub(self.elapsed()).as_secs_f64() * 1000.0).ceil() as i32;
            if let Ok(alarm) =
                self.browser.set_timeout_with_callback_and_timeout_and_arguments_0(self.on_alarm.as_ref().unchecked_ref(), delay)
            {
                self.alarm.set(Some((deadline, alarm)));
            }
        }
    }

    fn stop(&self) {
        if self.stopped.replace(true) {
            return;
        }
        self.listeners.borrow_mut().clear();
        self.page.stop();
        self.history.take();
        self.resize.take();
        self.arm(None);
        if let Some(frame) = self.frame.take() {
            let _ = self.browser.cancel_animation_frame(frame);
        }
        self.receiver.set_onmessage(None);
        self.receiver.close();
        self.sender.close();
        self.services.borrow_mut().stop();
        if let Some(mut app) = self.app.take() {
            app.runtime_mut().set_wake(None);
            if self.attached.replace(false)
                && let Some(window) = self.window.get()
            {
                let _ = app.runtime_mut().dispatch_lifecycle(HostLifecycleEvent::WindowDetached { window });
            }
            if self.window.get().is_some() {
                let _ = app.runtime_mut().dispatch_lifecycle(HostLifecycleEvent::WillTerminate);
            }
            self.native_views.borrow_mut().clear();
            app.shutdown();
        }
    }
}

fn modifiers(shift: bool, ctrl: bool, alt: bool, meta: bool) -> Modifiers {
    let mut modifiers = Modifiers::empty();
    modifiers.set(Modifiers::SHIFT, shift);
    modifiers.set(Modifiers::CTRL, ctrl);
    modifiers.set(Modifiers::ALT, alt);
    modifiers.set(Modifiers::META, meta);
    modifiers
}

fn event_element(event: &Event) -> Option<Element> {
    event.target()?.dyn_into().ok()
}

fn js_error(error: impl std::fmt::Display) -> JsValue {
    JsValue::from_str(&error.to_string())
}

struct Listener {
    target: EventTarget,
    name: &'static str,
    capture: bool,
    callback: Closure<dyn Fn(Event)>,
}

impl Drop for Listener {
    fn drop(&mut self) {
        let _ = self.target.remove_event_listener_with_callback_and_bool(self.name, self.callback.as_ref().unchecked_ref(), self.capture);
    }
}

struct Resize {
    observer: ResizeObserver,
    text_scale_probe: HtmlElement,
    _callback: Closure<dyn FnMut(Array)>,
}

impl Drop for Resize {
    fn drop(&mut self) {
        self.observer.disconnect();
        self.text_scale_probe.remove();
    }
}

struct ExtraWindow;

impl HostEngine for ExtraWindow {
    fn project(&mut self, _: &Tree, _: &TreeDelta, _: WindowViewport, _: f64) -> Result<ProjectFeedback, ProjectError> {
        Err(ProjectError("a web app owns one browser mount element".into()))
    }
    fn commit(&mut self) -> octara::runtime::CommitOutput {
        unreachable!("an extra browser window cannot publish a projection")
    }
    fn rollback(&mut self) {}
    fn hit_test(&self, _: &Tree, _: Point) -> Option<ContactTarget> {
        None
    }
    fn geometry(&self, _: NodeId) -> Option<LayoutBox> {
        None
    }
}
