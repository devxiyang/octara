//! Direct Wayland event loop and desktop surfaces.

mod data;
mod ime;
mod input;
mod keyboard;
mod window;

use super::compose::KeySymbol;
use super::preferences::{self, Observer};
use crate::os::{HostWake, OsHost, OsWindow, OsWindowContext, PlatformEvent, PlatformKey, PlatformSender, ShellDriver};
use calloop::EventLoop;
use calloop::channel::{self, Event};
use octara::WindowOptions;
use octara::runtime::WindowId;
use octara_model::ask::{ClipboardItem, ClipboardKinds, ServiceError};
use octara_model::paint::Image;
use smithay_client_toolkit::compositor::CompositorState;
use smithay_client_toolkit::output::{OutputHandler, OutputState};
use smithay_client_toolkit::reexports::calloop_wayland_source::WaylandSource;
use smithay_client_toolkit::registry::{ProvidesRegistryState, RegistryState};
use smithay_client_toolkit::seat::SeatState;
use smithay_client_toolkit::shell::xdg::XdgShell;
use smithay_client_toolkit::shm::{Shm, ShmHandler};
use smithay_client_toolkit::{
    delegate_compositor, delegate_output, delegate_registry, delegate_seat, delegate_shm, delegate_xdg_shell, delegate_xdg_window,
    registry_handlers,
};
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, VecDeque};
use std::rc::{Rc, Weak};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;
use wayland_client::backend::ObjectId;
use wayland_client::globals::{GlobalList, registry_queue_init};
use wayland_client::protocol::{wl_data_device_manager, wl_output, wl_shm, wl_surface};
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle, delegate_noop};
use wayland_protocols::wp::{
    cursor_shape::v1::client::wp_cursor_shape_manager_v1, fractional_scale::v1::client::wp_fractional_scale_manager_v1,
    pointer_gestures::zv1::client::zwp_pointer_gestures_v1, text_input::zv3::client::zwp_text_input_manager_v3,
    viewporter::client::wp_viewporter,
};
use wayland_protocols::xdg::activation::v1::client::{xdg_activation_token_v1, xdg_activation_v1};
use window::{WaylandWindow, WindowState};

pub(super) fn create() -> Box<dyn OsHost> {
    let (send, receive) = channel::channel();
    let wake_pending = Arc::new(AtomicBool::new(false));
    let pending = wake_pending.clone();
    let sender = PlatformSender::new(move |event| {
        if !matches!(event, PlatformEvent::UserWake) || !pending.swap(true, Ordering::AcqRel) {
            let _ = send.send(event);
        }
    });
    Box::new(WaylandHost { native: None, receive: Some(receive), sender, wake_pending })
}

struct WaylandHost {
    native: Option<Rc<Shared>>,
    receive: Option<channel::Channel<PlatformEvent>>,
    sender: PlatformSender,
    wake_pending: Arc<AtomicBool>,
}

struct Shared {
    connection: Connection,
    queue: QueueHandle<Protocol>,
    compositor: CompositorState,
    shell: XdgShell,
    activation: Option<xdg_activation_v1::XdgActivationV1>,
    viewporter: Option<wp_viewporter::WpViewporter>,
    fractional: Option<wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1>,
    text_input: Option<zwp_text_input_manager_v3::ZwpTextInputManagerV3>,
    gestures: Option<zwp_pointer_gestures_v1::ZwpPointerGesturesV1>,
    cursor_shape: Option<wp_cursor_shape_manager_v1::WpCursorShapeManagerV1>,
    data_device: Option<wl_data_device_manager::WlDataDeviceManager>,
    windows: RefCell<HashMap<ObjectId, Weak<WindowState>>>,
    seats: RefCell<HashMap<ObjectId, Rc<RefCell<input::Seat>>>>,
    sender: PlatformSender,
    events: RefCell<VecDeque<NativeEvent>>,
    pending_keysym: Cell<Option<(WindowId, KeySymbol)>>,
    dispatch_grab: RefCell<Option<input::GrabOrigin>>,
    cursor_shm: Option<wl_shm::WlShm>,
    cursors: RefCell<input::Cursors>,
}

struct Protocol {
    shared: Rc<Shared>,
    registry: RegistryState,
    outputs: OutputState,
    seats: SeatState,
    shm: Option<Shm>,
}

enum NativeEvent {
    Platform(PlatformEvent),
    Grab(PlatformEvent, input::GrabOrigin),
    Key { window: WindowId, symbol: KeySymbol, key: PlatformKey },
    ResetCompose(wl_surface::WlSurface),
    Ime(ime::Update),
    Drag(data::DragUpdate),
}

fn optional_global<P>(globals: &GlobalList, queue: &QueueHandle<Protocol>, max: u32) -> Option<P>
where
    P: Proxy + 'static,
    Protocol: Dispatch<P, ()>,
{
    globals.bind(queue, 1..=max, ()).ok()
}

impl Shared {
    fn window(&self, surface: &wl_surface::WlSurface) -> Option<Rc<WindowState>> {
        self.windows.borrow().get(&surface.id())?.upgrade().filter(|window| !window.closed.get())
    }

    fn emit(&self, event: PlatformEvent) {
        self.events.borrow_mut().push_back(NativeEvent::Platform(event));
    }
}

impl OsHost for WaylandHost {
    fn run(mut self: Box<Self>, mut driver: Box<dyn ShellDriver>) -> Result<(), String> {
        let connection = Connection::connect_to_env().map_err(|error| format!("Connect to Wayland: {error}"))?;
        let (globals, queue) = registry_queue_init::<Protocol>(&connection).map_err(|error| error.to_string())?;
        let handle = queue.handle();
        let shm = Shm::bind(&globals, &handle).ok();
        let shared = Rc::new(Shared {
            connection: connection.clone(),
            queue: handle.clone(),
            compositor: CompositorState::bind(&globals, &handle).map_err(|error| error.to_string())?,
            shell: XdgShell::bind(&globals, &handle).map_err(|error| error.to_string())?,
            activation: optional_global(&globals, &handle, 1),
            viewporter: optional_global(&globals, &handle, 1),
            fractional: optional_global(&globals, &handle, 1),
            text_input: optional_global(&globals, &handle, 1),
            gestures: optional_global(&globals, &handle, 2),
            cursor_shape: optional_global(&globals, &handle, 1),
            data_device: optional_global(&globals, &handle, 3),
            cursor_shm: shm.as_ref().map(|shm| shm.wl_shm().clone()),
            cursors: RefCell::default(),
            windows: RefCell::default(),
            seats: RefCell::default(),
            sender: self.sender.clone(),
            events: RefCell::default(),
            pending_keysym: Cell::new(None),
            dispatch_grab: RefCell::default(),
        });
        let mut protocol = Protocol {
            shared: shared.clone(),
            registry: RegistryState::new(&globals),
            outputs: OutputState::new(&globals, &handle),
            seats: SeatState::new(&globals, &handle),
            shm,
        };

        for seat in protocol.seats.seats() {
            protocol.add_seat(seat);
        }

        let mut event_loop = EventLoop::<Protocol>::try_new().map_err(|error| error.to_string())?;
        WaylandSource::new(connection, queue).insert(event_loop.handle()).map_err(|error| error.to_string())?;
        let receive = self.receive.take().ok_or("Wayland event loop already initialized")?;
        let wake_pending = self.wake_pending.clone();
        event_loop
            .handle()
            .insert_source(receive, move |event, (), state| {
                if let Event::Msg(event) = event {
                    if matches!(event, PlatformEvent::UserWake) {
                        wake_pending.store(false, Ordering::Release);
                    }
                    state.shared.emit(event);
                }
            })
            .map_err(|error| error.to_string())?;

        self.native = Some(shared.clone());
        let sender = self.sender.clone();
        let _preferences = Observer::start(sender.clone()).ok();
        let result = (|| {
            driver.start(self.as_mut(), sender)?;
            driver.event(self.as_mut(), PlatformEvent::Launched)?;
            loop {
                loop {
                    let event = shared.events.borrow_mut().pop_front();
                    match event {
                        Some(NativeEvent::Platform(event)) => {
                            driver.event(self.as_mut(), event)?;
                        }
                        Some(NativeEvent::Grab(event, grab)) => {
                            shared.dispatch_grab.replace(Some(grab));
                            let result = driver.event(self.as_mut(), event);
                            shared.dispatch_grab.take();
                            result?;
                        }
                        Some(NativeEvent::Key { window, symbol, key }) => {
                            shared.pending_keysym.set(Some((window, symbol)));
                            let result = driver.event(self.as_mut(), PlatformEvent::Key { window, event: key });
                            shared.pending_keysym.take();
                            result?;
                        }
                        Some(NativeEvent::ResetCompose(surface)) => {
                            if let Some(window) = shared.window(&surface)
                                && let Some(compose) = &window.compose
                            {
                                compose.reset();
                            }
                        }
                        Some(NativeEvent::Ime(update)) => {
                            update.apply(&shared, self.as_mut(), driver.as_mut())?;
                        }
                        Some(NativeEvent::Drag(update)) => {
                            update.apply(&shared, self.as_mut(), driver.as_mut())?;
                        }
                        None => break,
                    }
                }
                let state = driver.idle(self.as_mut())?;
                if state.exit {
                    break;
                }
                let now = Instant::now();
                let deadline = state.deadline.into_iter().chain(protocol.repeat_deadline()).min();
                let timeout = if shared.events.borrow().is_empty() {
                    deadline.map(|at| at.saturating_duration_since(now))
                } else {
                    Some(std::time::Duration::ZERO)
                };
                event_loop.dispatch(timeout, &mut protocol).map_err(|error| error.to_string())?;
                protocol.repeat_keys(Instant::now());
            }
            driver.event(self.as_mut(), PlatformEvent::WillTerminate)?;
            Ok(())
        })();
        let shutdown = driver.shutdown(self.as_mut());
        result.and(shutdown)
    }

    fn wake(&self) -> HostWake {
        self.sender.wake()
    }

    fn follows_window_focus(&self) -> bool {
        true
    }

    fn locale(&self) -> Option<Arc<str>> {
        preferences::locale()
    }

    fn create_window(&mut self, id: WindowId, options: &WindowOptions, context: OsWindowContext<'_>) -> Result<Box<dyn OsWindow>, String> {
        let shared = self.native.as_ref().ok_or("Wayland windows require a running event loop")?;
        Ok(Box::new(WaylandWindow::new(shared.clone(), id, options, context.sender)))
    }

    fn open_url(&mut self, url: &str) {
        super::open_url(url);
    }

    fn read_clipboard(&mut self, accept: ClipboardKinds) -> Option<Vec<ClipboardItem>> {
        self.native.as_ref()?.read_clipboard(accept).ok()
    }

    fn write_clipboard(&mut self, items: &[ClipboardItem]) -> Result<(), ServiceError> {
        self.native.as_ref().ok_or(ServiceError::Unavailable)?.write_clipboard(items)
    }

    fn write_clipboard_image(&mut self, image: &Image) {
        use image::{ExtendedColorType, ImageEncoder, codecs::png::PngEncoder};

        let frame = image.frame();
        let mut bytes = Vec::new();
        if PngEncoder::new(&mut bytes).write_image(&frame.rgba, frame.width, frame.height, ExtendedColorType::Rgba8).is_ok() {
            let _ = self.write_clipboard(&[ClipboardItem::Image { media_type: "image/png".into(), bytes }]);
        }
    }
}

impl ProvidesRegistryState for Protocol {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry
    }

    registry_handlers!(OutputState, SeatState);
}

impl OutputHandler for Protocol {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.outputs
    }
    fn new_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
    fn update_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
    fn output_destroyed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
}

impl ShmHandler for Protocol {
    fn shm_state(&mut self) -> &mut Shm {
        self.shm.as_mut().expect("bound wl_shm")
    }
}

impl Dispatch<xdg_activation_token_v1::XdgActivationTokenV1, wl_surface::WlSurface> for Protocol {
    fn event(
        state: &mut Self,
        request: &xdg_activation_token_v1::XdgActivationTokenV1,
        event: xdg_activation_token_v1::Event,
        surface: &wl_surface::WlSurface,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_activation_token_v1::Event::Done { token } = event {
            request.destroy();
            if let Some(activation) = &state.shared.activation
                && state.shared.window(surface).is_some()
                && !token.is_empty()
            {
                activation.activate(token, surface);
            }
        }
    }
}

delegate_noop!(Protocol: ignore xdg_activation_v1::XdgActivationV1);
delegate_compositor!(Protocol);
delegate_output!(Protocol);
delegate_registry!(Protocol);
delegate_seat!(Protocol);
delegate_shm!(Protocol);
delegate_xdg_shell!(Protocol);
delegate_xdg_window!(Protocol);
delegate_noop!(Protocol: ignore wp_viewporter::WpViewporter);
delegate_noop!(Protocol: ignore wp_cursor_shape_manager_v1::WpCursorShapeManagerV1);
delegate_noop!(Protocol: ignore wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1);
delegate_noop!(Protocol: ignore zwp_text_input_manager_v3::ZwpTextInputManagerV3);
delegate_noop!(Protocol: ignore zwp_pointer_gestures_v1::ZwpPointerGesturesV1);
delegate_noop!(Protocol: ignore wl_data_device_manager::WlDataDeviceManager);
