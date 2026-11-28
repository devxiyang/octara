//! X11 windows and protocol events, owned by Octara's UI thread.

mod clipboard;
mod drag;
mod input;
mod window;
mod xlib;

use super::compose::KeySymbol;
use super::preferences;
use crate::os::{HostWake, OsHost, OsWindow, OsWindowContext, PlatformEvent, PlatformSender, ShellDriver};
use clipboard::Clipboard;
use octara::WindowOptions;
use octara::runtime::WindowId;
use octara_model::ask::{ClipboardItem, ClipboardKinds, ServiceError};
use octara_model::paint::Image;
use rustix::event::{PollFd, PollFlags, Timespec, poll};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::rc::{Rc, Weak};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, OnceLock};
use std::time::Instant;
use x11rb::connection::Connection;
use x11rb::protocol::Event;
use x11rb::protocol::xproto::{Atom, AtomEnum, ChangeWindowAttributesAux, ConnectionExt, EventMask, Screen, Window};
use x11rb::xcb_ffi::XCBConnection;
use xlib::Display;

type XResult<T> = Result<T, Box<dyn std::error::Error>>;

x11rb::atom_manager! {
    Atoms: AtomsCookie {
        UTF8_STRING, RESOURCE_MANAGER,
        WM_PROTOCOLS, WM_DELETE_WINDOW, WM_CHANGE_STATE, _MOTIF_WM_HINTS,
        _NET_SUPPORTED, _NET_WM_NAME, _NET_WM_PID, _NET_ACTIVE_WINDOW, _NET_WM_MOVERESIZE, _NET_MOVERESIZE_WINDOW,
        _GTK_THEME_VARIANT,
        _NET_FRAME_EXTENTS, _NET_REQUEST_FRAME_EXTENTS,
        _NET_WM_STATE, _NET_WM_STATE_ABOVE, _NET_WM_STATE_FULLSCREEN,
        _NET_WM_STATE_MAXIMIZED_VERT, _NET_WM_STATE_MAXIMIZED_HORZ,
        _NET_WM_STATE_SKIP_TASKBAR, _NET_WM_STATE_SKIP_PAGER, _NET_WM_STATE_DEMANDS_ATTENTION,
        _NET_WM_WINDOW_TYPE, _NET_WM_WINDOW_TYPE_NORMAL, _NET_WM_WINDOW_TYPE_UTILITY,
        XdndAware, XdndProxy, XdndEnter, XdndLeave, XdndPosition, XdndStatus, XdndDrop,
        XdndFinished, XdndSelection, XdndTypeList, XdndActionCopy, XdndActionList,
        OCTARA_XDND_DATA, TARGETS, TIMESTAMP, INCR,
        URI_LIST: b"text/uri-list",
        TEXT_PLAIN: b"text/plain",
        TEXT_UTF8: b"text/plain;charset=utf-8",
    }
}

pub(super) fn create() -> Box<dyn OsHost> {
    Box::new(X11Host { clipboard: Rc::new(Clipboard::new()), native: None, sender: Arc::new(OnceLock::new()) })
}

struct Native {
    // Drop the borrowed XCB wrapper before closing the owning Xlib display.
    conn: XCBConnection,
    display: Display,
    screen: usize,
    root: Window,
    atoms: Atoms,
    scale: Cell<f32>,
    cursor: Option<x11rb::cursor::Handle>,
    windows: RefCell<HashMap<Window, Weak<window::WindowState>>>,
    source: RefCell<drag::DragSource>,
    timestamp: Cell<u32>,
    pending_keysym: Cell<Option<(WindowId, KeySymbol)>>,
}

impl Native {
    fn connect() -> XResult<Self> {
        let (display, screen) = Display::open()?;
        // SAFETY: The sole XCB wrapper is dropped before the display, both during
        // initialization and once stored in Native.
        let conn = unsafe { display.borrow_xcb()? };
        let root = conn.setup().roots[screen].root;
        let atoms = Atoms::new(&conn)?.reply()?;
        conn.change_window_attributes(
            conn.setup().roots[0].root,
            &ChangeWindowAttributesAux::new().event_mask(EventMask::PROPERTY_CHANGE),
        )?
        .check()?;
        let resources = x11rb::resource_manager::new_from_default(&conn).unwrap_or_default();
        let scale = Cell::new(scale_factor(&resources, &conn.setup().roots[screen]));
        let cursor = x11rb::cursor::Handle::new(&conn, screen, &resources).ok().and_then(|cookie| cookie.reply().ok());
        Ok(Self {
            conn,
            display,
            screen,
            root,
            atoms,
            scale,
            cursor,
            windows: RefCell::default(),
            source: RefCell::default(),
            timestamp: Cell::new(x11rb::CURRENT_TIME),
            pending_keysym: Cell::new(None),
        })
    }

    fn screen(&self) -> &Screen {
        &self.conn.setup().roots[self.screen]
    }

    fn window(&self, xid: Window) -> Option<Rc<window::WindowState>> {
        self.windows.borrow().get(&xid)?.upgrade()
    }

    fn property32(&self, xid: Window, property: Atom, kind: impl Into<Atom>, limit: u32) -> Option<Vec<u32>> {
        let reply = self.conn.get_property(false, xid, property, kind, 0, limit).ok()?.reply().ok()?;
        Some(reply.value32()?.collect())
    }

    fn supports(&self, hint: Atom) -> bool {
        self.property32(self.root, self.atoms._NET_SUPPORTED, AtomEnum::ATOM, 4096).is_some_and(|hints| hints.contains(&hint))
    }

    fn unregister(&self, xid: Window) {
        self.windows.borrow_mut().remove(&xid);
        self.source.borrow_mut().cancel_window(self, xid);
    }

    fn scale_changed(&self) -> Vec<PlatformEvent> {
        let resources = x11rb::resource_manager::new_from_default(&self.conn).unwrap_or_default();
        let scale = scale_factor(&resources, self.screen());
        if self.scale.replace(scale) == scale {
            return Vec::new();
        }
        self.windows
            .borrow()
            .values()
            .filter_map(Weak::upgrade)
            .map(|window| {
                window.update_size_hints(self);
                PlatformEvent::WindowResized(window.id)
            })
            .collect()
    }
}

fn scale_factor(resources: &x11rb::resource_manager::Database, screen: &Screen) -> f32 {
    let physical_dpi = if screen.width_in_millimeters == 0 {
        96.0
    } else {
        f32::from(screen.width_in_pixels) * 25.4 / f32::from(screen.width_in_millimeters)
    };
    let configured_dpi = resources.get_value::<f32>("Xft.dpi", "Xft.Dpi").ok().flatten().filter(|value| value.is_finite() && *value > 0.0);
    // X servers without reliable monitor dimensions often report just under 96 dpi.
    let dpi = configured_dpi.unwrap_or_else(|| if (physical_dpi - 96.0).abs() < 2.0 { 96.0 } else { physical_dpi });
    (dpi / 96.0).clamp(0.5, 8.0)
}

struct X11Host {
    clipboard: Rc<Clipboard>,
    native: Option<Rc<Native>>,
    sender: Arc<OnceLock<PlatformSender>>,
}

impl OsHost for X11Host {
    fn run(mut self: Box<Self>, mut driver: Box<dyn ShellDriver>) -> Result<(), String> {
        let native = Rc::new(Native::connect().map_err(|error| error.to_string())?);
        let (sender, inbox) = Inbox::new().map_err(|error| error.to_string())?;
        self.native = Some(native.clone());
        self.sender.set(sender.clone()).map_err(|_| "X11 event loop already initialized")?;
        let _preferences = preferences::Observer::start(sender.clone()).ok();
        let result = driver.start(self.as_mut(), sender).and_then(|()| self.event_loop(driver.as_mut(), &native, inbox));
        let termination = if result.is_ok() { driver.event(self.as_mut(), PlatformEvent::WillTerminate).map(|_| ()) } else { Ok(()) };
        let shutdown = driver.shutdown(self.as_mut());
        self.clipboard.persist();
        result.and(termination).and(shutdown)
    }

    fn wake(&self) -> HostWake {
        let sender = self.sender.clone();
        Arc::new(move || {
            if let Some(sender) = sender.get() {
                sender.send_event(PlatformEvent::UserWake);
            }
        })
    }

    fn follows_window_focus(&self) -> bool {
        true
    }

    fn locale(&self) -> Option<Arc<str>> {
        preferences::locale()
    }

    fn create_window(&mut self, id: WindowId, options: &WindowOptions, context: OsWindowContext<'_>) -> Result<Box<dyn OsWindow>, String> {
        let native = self.native.as_ref().ok_or("X11 windows require a running event loop")?;
        let window =
            window::X11Window::new(native.clone(), self.clipboard.clone(), id, options, context).map_err(|error| error.to_string())?;
        Ok(Box::new(window))
    }

    fn open_url(&mut self, url: &str) {
        super::open_url(url);
    }

    fn read_clipboard(&mut self, accept: ClipboardKinds) -> Option<Vec<ClipboardItem>> {
        self.clipboard.read(accept).ok()
    }

    fn write_clipboard(&mut self, items: &[ClipboardItem]) -> Result<(), ServiceError> {
        self.clipboard.write(items.to_vec())
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

impl X11Host {
    fn event_loop(&mut self, driver: &mut dyn ShellDriver, native: &Native, mut inbox: Inbox) -> Result<(), String> {
        let mut keyboard = input::Keyboard::new(native);
        driver.event(self, PlatformEvent::Launched)?;
        let mut queued = None;
        loop {
            const BATCH_LIMIT: usize = 256;
            inbox.drain_pipe().map_err(|error| error.to_string())?;
            let mut worker_count = 0;
            while worker_count < BATCH_LIMIT {
                let Ok(event) = inbox.events.try_recv() else { break };
                if matches!(event, PlatformEvent::UserWake) {
                    inbox.wake_pending.store(false, Ordering::Release);
                }
                driver.event(self, event)?;
                worker_count += 1;
            }
            let mut native_count = 0;
            while native_count < BATCH_LIMIT {
                let event = match queued.take() {
                    Some(event) => event,
                    None => match native.conn.poll_for_event().map_err(|error| error.to_string())? {
                        Some(event) => event,
                        None => break,
                    },
                };
                if let Event::PropertyNotify(property) = &event
                    && property.window == native.conn.setup().roots[0].root
                    && property.atom == native.atoms.RESOURCE_MANAGER
                {
                    for event in native.scale_changed() {
                        driver.event(self, event)?;
                    }
                } else if !drag::dispatch(native, &keyboard, &event, |event| driver.event(self, event))?
                    && let Some(event) = input::translate(native, &mut keyboard, event)
                {
                    let result = driver.event(self, event);
                    native.pending_keysym.take();
                    result?;
                }
                native_count += 1;
            }
            let drag_deadline = drag::tick(native, Instant::now(), |event| driver.event(self, event))?;
            let state = driver.idle(self)?;
            if state.exit {
                return Ok(());
            }
            native.conn.flush().map_err(|error| error.to_string())?;
            if native_count == BATCH_LIMIT || worker_count == BATCH_LIMIT {
                continue;
            }
            // Synchronous X11 requests made while running idle can queue additional events.
            queued = native.conn.poll_for_event().map_err(|error| error.to_string())?;
            if queued.is_some() {
                continue;
            }
            let deadline = state.deadline.into_iter().chain(drag_deadline).min();
            let timeout = deadline.map(|deadline| {
                let delay = deadline.saturating_duration_since(Instant::now());
                Timespec { tv_sec: i64::try_from(delay.as_secs()).unwrap_or(i64::MAX), tv_nsec: delay.subsec_nanos().into() }
            });
            let mut fds = [PollFd::new(&native.conn, PollFlags::IN), PollFd::new(&inbox.pipe, PollFlags::IN)];
            match poll(&mut fds, timeout.as_ref()) {
                Ok(_) | Err(rustix::io::Errno::INTR) => {}
                Err(error) => return Err(error.to_string()),
            }
        }
    }
}

struct Inbox {
    pipe: UnixStream,
    events: Receiver<PlatformEvent>,
    wake_pending: Arc<AtomicBool>,
}

impl Inbox {
    fn new() -> std::io::Result<(PlatformSender, Self)> {
        let (pipe, signal) = UnixStream::pair()?;
        pipe.set_nonblocking(true)?;
        signal.set_nonblocking(true)?;
        let (tx, events) = mpsc::channel();
        let wake_pending = Arc::new(AtomicBool::new(false));
        let pending = wake_pending.clone();
        let sender = PlatformSender::new(move |event| {
            if matches!(event, PlatformEvent::UserWake) && pending.swap(true, Ordering::AcqRel) {
                return;
            }
            if tx.send(event).is_ok() {
                // A full socket already makes the receiver readable.
                while (&signal).write(&[1]).is_err_and(|error| error.kind() == std::io::ErrorKind::Interrupted) {}
            }
        });
        Ok((sender, Self { pipe, events, wake_pending }))
    }

    fn drain_pipe(&mut self) -> std::io::Result<()> {
        let mut bytes = [0; 4096];
        loop {
            match self.pipe.read(&mut bytes) {
                Ok(_) => return Ok(()),
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => return Ok(()),
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
                Err(error) => return Err(error),
            }
        }
    }
}
