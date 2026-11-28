use super::{Protocol, Shared};
use crate::os::{
    ActiveEditorView, OsWindow, PlatformEvent, PlatformSender, ServiceCleanup, WindowAccessibility, WindowHostEvent, WindowTextInput,
};
use octara::WindowOptions;
use octara::module::{HostSurface, HostSurfaceKind, RawNativeSurface};
use octara::runtime::WindowId;
use octara_model::ask::{ServiceId, ServiceRequest};
use octara_model::geometry::{Rect, Size, WindowViewport};
use octara_model::input::{CursorStyle, DragItem};
use octara_model::theme::ColorScheme;
use octara_model::window::{
    StatusBarStyle, TitlebarStyle, UserAttention, WindowGeometry, WindowGeometryRequest, WindowLevel, WindowMaterial,
};
use raw_window_handle::{RawDisplayHandle, RawWindowHandle, WaylandDisplayHandle, WaylandWindowHandle};
use smithay_client_toolkit::compositor::CompositorHandler;
use smithay_client_toolkit::shell::WaylandSurface;
use smithay_client_toolkit::shell::xdg::window::{Window, WindowConfigure, WindowDecorations, WindowHandler};
use std::cell::{Cell, RefCell};
use std::ptr::NonNull;
use std::rc::Rc;
use wayland_client::protocol::{wl_output, wl_surface};
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle, delegate_noop};
use wayland_protocols::wp::fractional_scale::v1::client::wp_fractional_scale_v1;
use wayland_protocols::wp::viewporter::client::wp_viewport;

use super::super::{accessibility::Accessibility, compose::Compose};
use crate::text_bridge::HostTextBridge;

pub(super) struct WindowState {
    pub id: WindowId,
    pub closed: Cell<bool>,
    pub native: Window,
    pub cursor: Cell<CursorStyle>,
    pub editor: RefCell<HostTextBridge>,
    pub compose: Option<Compose>,
    viewport: Option<wp_viewport::WpViewport>,
    fractional: Option<wp_fractional_scale_v1::WpFractionalScaleV1>,
    preferred_scale: Cell<Option<u32>>,
    integer_scale: Cell<i32>,
    logical_size: Cell<[u32; 2]>,
    windowed_size: Cell<[u32; 2]>,
    limits: Limits,
    configured: Cell<bool>,
    mapped: Cell<bool>,
    focused: Cell<bool>,
    maximized: Cell<bool>,
    fullscreen: Cell<bool>,
    waiting_for_frame: Cell<bool>,
    redraw_pending: Cell<bool>,
}

pub(super) struct WaylandWindow {
    native: Rc<WindowState>,
    shared: Rc<Shared>,
    sender: PlatformSender,
    accessibility: Accessibility,
    non_activating: bool,
}

struct Limits {
    min: [u32; 2],
    max: [u32; 2],
    resizable: bool,
}

impl Limits {
    fn new(options: &WindowOptions) -> Self {
        let min = logical_size(options.min_size).unwrap_or([0; 2]);
        let mut max = logical_size(options.max_size).unwrap_or([0; 2]);
        for index in 0..2 {
            if max[index] != 0 {
                max[index] = max[index].max(min[index]);
            }
        }
        Self { min, max, resizable: options.resizable }
    }

    fn constrain(&self, mut size: [u32; 2]) -> [u32; 2] {
        for (index, value) in size.iter_mut().enumerate() {
            *value = (*value).max(self.min[index]);
            if self.max[index] != 0 {
                *value = (*value).min(self.max[index]);
            }
        }
        size
    }

    fn apply(&self, native: &Window, size: [u32; 2]) {
        let (min, max) = if self.resizable { (self.min, self.max) } else { (size, size) };
        native.set_min_size(Some((min[0], min[1])));
        native.set_max_size(Some((max[0], max[1])));
    }
}

fn logical_size(size: Option<Size>) -> Option<[u32; 2]> {
    let size = size?;
    let dimensions = [size.width, size.height];
    dimensions
        .iter()
        .all(|value| value.is_finite() && *value > 0.0)
        .then(|| dimensions.map(|value| (value.round() as u32).clamp(1, i32::MAX as u32)))
}

fn buffer_pixels(logical: u32, scale: u64) -> u32 {
    (u64::from(logical).saturating_mul(scale).saturating_add(60) / 120).clamp(1, u64::from(u32::MAX)) as u32
}

impl WindowState {
    pub(super) fn scale(&self) -> f32 {
        self.preferred_scale.get().map_or_else(|| self.integer_scale.get() as f32, |scale| scale as f32 / 120.0)
    }

    fn physical_size(&self) -> [u32; 2] {
        if !self.configured.get() || self.closed.get() {
            return [0, 0];
        }
        let scale = self.preferred_scale.get().map(u64::from).unwrap_or_else(|| self.integer_scale.get() as u64 * 120);
        self.logical_size.get().map(|size| buffer_pixels(size, scale))
    }

    fn request_frame(&self, shared: &Shared) {
        if self.closed.get() || !self.configured.get() {
            return;
        }
        if !self.redraw_pending.replace(true) && !self.waiting_for_frame.get() {
            shared.emit(PlatformEvent::WindowRedrawRequested(self.id));
        }
    }
}

impl Drop for WindowState {
    fn drop(&mut self) {
        if let Some(viewport) = &self.viewport {
            viewport.destroy();
        }
        if let Some(fractional) = &self.fractional {
            fractional.destroy();
        }
    }
}

impl WaylandWindow {
    pub(super) fn new(shared: Rc<Shared>, id: WindowId, options: &WindowOptions, sender: PlatformSender) -> Self {
        let surface = shared.compositor.create_surface(&shared.queue);
        let decoration = match options.titlebar {
            TitlebarStyle::System => WindowDecorations::RequestServer,
            TitlebarStyle::HiddenInset { .. } => WindowDecorations::RequestClient,
        };
        let native = shared.shell.create_window(surface.clone(), decoration, &shared.queue);
        let title = options.presentation.title.replace('\0', "");
        native.set_title(title);
        let app_id =
            std::env::current_exe().ok().and_then(|path| path.file_name()?.to_str().map(str::to_owned)).unwrap_or_else(|| "octara".into());
        native.set_app_id(app_id);
        let limits = Limits::new(options);
        let size = limits.constrain(logical_size(Some(options.viewport.size)).unwrap_or([800, 600]));
        limits.apply(&native, size);
        if options.maximized {
            native.set_maximized();
        }

        let scale_protocols = shared.viewporter.as_ref().zip(shared.fractional.as_ref());
        let viewport = scale_protocols.map(|(manager, _)| manager.get_viewport(&surface, &shared.queue, ()));
        let fractional = scale_protocols.map(|(_, manager)| manager.get_fractional_scale(&surface, &shared.queue, surface.clone()));
        let native = Rc::new(WindowState {
            id,
            native,
            viewport,
            fractional,
            limits,
            logical_size: Cell::new(size),
            windowed_size: Cell::new(size),
            closed: Cell::new(false),
            cursor: Cell::default(),
            editor: RefCell::default(),
            compose: Compose::new(),
            preferred_scale: Cell::default(),
            integer_scale: Cell::new(1),
            configured: Cell::default(),
            mapped: Cell::default(),
            focused: Cell::default(),
            maximized: Cell::new(options.maximized),
            fullscreen: Cell::default(),
            waiting_for_frame: Cell::default(),
            redraw_pending: Cell::default(),
        });
        shared.windows.borrow_mut().insert(surface.id(), Rc::downgrade(&native));
        native.native.commit();
        Self { native, shared, accessibility: Accessibility::new(id, sender.clone()), sender, non_activating: options.non_activating_panel }
    }

    fn update_bounds(&mut self, size: Size) {
        let rect = Rect::new(0.0, 0.0, size.width, size.height);
        self.accessibility.set_bounds(rect, rect);
    }
}

impl Drop for WaylandWindow {
    fn drop(&mut self) {
        self.close();
    }
}

// SAFETY: The window owns its wl_surface and a reference to the connection. Closing
// unmaps the surface while preserving both handles until GPU work releases the window.
unsafe impl OsWindow for WaylandWindow {
    fn raw_handles(&self) -> Result<(RawDisplayHandle, RawWindowHandle), String> {
        let display = NonNull::new(self.shared.connection.backend().display_ptr().cast()).ok_or("Wayland display is unavailable")?;
        let surface = NonNull::new(self.native.native.wl_surface().id().as_ptr().cast()).ok_or("Wayland surface is unavailable")?;
        Ok((WaylandDisplayHandle::new(display).into(), WaylandWindowHandle::new(surface).into()))
    }

    fn host_surface(&self) -> HostSurface<'_> {
        let host = HostSurface::new(HostSurfaceKind::Linux, Some(self.native.id));
        let Ok((RawDisplayHandle::Wayland(display), RawWindowHandle::Wayland(window))) = self.raw_handles() else {
            return host;
        };
        // SAFETY: Both Wayland handles are borrowed from this window for the lifetime of the returned surface.
        unsafe { host.with_raw_native(RawNativeSurface::Wayland { display: display.display, surface: window.surface }) }
    }

    fn physical_size(&self) -> [u32; 2] {
        self.native.physical_size()
    }

    fn scale_factor(&self) -> f32 {
        self.native.scale()
    }

    fn request_redraw(&mut self) {
        self.native.request_frame(&self.shared);
    }

    fn pre_present_notify(&mut self) {
        let state = &self.native;
        let surface = state.native.wl_surface();
        if let Some(viewport) = &state.viewport
            && state.preferred_scale.get().is_some()
        {
            let [width, height] = state.logical_size.get();
            viewport.set_destination(width as i32, height as i32);
            surface.set_buffer_scale(1);
        } else {
            if let Some(viewport) = &state.viewport {
                viewport.set_destination(-1, -1);
            }
            if surface.version() >= 3 {
                surface.set_buffer_scale(state.integer_scale.get());
            }
        }
        state.redraw_pending.set(false);
        if !state.waiting_for_frame.replace(true) {
            surface.frame(&self.shared.queue, surface.clone());
        }
    }

    fn is_focused(&self) -> bool {
        self.native.focused.get()
    }

    fn apply_presentation(&mut self, title: &str, _: ColorScheme, _: WindowMaterial, _: StatusBarStyle, _: WindowLevel) {
        // xdg-shell leaves stacking order to the compositor.
        self.native.native.set_title(title.replace('\0', ""));
    }

    fn window_geometry(&mut self, viewport: WindowViewport) -> WindowGeometry {
        self.update_bounds(viewport.size);
        WindowGeometry {
            position: None,
            size: viewport.size,
            scale: viewport.scale,
            maximized: self.native.maximized.get(),
            fullscreen: self.native.fullscreen.get(),
        }
    }

    fn set_window_geometry(&mut self, geometry: WindowGeometryRequest) {
        let state = &self.native;
        if let Some(size) = logical_size(geometry.size).map(|size| state.limits.constrain(size)) {
            state.windowed_size.set(size);
            state.limits.apply(&state.native, size);
            if !state.maximized.get() && !state.fullscreen.get() && state.logical_size.replace(size) != size {
                self.shared.emit(PlatformEvent::WindowResized(state.id));
            }
        }
        if let Some(maximized) = geometry.maximized {
            if maximized {
                state.native.set_maximized();
            } else {
                state.native.unset_maximized();
            }
        }
        if let Some(fullscreen) = geometry.fullscreen {
            if fullscreen {
                state.native.set_fullscreen(None);
            } else {
                state.native.unset_fullscreen();
            }
        }
    }

    fn focus(&mut self) {
        if self.non_activating || self.native.focused.get() {
            return;
        }
        if let Some(activation) = &self.shared.activation {
            let token = activation.get_activation_token(&self.shared.queue, self.native.native.wl_surface().clone());
            if let Some((seat, serial, surface)) = self.shared.activation_serial() {
                token.set_serial(serial, &seat);
                token.set_surface(&surface);
            }
            token.commit();
        }
    }

    fn request_user_attention(&mut self, _: UserAttention) {
        self.focus();
    }

    fn minimize(&mut self) {
        self.native.native.set_minimized();
    }

    fn toggle_maximize(&mut self) {
        self.set_window_geometry(WindowGeometryRequest { maximized: Some(!self.native.maximized.get()), ..Default::default() });
    }

    fn start_window_drag(&mut self) {
        if let Some((seat, serial)) = self.shared.take_grab(self.native.id) {
            self.native.native.move_(&seat, serial);
        }
    }

    fn start_drag_out(&mut self, items: Vec<DragItem>) -> bool {
        self.shared.start_drag(&self.native, items)
    }

    fn show(&mut self) {
        self.native.mapped.set(true);
    }

    fn close(&mut self) {
        if self.native.closed.replace(true) {
            return;
        }
        let surface = self.native.native.wl_surface();
        self.shared.windows.borrow_mut().remove(&surface.id());
        self.shared.sync_ime(&self.native);
        if self.native.mapped.get() {
            surface.attach(None, 0, 0);
            surface.commit();
        }
    }

    fn set_cursor(&mut self, style: CursorStyle) {
        if self.native.cursor.replace(style) != style {
            self.shared.update_cursor(self.native.id);
        }
    }

    fn text_input(&mut self) -> &mut dyn WindowTextInput {
        self
    }

    fn accessibility(&mut self) -> Option<&mut dyn WindowAccessibility> {
        Some(&mut self.accessibility)
    }

    fn request_service(&mut self, id: ServiceId, request: ServiceRequest) -> Option<ServiceCleanup> {
        self.shared.clipboard_service(self.native.id, &self.sender, id, request)
    }
}

impl WindowTextInput for WaylandWindow {
    fn sync_text_input(&mut self, session: Option<&ActiveEditorView<'_>>) {
        if self.native.closed.get() {
            return;
        }
        let outcome = self.native.editor.borrow_mut().sync(session);
        if (outcome.session_changed || outcome.rejected)
            && let Some(compose) = &self.native.compose
        {
            compose.reset();
        }
        self.shared.sync_ime(&self.native);
    }

    fn is_composing(&self) -> bool {
        self.native.editor.borrow().is_composing() || self.native.compose.as_ref().is_some_and(Compose::is_composing)
    }

    fn on_unhandled_key(&mut self, _: &crate::os::PlatformKey) -> bool {
        let mut editor = self.native.editor.borrow_mut();
        if editor.is_composing() {
            if let Some(compose) = &self.native.compose {
                compose.reset();
            }
            return true;
        }
        let Some((window, symbol)) = self.shared.pending_keysym.get() else { return false };
        window == self.native.id && self.native.compose.as_ref().is_some_and(|compose| compose.dispatch(symbol, &mut editor))
    }

    fn focused(&mut self, active: bool) {
        self.accessibility.set_focused(active);
        if !active && let Some(compose) = &self.native.compose {
            compose.reset();
        }
    }

    fn drain_text_input(&mut self, out: &mut Vec<WindowHostEvent>) {
        self.native.editor.borrow_mut().drain(out);
    }

    fn inspect_text_input(&self) -> serde_json::Value {
        let editor = self.native.editor.borrow();
        serde_json::json!({
            "allowed": self.shared.text_input.is_some() && editor.session().is_some(),
            "native_focus": self.native.focused.get(),
            "has_marked": editor.is_composing(),
        })
    }
}

impl WindowHandler for Protocol {
    fn request_close(&mut self, _: &Connection, _: &QueueHandle<Self>, native: &Window) {
        if let Some(window) = self.shared.window(native.wl_surface()) {
            self.shared.emit(PlatformEvent::WindowCloseRequested(window.id));
        }
    }

    fn configure(&mut self, _: &Connection, _: &QueueHandle<Self>, native: &Window, configure: WindowConfigure, _: u32) {
        let Some(window) = self.shared.window(native.wl_surface()) else { return };
        let first = !window.configured.replace(true);
        let previous_maximized = window.maximized.replace(configure.is_maximized());
        let previous_fullscreen = window.fullscreen.replace(configure.is_fullscreen());
        let changed_mode = previous_maximized != configure.is_maximized() || previous_fullscreen != configure.is_fullscreen();
        let [previous_width, previous_height] = if !changed_mode && (configure.is_maximized() || configure.is_fullscreen()) {
            window.logical_size.get()
        } else {
            window.windowed_size.get()
        };
        let mut size = [configure.new_size.0.map_or(previous_width, u32::from), configure.new_size.1.map_or(previous_height, u32::from)];
        if !configure.is_maximized() && !configure.is_fullscreen() {
            if first && let Some((width, height)) = configure.suggested_bounds {
                if width > 0 {
                    size[0] = size[0].min(width);
                }
                if height > 0 {
                    size[1] = size[1].min(height);
                }
            }
            size = if window.limits.resizable { window.limits.constrain(size) } else { window.windowed_size.get() };
            window.windowed_size.set(size);
        }
        let resized = window.logical_size.replace(size) != size;
        if first || resized {
            self.shared.emit(PlatformEvent::WindowResized(window.id));
        } else if changed_mode {
            self.shared.emit(PlatformEvent::RefreshViewport { window: window.id });
        }
        if first || resized || changed_mode {
            self.shared.emit(PlatformEvent::WindowRedrawRequested(window.id));
        }
        let focused = configure.is_activated();
        if window.focused.replace(focused) != focused {
            self.shared.emit(PlatformEvent::WindowFocused { window: window.id, active: focused });
        }
    }
}

impl CompositorHandler for Protocol {
    fn scale_factor_changed(&mut self, _: &Connection, _: &QueueHandle<Self>, surface: &wl_surface::WlSurface, scale: i32) {
        if let Some(window) = self.shared.window(surface)
            && window.integer_scale.replace(if surface.version() >= 3 { scale.max(1) } else { 1 }) != window.integer_scale.get()
            && window.preferred_scale.get().is_none()
            && window.configured.get()
        {
            self.shared.update_cursor(window.id);
            self.shared.emit(PlatformEvent::WindowResized(window.id));
            self.shared.emit(PlatformEvent::WindowRedrawRequested(window.id));
        }
    }

    fn transform_changed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: wl_output::Transform) {}

    fn frame(&mut self, _: &Connection, _: &QueueHandle<Self>, surface: &wl_surface::WlSurface, _: u32) {
        if let Some(window) = self.shared.window(surface) {
            window.waiting_for_frame.set(false);
            if window.redraw_pending.get() {
                self.shared.emit(PlatformEvent::WindowRedrawRequested(window.id));
            }
        }
    }

    fn surface_enter(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: &wl_output::WlOutput) {}
    fn surface_leave(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: &wl_output::WlOutput) {}
}

impl Dispatch<wp_fractional_scale_v1::WpFractionalScaleV1, wl_surface::WlSurface> for Protocol {
    fn event(
        state: &mut Self,
        _: &wp_fractional_scale_v1::WpFractionalScaleV1,
        event: wp_fractional_scale_v1::Event,
        surface: &wl_surface::WlSurface,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wp_fractional_scale_v1::Event::PreferredScale { scale } = event
            && scale != 0
            && let Some(window) = state.shared.window(surface)
            && window.preferred_scale.replace(Some(scale)) != Some(scale)
            && window.configured.get()
        {
            state.shared.update_cursor(window.id);
            state.shared.emit(PlatformEvent::WindowResized(window.id));
            state.shared.emit(PlatformEvent::WindowRedrawRequested(window.id));
        }
    }
}

delegate_noop!(Protocol: ignore wp_viewport::WpViewport);

#[cfg(test)]
mod tests {
    use super::buffer_pixels;

    #[test]
    fn fractional_buffer_rounds_half_up_in_physical_pixels() {
        assert_eq!(buffer_pixels(601, 150), 751);
        assert_eq!(buffer_pixels(602, 150), 753);
        assert_eq!(buffer_pixels(603, 150), 754);
        assert_eq!(buffer_pixels(603, 240), 1206);
    }
}
