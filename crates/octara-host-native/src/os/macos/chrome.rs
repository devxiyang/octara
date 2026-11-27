//! AppKit's titlebar, native window observations, and visual-effect backing view.

use crate::ShellEvent;
use crate::os::{apply_window_geometry, read_window_geometry};
use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::{NSObjectProtocol, ProtocolObject};
use objc2::{MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{
    NSApplication, NSAutoresizingMaskOptions, NSButton, NSEventType, NSUserInterfaceLayoutDirection, NSView, NSVisualEffectBlendingMode,
    NSVisualEffectMaterial, NSVisualEffectState, NSVisualEffectView, NSWindow, NSWindowButton, NSWindowDidEnterFullScreenNotification,
    NSWindowDidExitFullScreenNotification, NSWindowStyleMask,
};
use objc2_foundation::{NSNotificationCenter, NSPoint, NSRect};
use octara::{
    Rect, TitlebarStyle, VibrancyMaterial, WindowChrome, WindowGeometry, WindowGeometryRequest, WindowId, WindowOptions, WindowViewport,
};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use winit::event_loop::EventLoopProxy;
use winit::window::Window;

pub(super) struct Chrome {
    style: TitlebarStyle,
    window: Retained<NSWindow>,
    fullscreen_observers: [Retained<ProtocolObject<dyn NSObjectProtocol>>; 2],
    fullscreen: Arc<AtomicBool>,
    pending_geometry: Option<WindowGeometryRequest>,
    maximized: bool,
    initial_geometry: Option<WindowGeometryRequest>,
}

impl Chrome {
    pub(super) fn new(window: Retained<NSWindow>, proxy: EventLoopProxy<ShellEvent>, id: WindowId) -> Self {
        let center = NSNotificationCenter::defaultCenter();
        let fullscreen = Arc::new(AtomicBool::new(false));
        // SAFETY: These AppKit notifications are posted on the main thread by this window.
        let names = unsafe { [(NSWindowDidEnterFullScreenNotification, true), (NSWindowDidExitFullScreenNotification, false)] };
        let fullscreen_observers = names.map(|(name, entered)| {
            let fullscreen = fullscreen.clone();
            let proxy = proxy.clone();
            let callback = RcBlock::new(move |_| {
                fullscreen.store(entered, Ordering::Relaxed);
                let _ = proxy.send_event(ShellEvent::RefreshViewport { window: id });
            });
            // SAFETY: The callback captures thread-safe values and is registered for this window.
            unsafe { center.addObserverForName_object_queue_usingBlock(Some(name), Some(&window), None, &callback) }
        });
        Self {
            style: TitlebarStyle::System,
            window,
            fullscreen_observers,
            fullscreen,
            pending_geometry: None,
            maximized: false,
            initial_geometry: Some(WindowGeometryRequest::default()),
        }
    }

    pub(super) fn configure(&mut self, window: &Window, options: &WindowOptions) {
        self.style = options.titlebar;
        self.window.setMovableByWindowBackground(false);
        self.set_geometry(
            window,
            WindowGeometryRequest { position: options.position, maximized: options.maximized.then_some(true), ..Default::default() },
        );
    }

    pub(super) fn did_show(&mut self, window: &Window) {
        if let Some(geometry) = self.initial_geometry.take() {
            apply_window_geometry(window, geometry);
        }
    }

    pub(super) fn geometry(&mut self, window: &Window, viewport: WindowViewport) -> WindowGeometry {
        let fullscreen = self.fullscreen();
        if !fullscreen && window.fullscreen().is_none() && !self.has_fullscreen_style() {
            self.maximized = window.is_maximized();
        }
        read_window_geometry(window, viewport, self.maximized, fullscreen)
    }

    pub(super) fn apply_pending(&mut self, window: &Window) {
        if !self.fullscreen()
            && window.fullscreen().is_none()
            && let Some(geometry) = self.pending_geometry.take()
        {
            apply_window_geometry(window, geometry);
        }
    }

    pub(super) fn set_geometry(&mut self, window: &Window, mut geometry: WindowGeometryRequest) {
        if let Some(initial) = &mut self.initial_geometry {
            initial.position = geometry.position.or(initial.position);
            initial.maximized = geometry.maximized.take().or(initial.maximized);
            initial.fullscreen = geometry.fullscreen.take().or(initial.fullscreen);
            apply_window_geometry(window, geometry);
            return;
        }
        if let Some(pending) = &mut self.pending_geometry {
            pending.position = geometry.position.or(pending.position);
            pending.size = geometry.size.or(pending.size);
            pending.maximized = geometry.maximized.or(pending.maximized);
            if let Some(fullscreen) = geometry.fullscreen {
                pending.fullscreen = fullscreen.then_some(true);
            }
            return;
        }
        if geometry.fullscreen == Some(false) && (self.fullscreen() || window.fullscreen().is_some()) {
            geometry.fullscreen = None;
            self.pending_geometry = Some(geometry);
            window.set_fullscreen(None);
        } else {
            apply_window_geometry(window, geometry);
        }
    }

    pub(super) fn toggle_maximize(&mut self, window: &Window) {
        let maximized = self.initial_geometry.and_then(|geometry| geometry.maximized).unwrap_or(self.maximized);
        self.set_geometry(window, WindowGeometryRequest { maximized: Some(!maximized), ..Default::default() });
    }

    pub(super) fn minimize(&self) {
        self.window.performMiniaturize(None);
    }

    pub(super) fn start_drag(&self, window: &Window) {
        let mtm = MainThreadMarker::from(&*self.window);
        if NSApplication::sharedApplication(mtm)
            .currentEvent()
            .is_some_and(|event| event.r#type() == NSEventType::LeftMouseDown && event.windowNumber() == self.window.windowNumber())
        {
            let _ = window.drag_window();
        }
    }

    fn fullscreen(&self) -> bool {
        self.fullscreen.load(Ordering::Relaxed)
    }

    fn has_fullscreen_style(&self) -> bool {
        self.window.styleMask().contains(NSWindowStyleMask::FullScreen)
    }

    pub(super) fn metrics(&self, view: &NSView) -> WindowChrome {
        let TitlebarStyle::HiddenInset { traffic_light_offset } = self.style else { return WindowChrome::default() };
        if self.has_fullscreen_style() {
            return WindowChrome::default();
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

impl Drop for Chrome {
    fn drop(&mut self) {
        let center = NSNotificationCenter::defaultCenter();
        for observer in &self.fullscreen_observers {
            // SAFETY: The observer token belongs to this notification center.
            unsafe { center.removeObserver((**observer).as_ref()) };
        }
    }
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

pub(super) fn visual_effect(view: &NSView, material: VibrancyMaterial) -> Retained<NSView> {
    let effect = WindowVisualEffect::alloc(MainThreadMarker::from(view)).set_ivars(());
    // SAFETY: The subclass completes NSView initialization with its retained view frame.
    let effect: Retained<WindowVisualEffect> = unsafe { msg_send![super(effect), initWithFrame: view.bounds()] };
    let effect = effect.into_super();
    effect.setMaterial(match material {
        VibrancyMaterial::Sidebar => NSVisualEffectMaterial::Sidebar,
        VibrancyMaterial::UnderWindowBackground => NSVisualEffectMaterial::UnderWindowBackground,
        VibrancyMaterial::Titlebar => NSVisualEffectMaterial::Titlebar,
        VibrancyMaterial::Popover => NSVisualEffectMaterial::Popover,
        VibrancyMaterial::Hud => NSVisualEffectMaterial::HUDWindow,
    });
    effect.setBlendingMode(NSVisualEffectBlendingMode::BehindWindow);
    effect.setState(NSVisualEffectState::FollowsWindowActiveState);
    effect.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewHeightSizable);
    effect.setWantsLayer(true);
    effect.into_super()
}
