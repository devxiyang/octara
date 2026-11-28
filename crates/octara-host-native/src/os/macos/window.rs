//! AppKit window and delegate ownership.

use super::chrome::Fullscreen;
use super::view::OctaraView;
use crate::os::{PlatformEvent, PlatformSender};
use objc2::rc::Retained;
use objc2::runtime::{NSObject, NSObjectProtocol, ProtocolObject};
use objc2::{ClassType, DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{
    NSBackingStoreType, NSFloatingWindowLevel, NSNormalWindowLevel, NSPanel, NSStatusWindowLevel, NSView, NSWindow, NSWindowDelegate,
    NSWindowOcclusionState, NSWindowStyleMask, NSWindowTabbingMode, NSWindowTitleVisibility,
};
use objc2_foundation::{NSNotification, NSPoint, NSRect, NSSize, NSString};
use octara::{TitlebarStyle, WindowId, WindowOptions};
use octara_model::window::WindowLevel;
use std::cell::Cell;
use std::rc::Rc;

define_class!(
    // SAFETY: OctaraWindow uses the designated NSWindow initializer on the main thread.
    #[unsafe(super(NSWindow))]
    #[thread_kind = MainThreadOnly]
    #[name = "OctaraWindow"]
    struct OctaraWindow;
);

define_class!(
    // SAFETY: OctaraPanel uses the designated NSPanel initializer on the main thread.
    #[unsafe(super(NSPanel, NSWindow))]
    #[thread_kind = MainThreadOnly]
    #[name = "OctaraPanel"]
    struct OctaraPanel;
);

pub(super) fn create(mtm: MainThreadMarker, options: &WindowOptions) -> Retained<NSWindow> {
    let mut size = options.viewport.size;
    if let Some(max) = options.max_size {
        size.width = size.width.min(max.width);
        size.height = size.height.min(max.height);
    }
    if let Some(min) = options.min_size {
        size.width = size.width.max(min.width);
        size.height = size.height.max(min.height);
    }
    let frame = NSRect::new(NSPoint::ZERO, NSSize::new(f64::from(size.width), f64::from(size.height)));
    let mut style = NSWindowStyleMask::Titled | NSWindowStyleMask::Closable | NSWindowStyleMask::Miniaturizable;
    if options.resizable {
        style |= NSWindowStyleMask::Resizable;
    }
    let hidden_inset = matches!(options.titlebar, TitlebarStyle::HiddenInset { .. });
    if hidden_inset {
        style |= NSWindowStyleMask::FullSizeContentView;
    }
    let window = if options.non_activating_panel {
        let panel = mtm.alloc::<OctaraPanel>().set_ivars(());
        let style = style | NSWindowStyleMask::NonactivatingPanel;
        // SAFETY: Pass the nonactivating style to the designated initializer to establish AppKit's activation behavior.
        let panel: Retained<OctaraPanel> = unsafe {
            msg_send![super(panel), initWithContentRect: frame, styleMask: style, backing: NSBackingStoreType::Buffered, defer: false]
        };
        panel.setHidesOnDeactivate(false);
        panel.into_super().into_super()
    } else {
        let window = mtm.alloc::<OctaraWindow>().set_ivars(());
        // SAFETY: This is the designated initializer for the allocated NSWindow subclass.
        let window: Retained<OctaraWindow> = unsafe {
            msg_send![super(window), initWithContentRect: frame, styleMask: style, backing: NSBackingStoreType::Buffered, defer: false]
        };
        window.into_super()
    };
    // SAFETY: The Rust host owns the retain count, including after the native window closes.
    unsafe { window.setReleasedWhenClosed(false) };
    window.setTitle(&NSString::from_str(&options.presentation.title));
    window.setTabbingMode(NSWindowTabbingMode::Disallowed);
    window.setAcceptsMouseMovedEvents(true);
    window.setMovableByWindowBackground(false);
    if hidden_inset {
        window.setTitleVisibility(NSWindowTitleVisibility::Hidden);
        window.setTitlebarAppearsTransparent(true);
    }
    if let Some(min) = options.min_size {
        window.setContentMinSize(NSSize::new(f64::from(min.width), f64::from(min.height)));
    }
    if let Some(max) = options.max_size {
        window.setContentMaxSize(NSSize::new(f64::from(max.width), f64::from(max.height)));
    }
    window.center();
    window
}

pub(super) fn set_level(window: &NSWindow, level: WindowLevel) {
    let level = match level {
        WindowLevel::Normal => NSNormalWindowLevel,
        WindowLevel::Floating => NSFloatingWindowLevel,
        WindowLevel::AlwaysOnTop => NSStatusWindowLevel,
    };
    if window.level() != level {
        window.setLevel(level);
    }
}

define_class!(
    // SAFETY: The content container and its render view are owned by the main thread.
    #[unsafe(super(NSView))]
    #[thread_kind = MainThreadOnly]
    #[name = "OctaraWindowContentView"]
    #[ivars = Retained<OctaraView>]
    struct ContentView;

    impl ContentView {
        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> bool {
            true
        }

        #[unsafe(method_id(hitTest:))]
        fn hit_test(&self, point: NSPoint) -> Option<Retained<NSView>> {
            // SAFETY: This forwards NSView's documented responder hit test.
            let hit: Option<Retained<NSView>> = unsafe { msg_send![super(self), hitTest: point] };
            hit.map(|hit| self.native_hit(point, hit))
        }
    }
);

impl ContentView {
    fn native_hit(&self, point: NSPoint, hit: Retained<NSView>) -> Retained<NSView> {
        let render = self.ivars().as_super();
        if !std::ptr::eq(&*hit, render) {
            return hit;
        }
        // The Metal layer visually covers its native siblings. Their clip views decide when
        // a point lies in their transparent opening and should reach a native responder.
        // SAFETY: The view hierarchy is stable while AppKit performs main-thread hit testing.
        let parent = unsafe { self.superview() };
        let point = self.convertPoint_fromView(point, parent.as_deref());
        let views = self.subviews();
        (0..views.len())
            .rev()
            .map(|index| views.objectAtIndex(index))
            .filter(|view| !std::ptr::eq(&**view, render) && !view.isHidden())
            .find_map(|view| view.hitTest(point))
            .unwrap_or(hit)
    }
}

pub(super) fn attach(window: &NSWindow, render: &Retained<OctaraView>) -> Retained<NSView> {
    let parent = MainThreadMarker::from(window).alloc::<ContentView>().set_ivars(render.clone());
    // SAFETY: This initializes the NSView subclass on the main thread.
    let parent: Retained<ContentView> = unsafe { msg_send![super(parent), initWithFrame: render.frame()] };
    let parent = parent.into_super();
    parent.setWantsLayer(true);
    parent.addSubview(render);
    window.setContentView(Some(&parent));
    window.setInitialFirstResponder(Some(render));
    window.makeFirstResponder(Some(render));
    parent
}

pub(super) struct DelegateState {
    pub(super) fullscreen: Rc<Cell<Fullscreen>>,
    pub(super) closed: Rc<Cell<bool>>,
    pub(super) view: Retained<OctaraView>,
    pub(super) window: WindowId,
    pub(super) sender: PlatformSender,
}

impl DelegateState {
    fn resize(&self) {
        self.sender.send_event(PlatformEvent::WindowResized(self.window));
    }

    fn fullscreen(&self, fullscreen: Fullscreen) {
        self.fullscreen.set(fullscreen);
        self.resize();
    }

    fn focus(&self, active: bool) {
        self.view.set_window_focused(active);
        self.sender.send_event(PlatformEvent::WindowFocused { window: self.window, active });
    }

    fn redraw(&self) {
        self.sender.send_event(PlatformEvent::WindowRedrawRequested(self.window));
    }
}

define_class!(
    // SAFETY: The main-thread window owns the delegate's lifetime through a retained reference.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "OctaraWindowDelegate"]
    #[ivars = DelegateState]
    pub(super) struct Delegate;

    unsafe impl NSObjectProtocol for Delegate {}

    unsafe impl NSWindowDelegate for Delegate {
        #[unsafe(method(windowShouldClose:))]
        fn should_close(&self, _: &NSWindow) -> bool {
            let state = self.ivars();
            state.sender.send_event(PlatformEvent::WindowCloseRequested(state.window));
            false
        }

        #[unsafe(method(windowWillClose:))]
        fn will_close(&self, _: &NSNotification) {
            let state = self.ivars();
            state.closed.set(true);
            state.view.set_window_focused(false);
            state.sender.send_event(PlatformEvent::WindowDestroyed(state.window));
        }

        #[unsafe(method(windowDidResize:))]
        fn did_resize(&self, _: &NSNotification) {
            self.ivars().resize();
        }

        #[unsafe(method(windowDidMove:))]
        fn did_move(&self, _: &NSNotification) {
            let state = self.ivars();
            state.sender.send_event(PlatformEvent::WindowMoved(state.window));
        }

        #[unsafe(method(windowDidChangeBackingProperties:))]
        fn did_change_backing_properties(&self, _: &NSNotification) {
            self.ivars().resize();
        }

        #[unsafe(method(windowDidChangeScreen:))]
        fn did_change_screen(&self, _: &NSNotification) {
            self.ivars().resize();
        }

        #[unsafe(method(windowDidBecomeKey:))]
        fn did_become_key(&self, _: &NSNotification) {
            self.ivars().focus(true);
        }

        #[unsafe(method(windowDidResignKey:))]
        fn did_resign_key(&self, _: &NSNotification) {
            self.ivars().focus(false);
        }

        #[unsafe(method(windowDidExpose:))]
        fn did_expose(&self, _: &NSNotification) {
            self.ivars().redraw();
        }

        #[unsafe(method(windowDidDeminiaturize:))]
        fn did_deminiaturize(&self, _: &NSNotification) {
            self.ivars().redraw();
        }

        #[unsafe(method(windowDidChangeOcclusionState:))]
        fn did_change_occlusion_state(&self, _: &NSNotification) {
            if self.ivars().view.window().is_some_and(|window| window.occlusionState().contains(NSWindowOcclusionState::Visible)) {
                self.ivars().redraw();
            }
        }

        #[unsafe(method(windowWillEnterFullScreen:))]
        fn will_enter_fullscreen(&self, _: &NSNotification) {
            self.ivars().fullscreen(Fullscreen::Entering);
        }

        #[unsafe(method(windowDidEnterFullScreen:))]
        fn did_enter_fullscreen(&self, _: &NSNotification) {
            self.ivars().fullscreen(Fullscreen::Entered);
        }

        #[unsafe(method(windowDidFailToEnterFullScreen:))]
        fn did_fail_to_enter_fullscreen(&self, _: &NSWindow) {
            self.ivars().fullscreen(Fullscreen::Windowed);
        }

        #[unsafe(method(windowWillExitFullScreen:))]
        fn will_exit_fullscreen(&self, _: &NSNotification) {
            self.ivars().fullscreen(Fullscreen::Exiting);
        }

        #[unsafe(method(windowDidExitFullScreen:))]
        fn did_exit_fullscreen(&self, _: &NSNotification) {
            self.ivars().fullscreen(Fullscreen::Windowed);
        }

        #[unsafe(method(windowDidFailToExitFullScreen:))]
        fn did_fail_to_exit_fullscreen(&self, _: &NSWindow) {
            self.ivars().fullscreen(Fullscreen::Entered);
        }
    }
);

impl Delegate {
    pub(super) fn install(window: &NSWindow, state: DelegateState) -> Retained<Self> {
        let delegate = MainThreadMarker::from(window).alloc::<Self>().set_ivars(state);
        // SAFETY: The delegate's NSObject superclass is initialized before AppKit can call it.
        let delegate: Retained<Self> = unsafe { msg_send![super(delegate), init] };
        window.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
        delegate
    }
}
