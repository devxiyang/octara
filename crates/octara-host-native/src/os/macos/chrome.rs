//! AppKit titlebar geometry and visual-effect backing view.

use objc2::rc::Retained;
use objc2::{MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{
    NSApplication, NSAutoresizingMaskOptions, NSButton, NSEventType, NSScreen, NSUserInterfaceLayoutDirection, NSView,
    NSVisualEffectBlendingMode, NSVisualEffectMaterial, NSVisualEffectState, NSVisualEffectView, NSWindow, NSWindowButton,
    NSWindowStyleMask,
};
use objc2_foundation::{NSPoint, NSRect, NSSize};
use octara::{
    Point, Rect, TitlebarStyle, VibrancyMaterial, WindowChrome, WindowGeometry, WindowGeometryRequest, WindowOptions, WindowViewport,
};
use std::cell::Cell;
use std::rc::Rc;

#[derive(Clone, Copy, Default, Eq, PartialEq)]
pub(super) enum Fullscreen {
    #[default]
    Windowed,
    Entering,
    Entered,
    Exiting,
}

pub(super) struct Chrome {
    style: TitlebarStyle,
    window: Retained<NSWindow>,
    fullscreen: Rc<Cell<Fullscreen>>,
    pending_geometry: Option<WindowGeometryRequest>,
    shown: bool,
    maximized: bool,
    restore_frame: Option<NSRect>,
    restore_fixed_size: bool,
}

impl Chrome {
    pub(super) fn new(window: Retained<NSWindow>, fullscreen: Rc<Cell<Fullscreen>>) -> Self {
        Self {
            style: TitlebarStyle::System,
            window,
            fullscreen,
            pending_geometry: None,
            shown: false,
            maximized: false,
            restore_frame: None,
            restore_fixed_size: false,
        }
    }

    pub(super) fn configure(&mut self, options: &WindowOptions) {
        self.style = options.titlebar;
        self.set_geometry(WindowGeometryRequest {
            position: options.position,
            maximized: options.maximized.then_some(true),
            ..Default::default()
        });
    }

    pub(super) fn did_show(&mut self) {
        self.shown = true;
        self.apply_pending();
    }

    pub(super) fn geometry(&mut self, viewport: WindowViewport) -> WindowGeometry {
        let fullscreen = self.fullscreen.get();
        if fullscreen == Fullscreen::Windowed {
            self.maximized = self.is_zoomed();
        }
        let frame = self.window.frame();
        let position =
            primary_screen(&self.window).map(|screen| Point::new(frame.origin.x as f32, (screen.frame().max().y - frame.max().y) as f32));
        WindowGeometry {
            position,
            size: viewport.size,
            scale: viewport.scale,
            maximized: self.maximized,
            fullscreen: matches!(fullscreen, Fullscreen::Entered | Fullscreen::Exiting),
        }
    }

    pub(super) fn apply_pending(&mut self) {
        if !self.shown {
            return;
        }
        match self.fullscreen.get() {
            Fullscreen::Windowed => {
                if std::mem::take(&mut self.restore_fixed_size) {
                    self.window.setStyleMask(self.window.styleMask() - NSWindowStyleMask::Resizable);
                }
                if let Some(geometry) = self.pending_geometry.take() {
                    self.apply_windowed(geometry);
                }
            }
            Fullscreen::Entered => {
                if let Some(geometry) = &mut self.pending_geometry
                    && geometry.fullscreen == Some(false)
                {
                    geometry.fullscreen = None;
                    self.fullscreen.set(Fullscreen::Exiting);
                    self.window.toggleFullScreen(None);
                }
            }
            Fullscreen::Entering | Fullscreen::Exiting => {}
        }
    }

    pub(super) fn set_geometry(&mut self, geometry: WindowGeometryRequest) {
        let pending = self.pending_geometry.get_or_insert_with(WindowGeometryRequest::default);
        pending.position = geometry.position.or(pending.position);
        pending.size = geometry.size.or(pending.size);
        pending.maximized = geometry.maximized.or(pending.maximized);
        pending.fullscreen = geometry.fullscreen.or(pending.fullscreen);
        if !self.shown {
            // Set the restore frame before the AppKit zoom or fullscreen animation starts.
            let (size, position) = (pending.size, pending.position);
            self.resize_and_move(size, position);
        }
        self.apply_pending();
    }

    fn apply_windowed(&mut self, geometry: WindowGeometryRequest) {
        if geometry.maximized == Some(false) {
            self.set_maximized(false);
        }
        self.resize_and_move(geometry.size, geometry.position);
        if geometry.maximized == Some(true) {
            self.set_maximized(true);
        }
        if geometry.fullscreen == Some(true) {
            self.maximized = self.is_zoomed();
            self.fullscreen.set(Fullscreen::Entering);
            let style = self.window.styleMask();
            if !style.contains(NSWindowStyleMask::Resizable) {
                self.restore_fixed_size = true;
                self.window.setStyleMask(style | NSWindowStyleMask::Resizable);
            }
            self.window.toggleFullScreen(None);
        }
    }

    fn resize_and_move(&self, size: Option<octara::Size>, position: Option<Point>) {
        if let Some(size) = size.filter(|size| size.width.is_finite() && size.height.is_finite() && size.width > 0.0 && size.height > 0.0) {
            self.window.setContentSize(NSSize::new(f64::from(size.width), f64::from(size.height)));
        }
        if let Some(point) = position.filter(|point| point.x.is_finite() && point.y.is_finite())
            && let Some(screen) = primary_screen(&self.window)
        {
            let bottom = screen.frame().max().y - f64::from(point.y) - self.window.frame().size.height;
            self.window.setFrameOrigin(NSPoint::new(f64::from(point.x), bottom));
        }
    }

    fn is_zoomed(&self) -> bool {
        self.restore_frame.is_some() || self.window.isZoomed()
    }

    fn set_maximized(&mut self, maximized: bool) {
        if self.is_zoomed() == maximized {
            return;
        }
        if self.window.styleMask().contains(NSWindowStyleMask::Resizable) {
            self.window.zoom(None);
        } else if maximized {
            if let Some(screen) = self.window.screen().or_else(|| primary_screen(&self.window)) {
                self.restore_frame = Some(self.window.frame());
                self.window.setFrame_display(screen.visibleFrame(), true);
            }
        } else if let Some(restore) = self.restore_frame.take() {
            self.window.setFrame_display(restore, true);
        }
    }

    pub(super) fn toggle_maximize(&mut self) {
        let maximized = self.pending_geometry.and_then(|geometry| geometry.maximized).unwrap_or_else(|| self.is_zoomed());
        self.set_geometry(WindowGeometryRequest { maximized: Some(!maximized), ..Default::default() });
    }

    pub(super) fn minimize(&self) {
        self.window.performMiniaturize(None);
    }

    pub(super) fn start_drag(&self) {
        let mtm = MainThreadMarker::from(&*self.window);
        if let Some(event) = NSApplication::sharedApplication(mtm).currentEvent()
            && event.r#type() == NSEventType::LeftMouseDown
            && event.windowNumber() == self.window.windowNumber()
        {
            self.window.performWindowDragWithEvent(&event);
        }
    }

    fn has_fullscreen_style(&self) -> bool {
        self.window.styleMask().contains(NSWindowStyleMask::FullScreen)
    }

    pub(super) fn relayout_after_presentation(&self, view: &NSView) -> bool {
        // AppKit can restore the default button frames when the window title changes.
        self.shown && self.layout_traffic_lights(view).is_some()
    }

    fn layout_traffic_lights(&self, view: &NSView) -> Option<Vec<Retained<NSButton>>> {
        let TitlebarStyle::HiddenInset { traffic_light_offset } = self.style else { return None };
        if self.has_fullscreen_style() {
            return None;
        }
        let buttons: Vec<_> = [NSWindowButton::CloseButton, NSWindowButton::MiniaturizeButton, NSWindowButton::ZoomButton]
            .into_iter()
            .filter_map(|kind| self.window.standardWindowButton(kind))
            .filter(|button| !button.isHiddenOrHasHiddenAncestor())
            .collect();
        if let Some(offset) = traffic_light_offset.filter(|point| point.x.is_finite() && point.y.is_finite())
            && let Some(leading) = buttons.first().and_then(|button| button_rect(view, button))
        {
            let dx = f64::from(offset.x.max(0.0) - leading.x);
            let dy = f64::from(offset.y.max(0.0) - leading.y) * if view.isFlipped() { 1.0 } else { -1.0 };
            for button in &buttons {
                // SAFETY: The retained AppKit button is inspected on the window's main thread.
                if let Some(parent) = unsafe { button.superview() } {
                    let mut rect = view.convertRect_fromView(button.frame(), Some(&parent));
                    rect.origin.x += dx;
                    rect.origin.y += dy;
                    let origin = parent.convertRect_fromView(rect, Some(view)).origin;
                    if button.frame().origin != origin {
                        button.setFrameOrigin(origin);
                    }
                }
            }
        }
        Some(buttons)
    }

    pub(super) fn metrics(&self, view: &NSView) -> WindowChrome {
        let Some(buttons) = self.layout_traffic_lights(view) else { return WindowChrome::default() };
        let traffic_light_rect = buttons.iter().filter_map(|button| button_rect(view, button)).reduce(|a, b| {
            let x = a.x.min(b.x);
            let y = a.y.min(b.y);
            Rect::new(x, y, a.right().max(b.right()) - x, a.bottom().max(b.bottom()) - y)
        });
        let layout_rect = content_rect(view, view.convertRect_fromView(self.window.contentLayoutRect(), None));
        let height = layout_rect.y.max(traffic_light_rect.map_or(0.0, |rect| rect.bottom()));
        let (mut leading_inset, mut trailing_inset) = (0.0, 0.0);
        if let Some(rect) = traffic_light_rect {
            let width = view.bounds().size.width as f32;
            if rect.x <= width - rect.right() {
                leading_inset = rect.right().min(width);
            } else {
                trailing_inset = (width - rect.x).min(width);
            }
            if view.userInterfaceLayoutDirection() == NSUserInterfaceLayoutDirection::RightToLeft {
                std::mem::swap(&mut leading_inset, &mut trailing_inset);
            }
        }
        WindowChrome { titlebar_height: height.max(0.0), traffic_light_rect, leading_inset, trailing_inset }
    }
}

fn primary_screen(window: &NSWindow) -> Option<Retained<NSScreen>> {
    NSScreen::screens(MainThreadMarker::from(window)).firstObject()
}

fn button_rect(view: &NSView, button: &NSButton) -> Option<Rect> {
    // SAFETY: The retained AppKit button is inspected on the window's main thread.
    let parent = unsafe { button.superview() }?;
    Some(content_rect(view, view.convertRect_fromView(button.frame(), Some(&parent))))
}

fn content_rect(view: &NSView, rect: NSRect) -> Rect {
    let bounds = view.bounds();
    let y = if view.isFlipped() { rect.origin.y - bounds.origin.y } else { bounds.max().y - rect.max().y };
    Rect::new((rect.origin.x - bounds.origin.x) as f32, y as f32, rect.size.width as f32, rect.size.height as f32)
}

define_class!(
    // SAFETY: NSVisualEffectView has no subclassing requirements.
    #[unsafe(super(NSVisualEffectView, NSView))]
    #[name = "OctaraWindowVisualEffect"]
    struct WindowVisualEffect;

    impl WindowVisualEffect {
        #[unsafe(method_id(hitTest:))]
        fn hit_test(&self, _: NSPoint) -> Option<Retained<NSView>> {
            None
        }
    }
);

pub(super) fn visual_effect_material(material: VibrancyMaterial) -> NSVisualEffectMaterial {
    match material {
        VibrancyMaterial::Sidebar => NSVisualEffectMaterial::Sidebar,
        VibrancyMaterial::UnderWindowBackground => NSVisualEffectMaterial::UnderWindowBackground,
        VibrancyMaterial::Titlebar => NSVisualEffectMaterial::Titlebar,
        VibrancyMaterial::Popover => NSVisualEffectMaterial::Popover,
        VibrancyMaterial::Hud => NSVisualEffectMaterial::HUDWindow,
    }
}

pub(super) fn visual_effect(view: &NSView, material: NSVisualEffectMaterial) -> Retained<NSVisualEffectView> {
    let effect = WindowVisualEffect::alloc(MainThreadMarker::from(view)).set_ivars(());
    // SAFETY: The subclass completes NSView initialization with its retained view frame.
    let effect: Retained<WindowVisualEffect> = unsafe { msg_send![super(effect), initWithFrame: view.bounds()] };
    let effect = effect.into_super();
    effect.setMaterial(material);
    effect.setBlendingMode(NSVisualEffectBlendingMode::BehindWindow);
    effect.setState(NSVisualEffectState::FollowsWindowActiveState);
    effect.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewHeightSizable);
    effect.setWantsLayer(true);
    effect
}
