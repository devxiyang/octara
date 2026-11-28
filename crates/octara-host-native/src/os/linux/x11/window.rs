use super::clipboard::Clipboard;
use super::{Native, XResult, drag, input};
use crate::os::linux::accessibility::Accessibility;
use crate::os::linux::compose::Compose;
use crate::os::{
    ActiveEditorView, OsWindow, OsWindowContext, PlatformEvent, PlatformSender, ServiceCleanup, WindowAccessibility, WindowHostEvent,
    WindowTextInput,
};
use crate::text_bridge::HostTextBridge;
use octara::WindowOptions;
use octara::module::{HostSurface, HostSurfaceKind, RawNativeSurface};
use octara::runtime::WindowId;
use octara_model::ask::{ServiceId, ServiceRequest};
use octara_model::geometry::{Point, Rect, Size, WindowViewport};
use octara_model::input::{CursorStyle, DragItem};
use octara_model::theme::ColorScheme;
use octara_model::window::{
    StatusBarStyle, TitlebarStyle, UserAttention, WindowGeometry, WindowGeometryRequest, WindowLevel, WindowMaterial,
};
use raw_window_handle::{RawDisplayHandle, RawWindowHandle, XlibDisplayHandle, XlibWindowHandle};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use x11rb::connection::Connection;
use x11rb::properties::{WmHints, WmSizeHints, WmSizeHintsSpecification};
use x11rb::protocol::render;
use x11rb::protocol::xproto::{
    Atom, AtomEnum, ChangeWindowAttributesAux, ClientMessageEvent, Colormap, ColormapAlloc, ConfigureWindowAux, ConnectionExt,
    CreateWindowAux, Cursor, EventMask, Gravity, InputFocus, PropMode, Visualid, Window, WindowClass,
};
use x11rb::wrapper::ConnectionExt as _;

pub(super) struct WindowState {
    pub id: WindowId,
    pub xid: Window,
    pub size: Cell<[u32; 2]>,
    pub focused: Cell<bool>,
    pub destroyed: Cell<bool>,
    pub mapped: Cell<bool>,
    pub primary_down: Cell<bool>,
    pub pointer: Cell<Option<input::PointerPosition>>,
    pub target: RefCell<Option<drag::DragTarget>>,
    accessibility: RefCell<Accessibility>,
    urgent: Cell<bool>,
    options: WindowOptions,
}

impl WindowState {
    pub fn update_size_hints(&self, native: &Native) {
        let scale = native.scale.get();
        let options = &self.options;
        let min = if options.resizable { options.min_size } else { Some(options.viewport.size) };
        let max = if options.resizable { options.max_size } else { Some(options.viewport.size) };
        let hints = WmSizeHints {
            min_size: min.map(|size| hint_size(size, scale)),
            max_size: max.map(|size| hint_size(size, scale)),
            position: options.position.map(|point| {
                (
                    WmSizeHintsSpecification::ProgramSpecified,
                    i32::from(pixel_coordinate(point.x, scale)),
                    i32::from(pixel_coordinate(point.y, scale)),
                )
            }),
            ..WmSizeHints::default()
        };
        let _ = hints.set_normal_hints(&native.conn, self.xid);
    }

    pub fn update_accessibility(&self, native: &Native) {
        let Some((outer, inner)) = self.bounds(native) else { return };
        self.accessibility.borrow_mut().set_bounds(outer, inner);
    }

    pub fn show_without_window_manager(&self, native: &Native) {
        if self.options.non_activating_panel {
            return;
        }
        let conn = &native.conn;
        let unmanaged = conn
            .get_window_attributes(native.root)
            .ok()
            .and_then(|cookie| cookie.reply().ok())
            .is_some_and(|attributes| !attributes.all_event_masks.contains(EventMask::SUBSTRUCTURE_REDIRECT));
        if unmanaged {
            let _ = conn.set_input_focus(InputFocus::PARENT, self.xid, native.timestamp.get());
        }
    }

    pub fn set_focused(&self, native: &Native, focused: bool) {
        self.focused.set(focused);
        self.accessibility.borrow_mut().set_focused(focused);
        if focused && self.urgent.get() {
            self.set_urgency(native, false);
        }
    }

    fn set_urgency(&self, native: &Native, urgent: bool) {
        self.urgent.set(urgent);
        let hints = WmHints { input: Some(!self.options.non_activating_panel), urgent, ..WmHints::default() };
        let _ = hints.set(&native.conn, self.xid);
    }

    fn bounds(&self, native: &Native) -> Option<(Rect, Rect)> {
        let coordinates = native.conn.translate_coordinates(self.xid, native.root, 0, 0).ok()?;
        let extents = native.property32(self.xid, native.atoms._NET_FRAME_EXTENTS, AtomEnum::CARDINAL, 4).unwrap_or_default();
        let coordinates = coordinates.reply().ok()?;
        if !coordinates.same_screen {
            return None;
        }
        let [width, height] = self.size.get().map(|value| value as f32);
        // AT-SPI coordinates use the same screen pixels as the X11 properties.
        let x = f32::from(coordinates.dst_x);
        let y = f32::from(coordinates.dst_y);
        let inner = Rect::new(x, y, width, height);
        if let [left, right, top, bottom] = extents.as_slice() {
            let [left, right, top, bottom] = [*left, *right, *top, *bottom].map(|value| value as f32);
            let outer = Rect::new(x - left, y - top, width + left + right, height + top + bottom);
            return Some((outer, inner));
        }
        let parent = native.conn.query_tree(self.xid).ok()?.reply().ok()?.parent;
        if parent == native.root {
            return Some((inner, inner));
        }
        let frame = native.conn.get_geometry(parent).ok()?.reply().ok()?;
        let frame_position = native.conn.translate_coordinates(parent, native.root, 0, 0).ok()?.reply().ok()?;
        let outer =
            Rect::new(f32::from(frame_position.dst_x), f32::from(frame_position.dst_y), f32::from(frame.width), f32::from(frame.height));
        Some((outer, inner))
    }
}

pub(super) struct X11Window {
    native: Rc<Native>,
    state: Rc<WindowState>,
    sender: PlatformSender,
    clipboard: Rc<Clipboard>,
    visual: Visualid,
    colormap: Colormap,
    cursor: Cursor,
    cursor_style: CursorStyle,
    level: Option<WindowLevel>,
    editor: HostTextBridge,
    compose: Option<Compose>,
    shown: bool,
    closed: bool,
}

impl X11Window {
    pub fn new(
        native: Rc<Native>,
        clipboard: Rc<Clipboard>,
        id: WindowId,
        options: &WindowOptions,
        context: OsWindowContext<'_>,
    ) -> XResult<Self> {
        let conn = &native.conn;
        let screen = native.screen();
        let xid = conn.generate_id()?;
        let scale = native.scale.get();
        let [width, height] = pixel_size(options.viewport.size, scale);
        let sender = context.sender;
        let (depth, visual) = if context.transparent {
            alpha_visual(&native).unwrap_or((screen.root_depth, screen.root_visual))
        } else {
            (screen.root_depth, screen.root_visual)
        };
        let colormap = conn.generate_id()?;
        conn.create_colormap(ColormapAlloc::NONE, colormap, native.root, visual)?.check()?;
        let state = Rc::new(WindowState {
            id,
            xid,
            size: Cell::new([u32::from(width), u32::from(height)]),
            focused: Cell::new(false),
            destroyed: Cell::new(false),
            mapped: Cell::new(false),
            primary_down: Cell::new(false),
            pointer: Cell::new(None),
            target: RefCell::new(None),
            accessibility: RefCell::new(Accessibility::new(id, sender.clone())),
            urgent: Cell::new(false),
            options: options.clone(),
        });
        let mut window = Self {
            native,
            state,
            sender,
            clipboard,
            visual,
            colormap,
            cursor: x11rb::NONE,
            cursor_style: CursorStyle::Arrow,
            level: None,
            editor: HostTextBridge::default(),
            compose: Compose::new(),
            shown: false,
            closed: false,
        };
        window.create_x_window(depth, [width, height])?;
        window.native.windows.borrow_mut().insert(xid, Rc::downgrade(&window.state));
        Ok(window)
    }

    fn create_x_window(&mut self, depth: u8, [width, height]: [u16; 2]) -> XResult<()> {
        let native = &self.native;
        let conn = &native.conn;
        let atoms = &native.atoms;
        let xid = self.state.xid;
        let options = &self.state.options;
        let scale = native.scale.get();
        let position = options.position.unwrap_or_default();
        let events = EventMask::EXPOSURE
            | EventMask::STRUCTURE_NOTIFY
            | EventMask::FOCUS_CHANGE
            | EventMask::PROPERTY_CHANGE
            | EventMask::ENTER_WINDOW
            | EventMask::LEAVE_WINDOW
            | EventMask::POINTER_MOTION
            | EventMask::BUTTON_PRESS
            | EventMask::BUTTON_RELEASE
            | EventMask::KEY_PRESS
            | EventMask::KEY_RELEASE;
        let attributes = CreateWindowAux::new().event_mask(events).colormap(self.colormap).border_pixel(0).background_pixel(0);
        let created = conn
            .create_window(
                depth,
                xid,
                native.root,
                pixel_coordinate(position.x, scale),
                pixel_coordinate(position.y, scale),
                width,
                height,
                0,
                WindowClass::INPUT_OUTPUT,
                self.visual,
                &attributes,
            )?
            .check();
        if let Err(error) = created {
            self.state.destroyed.set(true);
            return Err(error.into());
        }
        let hints = WmHints { input: Some(!options.non_activating_panel), ..WmHints::default() };
        hints.set(conn, xid)?;
        conn.change_property32(PropMode::REPLACE, xid, atoms.WM_PROTOCOLS, AtomEnum::ATOM, &[atoms.WM_DELETE_WINDOW])?;
        conn.change_property32(PropMode::REPLACE, xid, atoms._NET_WM_PID, AtomEnum::CARDINAL, &[std::process::id()])?;
        conn.change_property32(PropMode::REPLACE, xid, atoms.XdndAware, AtomEnum::ATOM, &[drag::VERSION])?;
        // WM_CLASS contains two consecutive NUL-terminated strings.
        let class =
            std::env::current_exe().ok().and_then(|path| path.file_name()?.to_str().map(str::to_owned)).unwrap_or_else(|| "octara".into());
        let wm_class = format!("{class}\0{class}\0");
        conn.change_property8(PropMode::REPLACE, xid, AtomEnum::WM_CLASS, AtomEnum::STRING, wm_class.as_bytes())?;
        self.state.update_size_hints(native);
        if options.maximized {
            self.change_state(true, &[atoms._NET_WM_STATE_MAXIMIZED_VERT, atoms._NET_WM_STATE_MAXIMIZED_HORZ]);
        }
        Ok(())
    }

    fn message(&self, kind: Atom, data: [u32; 5]) {
        let native = &self.native;
        let event = ClientMessageEvent::new(32, self.state.xid, kind, data);
        let _ = native.conn.send_event(false, native.root, EventMask::SUBSTRUCTURE_NOTIFY | EventMask::SUBSTRUCTURE_REDIRECT, event);
    }

    fn states(&self) -> Vec<Atom> {
        let native = &self.native;
        native.property32(self.state.xid, native.atoms._NET_WM_STATE, AtomEnum::ATOM, 128).unwrap_or_default()
    }

    fn change_state(&self, enabled: bool, atoms: &[Atom]) {
        let native = &self.native;
        let property = native.atoms._NET_WM_STATE;
        if self.shown {
            for atoms in atoms.chunks(2) {
                self.message(property, [u32::from(enabled), atoms[0], atoms.get(1).copied().unwrap_or(x11rb::NONE), 1, 0]);
            }
        } else {
            let mut states = self.states();
            states.retain(|atom| !atoms.contains(atom));
            if enabled {
                states.extend_from_slice(atoms);
            }
            let _ = native.conn.change_property32(PropMode::REPLACE, self.state.xid, property, AtomEnum::ATOM, &states);
        }
    }

    fn set_level(&mut self, level: WindowLevel) {
        if self.level.replace(level) == Some(level) {
            return;
        }
        let native = &self.native;
        let atoms = &native.atoms;
        let panel = self.state.options.non_activating_panel;
        // EWMH has one above-normal layer. Utility type also identifies a floating palette.
        let utility = panel || level == WindowLevel::Floating;
        let kind = if utility { atoms._NET_WM_WINDOW_TYPE_UTILITY } else { atoms._NET_WM_WINDOW_TYPE_NORMAL };
        let _ = native.conn.change_property32(PropMode::REPLACE, self.state.xid, atoms._NET_WM_WINDOW_TYPE, AtomEnum::ATOM, &[kind]);
        self.change_state(level != WindowLevel::Normal, &[atoms._NET_WM_STATE_ABOVE]);
        if panel && !self.shown {
            self.change_state(true, &[atoms._NET_WM_STATE_SKIP_TASKBAR, atoms._NET_WM_STATE_SKIP_PAGER]);
        }
    }

    fn free_cursor(&mut self) {
        let cursor = std::mem::replace(&mut self.cursor, x11rb::NONE);
        if cursor != x11rb::NONE {
            let _ = self.native.conn.free_cursor(cursor);
        }
    }
}

impl Drop for X11Window {
    fn drop(&mut self) {
        self.native.unregister(self.state.xid);
        self.free_cursor();
        if !self.state.destroyed.get() {
            let _ = self.native.conn.destroy_window(self.state.xid);
        }
        let _ = self.native.conn.free_colormap(self.colormap);
        let _ = self.native.conn.flush();
    }
}

// SAFETY: This window owns the XID and retains the Xlib display. Closing it unmaps
// the window and unregisters it from event routing. Native handles are kept until drop.
unsafe impl OsWindow for X11Window {
    fn raw_handles(&self) -> Result<(RawDisplayHandle, RawWindowHandle), String> {
        if self.state.destroyed.get() {
            return Err("The X11 window has been destroyed".into());
        }
        let display = XlibDisplayHandle::new(Some(self.native.display.ptr.cast()), self.native.screen as i32);
        let mut window = XlibWindowHandle::new(self.state.xid.into());
        window.visual_id = self.visual.into();
        Ok((display.into(), window.into()))
    }

    fn host_surface(&self) -> HostSurface<'_> {
        let surface = HostSurface::new(HostSurfaceKind::Linux, Some(self.state.id));
        if self.state.destroyed.get() {
            return surface;
        }
        let handles = RawNativeSurface::Xlib { display: self.native.display.ptr.cast(), window: self.state.xid.into() };
        // SAFETY: The display and window belong to this X11Window.
        unsafe { surface.with_raw_native(handles) }
    }

    fn physical_size(&self) -> [u32; 2] {
        if self.state.destroyed.get() { [0, 0] } else { self.state.size.get() }
    }

    fn scale_factor(&self) -> f32 {
        self.native.scale.get()
    }

    fn request_redraw(&mut self) {
        self.sender.send_event(PlatformEvent::WindowRedrawRequested(self.state.id));
    }

    fn is_focused(&self) -> bool {
        self.state.focused.get()
    }

    fn set_cursor(&mut self, style: CursorStyle) {
        if self.cursor_style == style {
            return;
        }
        let native = &self.native;
        let xid = self.state.xid;
        let cursor = if style == CursorStyle::None {
            hidden_cursor(native, xid).ok()
        } else {
            native.cursor.as_ref().and_then(|handle| handle.load_cursor(&native.conn, cursor_name(style)).ok())
        };
        let Some(cursor) = cursor else { return };
        let _ = native.conn.change_window_attributes(xid, &ChangeWindowAttributesAux::new().cursor(cursor));
        self.free_cursor();
        self.cursor = cursor;
        self.cursor_style = style;
    }

    fn request_user_attention(&mut self, attention: UserAttention) {
        let native = &self.native;
        self.change_state(true, &[native.atoms._NET_WM_STATE_DEMANDS_ATTENTION]);
        if attention == UserAttention::Critical {
            self.state.set_urgency(native, true);
        }
    }

    fn apply_presentation(
        &mut self,
        title: &str,
        scheme: ColorScheme,
        _material: WindowMaterial,
        _status: StatusBarStyle,
        level: WindowLevel,
    ) {
        let native = &self.native;
        let conn = &native.conn;
        let atoms = &native.atoms;
        let xid = self.state.xid;
        let _ = conn.change_property8(PropMode::REPLACE, xid, atoms._NET_WM_NAME, atoms.UTF8_STRING, title.as_bytes());
        let legacy: Vec<_> = title.chars().map(|character| u8::try_from(u32::from(character)).unwrap_or(b'?')).collect();
        let _ = conn.change_property8(PropMode::REPLACE, xid, AtomEnum::WM_NAME, AtomEnum::STRING, &legacy);
        let variant: &[u8] = if scheme == ColorScheme::Dark { b"dark" } else { b"light" };
        let _ = conn.change_property8(PropMode::REPLACE, xid, atoms._GTK_THEME_VARIANT, atoms.UTF8_STRING, variant);
        self.set_level(level);
    }

    fn configure_chrome(&mut self, options: &WindowOptions) {
        let atoms = &self.native.atoms;
        match options.titlebar {
            TitlebarStyle::System => {
                let _ = self.native.conn.delete_property(self.state.xid, atoms._MOTIF_WM_HINTS);
            }
            TitlebarStyle::HiddenInset { .. } => {
                let _ = self.native.conn.change_property32(
                    PropMode::REPLACE,
                    self.state.xid,
                    atoms._MOTIF_WM_HINTS,
                    atoms._MOTIF_WM_HINTS,
                    &[2, 0, 0, 0, 0],
                );
            }
        }
    }

    fn window_geometry(&mut self, viewport: WindowViewport) -> WindowGeometry {
        let native = &self.native;
        let atoms = &native.atoms;
        let scale = native.scale.get();
        let states = self.states();
        let position = self.state.bounds(native).map(|(outer, inner)| {
            self.state.accessibility.borrow_mut().set_bounds(outer, inner);
            Point::new(outer.x / scale, outer.y / scale)
        });
        WindowGeometry {
            position,
            size: viewport.size,
            scale,
            maximized: states.contains(&atoms._NET_WM_STATE_MAXIMIZED_VERT) && states.contains(&atoms._NET_WM_STATE_MAXIMIZED_HORZ),
            fullscreen: states.contains(&atoms._NET_WM_STATE_FULLSCREEN),
        }
    }

    fn set_window_geometry(&mut self, geometry: WindowGeometryRequest) {
        let native = &self.native;
        let atoms = &native.atoms;
        let scale = native.scale.get();
        let fullscreen = [atoms._NET_WM_STATE_FULLSCREEN];
        let maximized = [atoms._NET_WM_STATE_MAXIMIZED_VERT, atoms._NET_WM_STATE_MAXIMIZED_HORZ];
        if geometry.fullscreen == Some(false) {
            self.change_state(false, &fullscreen);
        }
        if geometry.maximized == Some(false) {
            self.change_state(false, &maximized);
        }
        let position = geometry.position.filter(|point| point.x.is_finite() && point.y.is_finite());
        let size = geometry.size.filter(|size| size.width.is_finite() && size.height.is_finite() && size.width > 0.0 && size.height > 0.0);
        if position.is_some() || size.is_some() {
            let (x, y) = position.map_or((0, 0), |point| (pixel_coordinate(point.x, scale), pixel_coordinate(point.y, scale)));
            let [width, height] = size.map_or([0, 0], |size| pixel_size(size, scale));
            if self.shown && native.supports(atoms._NET_MOVERESIZE_WINDOW) {
                let flags = u32::from(Gravity::NORTH_WEST)
                    | (u32::from(position.is_some()) * 0x300)
                    | (u32::from(size.is_some()) * 0xc00)
                    | (1 << 12);
                self.message(atoms._NET_MOVERESIZE_WINDOW, [flags, i32::from(x) as u32, i32::from(y) as u32, width.into(), height.into()]);
            } else {
                let mut values = ConfigureWindowAux::new();
                if position.is_some() {
                    values = values.x(i32::from(x)).y(i32::from(y));
                }
                if size.is_some() {
                    values = values.width(u32::from(width)).height(u32::from(height));
                }
                let _ = native.conn.configure_window(self.state.xid, &values);
            }
        }
        if geometry.maximized == Some(true) {
            self.change_state(true, &maximized);
        }
        if geometry.fullscreen == Some(true) {
            self.change_state(true, &fullscreen);
        }
    }

    fn focus(&mut self) {
        let native = &self.native;
        if self.state.options.non_activating_panel || self.closed {
            return;
        }
        let time = native.timestamp.get();
        if native.supports(native.atoms._NET_ACTIVE_WINDOW) {
            self.message(native.atoms._NET_ACTIVE_WINDOW, [1, time, 0, 0, 0]);
        } else if self.state.mapped.get() {
            let _ = native.conn.set_input_focus(InputFocus::PARENT, self.state.xid, time);
        }
    }

    fn minimize(&mut self) {
        // ICCCM IconicState.
        self.message(self.native.atoms.WM_CHANGE_STATE, [3, 0, 0, 0, 0]);
    }

    fn toggle_maximize(&mut self) {
        let atoms = &self.native.atoms;
        if self.shown {
            self.message(atoms._NET_WM_STATE, [2, atoms._NET_WM_STATE_MAXIMIZED_VERT, atoms._NET_WM_STATE_MAXIMIZED_HORZ, 1, 0]);
        } else {
            let maximized = self.states().contains(&atoms._NET_WM_STATE_MAXIMIZED_VERT);
            self.change_state(!maximized, &[atoms._NET_WM_STATE_MAXIMIZED_VERT, atoms._NET_WM_STATE_MAXIMIZED_HORZ]);
        }
    }

    fn start_window_drag(&mut self) {
        let native = &self.native;
        let Some(pointer) = self.state.pointer.get().filter(|_| self.state.primary_down.get()) else { return };
        if !native.supports(native.atoms._NET_WM_MOVERESIZE) {
            return;
        }
        self.state.primary_down.set(false);
        let _ = native.conn.ungrab_pointer(native.timestamp.get());
        self.message(native.atoms._NET_WM_MOVERESIZE, [i32::from(pointer.root_x) as u32, i32::from(pointer.root_y) as u32, 8, 1, 1]);
    }

    fn start_drag_out(&mut self, items: Vec<DragItem>) -> bool {
        let native = &self.native;
        let mut source = native.source.borrow_mut();
        source.start(native, &self.state, &items).unwrap_or(false)
    }

    fn show(&mut self) {
        if self.closed || self.shown || self.state.destroyed.get() {
            return;
        }
        self.message(self.native.atoms._NET_REQUEST_FRAME_EXTENTS, [0; 5]);
        let _ = self.native.conn.map_window(self.state.xid);
        self.shown = true;
    }

    fn close(&mut self) {
        if self.closed {
            return;
        }
        self.closed = true;
        self.native.unregister(self.state.xid);
        if !self.state.destroyed.get() {
            let _ = self.native.conn.unmap_window(self.state.xid);
        }
    }

    fn text_input(&mut self) -> &mut dyn WindowTextInput {
        self
    }

    fn accessibility(&mut self) -> Option<&mut dyn WindowAccessibility> {
        Some(self)
    }

    fn request_service(&mut self, id: ServiceId, request: ServiceRequest) -> Option<ServiceCleanup> {
        let sender = self.sender.clone();
        let window = self.state.id;
        self.clipboard.request(id, request, move |reply| crate::os::reply(&sender, window, reply))
    }
}

impl WindowTextInput for X11Window {
    fn sync_text_input(&mut self, session: Option<&ActiveEditorView<'_>>) {
        let outcome = self.editor.sync(session);
        if (outcome.session_changed || outcome.rejected)
            && let Some(compose) = &self.compose
        {
            compose.reset();
        }
    }

    fn is_composing(&self) -> bool {
        self.compose.as_ref().is_some_and(Compose::is_composing)
    }

    fn on_unhandled_key(&mut self, _: &crate::os::PlatformKey) -> bool {
        let Some((window, symbol)) = self.native.pending_keysym.get() else { return false };
        window == self.state.id && self.compose.as_ref().is_some_and(|compose| compose.dispatch(symbol, &mut self.editor))
    }

    fn focused(&mut self, active: bool) {
        if !active && let Some(compose) = &self.compose {
            compose.reset();
        }
    }

    fn drain_text_input(&mut self, out: &mut Vec<WindowHostEvent>) {
        self.editor.drain(out);
    }

    fn inspect_text_input(&self) -> serde_json::Value {
        serde_json::json!({ "allowed": self.editor.session().is_some(), "native_focus": self.is_focused(), "has_marked": false })
    }
}

impl WindowAccessibility for X11Window {
    fn synchronize(&mut self, publication: &octara_pipeline::NativePublication) {
        self.state.accessibility.borrow_mut().synchronize(publication);
    }

    fn clear(&mut self) {
        self.state.accessibility.borrow_mut().clear();
    }

    fn accepts_action(
        &self,
        publication: &octara_pipeline::NativePublication,
        target: octara_model::identity::TargetGuard,
        action: &octara_model::semantics::A11yAction,
    ) -> bool {
        self.state.accessibility.borrow().accepts_action(publication, target, action)
    }
}

fn alpha_visual(native: &Native) -> Option<(u8, Visualid)> {
    let formats = render::query_pict_formats(&native.conn).ok()?.reply().ok()?;
    let depths = &formats.screens.get(native.screen)?.depths;
    let visuals = &depths.iter().find(|depth| depth.depth == 32)?.visuals;
    let visual = visuals.iter().find(|visual| {
        formats
            .formats
            .iter()
            .any(|format| format.id == visual.format && format.type_ == render::PictType::DIRECT && format.direct.alpha_mask != 0)
    })?;
    Some((32, visual.visual))
}

fn pixel_size(size: Size, scale: f32) -> [u16; 2] {
    [size.width, size.height].map(|value| ((value * scale).round() as u16).max(1))
}

fn hint_size(size: Size, scale: f32) -> (i32, i32) {
    let [width, height] = pixel_size(size, scale);
    (i32::from(width), i32::from(height))
}

fn pixel_coordinate(value: f32, scale: f32) -> i16 {
    (value * scale).round() as i16
}

fn hidden_cursor(native: &Native, xid: Window) -> XResult<Cursor> {
    let conn = &native.conn;
    let pixmap = conn.generate_id()?;
    let gc = conn.generate_id()?;
    let cursor = conn.generate_id()?;
    conn.create_pixmap(1, pixmap, xid, 1, 1)?;
    conn.create_gc(gc, pixmap, &x11rb::protocol::xproto::CreateGCAux::new().foreground(0))?;
    conn.poly_fill_rectangle(pixmap, gc, &[x11rb::protocol::xproto::Rectangle { x: 0, y: 0, width: 1, height: 1 }])?;
    let result = conn.create_cursor(cursor, pixmap, pixmap, 0, 0, 0, 0, 0, 0, 0, 0)?.check();
    let _ = conn.free_gc(gc);
    let _ = conn.free_pixmap(pixmap);
    result?;
    Ok(cursor)
}

fn cursor_name(style: CursorStyle) -> &'static str {
    match style {
        CursorStyle::Arrow | CursorStyle::Auto | CursorStyle::None => "left_ptr",
        CursorStyle::IBeam => "text",
        CursorStyle::PointingHand => "pointer",
        CursorStyle::Crosshair => "crosshair",
        CursorStyle::OpenHand => "grab",
        CursorStyle::ClosedHand => "grabbing",
        CursorStyle::ResizeLeftRight => "ew-resize",
        CursorStyle::ResizeUpDown => "ns-resize",
        CursorStyle::ResizeColumn => "col-resize",
        CursorStyle::ResizeRow => "row-resize",
        CursorStyle::OperationNotAllowed => "not-allowed",
        CursorStyle::Wait => "wait",
        CursorStyle::Move => "move",
        CursorStyle::Help => "help",
        CursorStyle::ContextMenu => "context-menu",
        CursorStyle::Progress => "progress",
        CursorStyle::Cell => "cell",
        CursorStyle::VerticalText => "vertical-text",
        CursorStyle::Alias => "alias",
        CursorStyle::Copy => "copy",
        CursorStyle::NoDrop => "no-drop",
        CursorStyle::AllScroll => "all-scroll",
        CursorStyle::ResizeNorth => "n-resize",
        CursorStyle::ResizeEast => "e-resize",
        CursorStyle::ResizeSouth => "s-resize",
        CursorStyle::ResizeWest => "w-resize",
        CursorStyle::ResizeNorthEast => "ne-resize",
        CursorStyle::ResizeNorthWest => "nw-resize",
        CursorStyle::ResizeSouthEast => "se-resize",
        CursorStyle::ResizeSouthWest => "sw-resize",
        CursorStyle::ResizeNorthEastSouthWest => "nesw-resize",
        CursorStyle::ResizeNorthWestSouthEast => "nwse-resize",
        CursorStyle::ZoomIn => "zoom-in",
        CursorStyle::ZoomOut => "zoom-out",
    }
}
