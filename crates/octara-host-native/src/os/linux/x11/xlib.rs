//! The Xlib ABI needed by the native surface and keyboard fallback.

use super::XResult;
use libloading::Library;
use std::ffi::{c_char, c_int, c_uint, c_ulong, c_void};
use std::ptr::{NonNull, null};
use std::sync::OnceLock;
use x11rb::protocol::xproto::{KeyButMask, MAPPING_NOTIFY_EVENT, MappingNotifyEvent};
use x11rb::xcb_ffi::XCBConnection;

const XCB_OWNS_EVENT_QUEUE: c_int = 1;

#[repr(C)]
struct XMappingEvent {
    type_: c_int,
    serial: c_ulong,
    send_event: c_int,
    display: *mut c_void,
    window: c_ulong,
    request: c_int,
    first_keycode: c_int,
    count: c_int,
}

struct Xlib {
    open_display: unsafe extern "C" fn(*const c_char) -> *mut c_void,
    close_display: unsafe extern "C" fn(*mut c_void) -> c_int,
    default_screen: unsafe extern "C" fn(*mut c_void) -> c_int,
    get_xcb_connection: unsafe extern "C" fn(*mut c_void) -> *mut c_void,
    set_event_queue_owner: unsafe extern "C" fn(*mut c_void, c_int),
    lookup_keysym: unsafe extern "C" fn(*mut c_void, u8, c_uint, *mut c_uint, *mut c_ulong) -> c_int,
    refresh_keyboard_mapping: unsafe extern "C" fn(*mut XMappingEvent) -> c_int,
    _x11_xcb: Library,
    _x11: Library,
}

impl Xlib {
    fn load() -> XResult<Self> {
        // SAFETY: These system libraries expose the named functions with the C
        // signatures above. Retain both libraries for the lifetime of the pointers.
        unsafe {
            let x11 = Library::new("libX11.so.6").or_else(|_| Library::new("libX11.so"))?;
            let x11_xcb = Library::new("libX11-xcb.so.1").or_else(|_| Library::new("libX11-xcb.so"))?;
            let init_threads = x11.get::<unsafe extern "C" fn() -> c_int>(b"XInitThreads\0")?;
            if init_threads() == 0 {
                return Err("Xlib thread initialization failed".into());
            }
            Ok(Self {
                open_display: *x11.get(b"XOpenDisplay\0")?,
                close_display: *x11.get(b"XCloseDisplay\0")?,
                default_screen: *x11.get(b"XDefaultScreen\0")?,
                get_xcb_connection: *x11_xcb.get(b"XGetXCBConnection\0")?,
                set_event_queue_owner: *x11_xcb.get(b"XSetEventQueueOwner\0")?,
                lookup_keysym: *x11.get(b"XkbLookupKeySym\0")?,
                refresh_keyboard_mapping: *x11.get(b"XRefreshKeyboardMapping\0")?,
                _x11_xcb: x11_xcb,
                _x11: x11,
            })
        }
    }
}

pub(super) struct Display {
    xlib: &'static Xlib,
    ptr: NonNull<c_void>,
}

impl Display {
    pub(super) fn open() -> XResult<(Self, usize)> {
        static XLIB: OnceLock<Result<Xlib, String>> = OnceLock::new();
        let xlib = XLIB.get_or_init(|| Xlib::load().map_err(|error| error.to_string())).as_ref().map_err(String::as_str)?;
        // SAFETY: Xlib was initialized for shared GPU and UI use. A null name
        // requests the default display. The Display closes it, including on errors.
        let ptr = NonNull::new(unsafe { (xlib.open_display)(null()) }).ok_or("Could not open the X11 display")?;
        let display = Self { xlib, ptr };
        // SAFETY: The display is live.
        let screen = usize::try_from(unsafe { (xlib.default_screen)(ptr.as_ptr()) })?;
        Ok((display, screen))
    }

    /// The caller must keep the display alive until the sole XCB wrapper is dropped.
    pub(super) unsafe fn borrow_xcb(&self) -> XResult<XCBConnection> {
        // SAFETY: The display owns the connection. XCB alone consumes the event queue
        // after the transfer, and the caller keeps the display alive for the wrapper.
        unsafe {
            let raw = NonNull::new((self.xlib.get_xcb_connection)(self.ptr.as_ptr())).ok_or("Xlib did not provide an XCB connection")?;
            (self.xlib.set_event_queue_owner)(self.ptr.as_ptr(), XCB_OWNS_EVENT_QUEUE);
            Ok(XCBConnection::from_raw_xcb_connection(raw.as_ptr(), false)?)
        }
    }

    pub(super) fn as_ptr(&self) -> NonNull<c_void> {
        self.ptr
    }

    pub(super) fn lookup_keysym(&self, keycode: u8, state: KeyButMask) -> u32 {
        let mut modifiers = 0;
        let mut keysym = 0;
        // SAFETY: The display is live and both XKB outputs point to writable values.
        unsafe { (self.xlib.lookup_keysym)(self.ptr.as_ptr(), keycode, u16::from(state).into(), &mut modifiers, &mut keysym) };
        keysym as u32
    }

    pub(super) fn refresh_keyboard_mapping(&self, event: MappingNotifyEvent) {
        let mut event = XMappingEvent {
            type_: MAPPING_NOTIFY_EVENT.into(),
            serial: 0,
            send_event: 0,
            display: self.ptr.as_ptr(),
            window: 0,
            request: u8::from(event.request).into(),
            first_keycode: event.first_keycode.into(),
            count: event.count.into(),
        };
        // SAFETY: This event mirrors the mapping notification from this live display.
        unsafe { (self.xlib.refresh_keyboard_mapping)(&mut event) };
    }
}

impl Drop for Display {
    fn drop(&mut self) {
        // SAFETY: Display owns the connection and its XCB wrapper has been dropped.
        unsafe { (self.xlib.close_display)(self.ptr.as_ptr()) };
    }
}
