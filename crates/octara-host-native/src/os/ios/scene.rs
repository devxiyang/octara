//! Attach winit's UIKit window to the application scene and observe its usable area.

use crate::ShellEvent;
use objc2::rc::{Retained, Weak as ObjcWeak};
use objc2::runtime::{NSObject, NSObjectProtocol};
use objc2::{ClassType, DefinedClass, MainThreadMarker, MainThreadOnly, Message, define_class, msg_send};
use objc2_foundation::NSRect;
use objc2_ui_kit::{
    UIApplication, UIScene, UISceneActivationState, UISceneConnectionOptions, UISceneDelegate, UISceneSession, UIView, UIViewAutoresizing,
    UIViewController, UIWindow, UIWindowScene,
};
use octara_model::geometry::{Rect, Size, WindowViewport};
use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};
use winit::event_loop::EventLoopProxy;
use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
use winit::window::Window;

#[derive(Default)]
struct SceneBridge {
    proxy: Option<EventLoopProxy<ShellEvent>>,
    scene: Option<Retained<UIWindowScene>>,
    windows: Vec<Weak<SceneWindow>>,
}

thread_local! {
    static BRIDGE: RefCell<SceneBridge> = RefCell::new(SceneBridge::default());
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "OctaraWindowSceneDelegate"]
    struct SceneDelegate;

    unsafe impl NSObjectProtocol for SceneDelegate {}

    unsafe impl UISceneDelegate for SceneDelegate {
        #[unsafe(method(scene:willConnectToSession:options:))]
        fn connect(&self, scene: &UIScene, _: &UISceneSession, _: &UISceneConnectionOptions) {
            let Ok(scene) = scene.retain().downcast::<UIWindowScene>() else { return };
            let proxy = BRIDGE.with_borrow_mut(|bridge| {
                bridge.scene = Some(scene.clone());
                bridge.proxy.clone()
            });
            if let Some(proxy) = proxy {
                let _ = proxy.send_event(ShellEvent::PlatformReady);
            }
            for window in scene_windows() {
                window.attach(&scene);
            }
        }

        #[unsafe(method(sceneDidDisconnect:))]
        fn disconnect(&self, scene: &UIScene) {
            BRIDGE.with_borrow_mut(|bridge| {
                if bridge.scene.as_ref().is_some_and(|current| std::ptr::eq(current.as_super(), scene)) {
                    bridge.scene = None;
                }
            });
        }

        #[unsafe(method(sceneDidBecomeActive:))]
        fn activate(&self, scene: &UIScene) {
            scene_activity(scene, true);
        }

        #[unsafe(method(sceneWillResignActive:))]
        fn deactivate(&self, scene: &UIScene) {
            scene_activity(scene, false);
        }
    }
);

fn scene_windows() -> Vec<Rc<SceneWindow>> {
    BRIDGE.with_borrow_mut(|bridge| {
        bridge.windows.retain(|window| window.strong_count() > 0);
        bridge.windows.iter().filter_map(Weak::upgrade).collect()
    })
}

fn scene_activity(scene: &UIScene, active: bool) {
    for window in scene_windows() {
        if window.window.load().and_then(|window| window.windowScene()).is_some_and(|current| std::ptr::eq(current.as_super(), scene)) {
            (window.activity)(active);
            (window.refresh)();
        }
    }
}

pub(super) fn ready() -> bool {
    BRIDGE.with_borrow(|bridge| bridge.scene.is_some()) || MainThreadMarker::new().and_then(application_scene).is_some()
}

/// Attach the hidden window to the active scene before Metal asks for its surface.
pub(super) fn prepare_window(window: &Window) -> Result<(), String> {
    let view = view(window)?;
    let native = view.window().ok_or("the UIKit view has no window")?;
    let main = MainThreadMarker::from(&*view);
    if let Some(scene) = BRIDGE.with_borrow(|bridge| bridge.scene.clone()).or_else(|| application_scene(main)) {
        native.setWindowScene(Some(&scene));
    }
    native.setHidden(true);
    Ok(())
}

pub(super) fn register(proxy: EventLoopProxy<ShellEvent>) {
    let _ = SceneDelegate::class();
    BRIDGE.with_borrow_mut(|bridge| bridge.proxy = Some(proxy));
}

struct SceneWindow {
    window: ObjcWeak<UIWindow>,
    presented: Cell<bool>,
    refresh: Rc<dyn Fn()>,
    activity: Rc<dyn Fn(bool)>,
}

impl SceneWindow {
    fn attach(&self, scene: &UIWindowScene) {
        let Some(window) = self.window.load() else { return };
        if !window.windowScene().is_some_and(|current| std::ptr::eq(&*current, scene)) {
            window.setWindowScene(Some(scene));
        }
        if self.presented.get() {
            window.makeKeyAndVisible();
        }
        (self.refresh)();
    }
}

#[derive(Default)]
struct AreaState {
    geometry: Cell<Option<(Size, Rect)>>,
    refresh: Option<Rc<dyn Fn()>>,
}

define_class!(
    #[unsafe(super(UIView))]
    #[thread_kind = MainThreadOnly]
    #[name = "OctaraSafeAreaObserver"]
    #[ivars = AreaState]
    struct AreaObserver;

    impl AreaObserver {
        #[unsafe(method(safeAreaInsetsDidChange))]
        fn insets_changed(&self) {
            // SAFETY: Invoke the inherited UIView lifecycle callback.
            let _: () = unsafe { msg_send![super(self), safeAreaInsetsDidChange] };
            self.notify();
        }

        #[unsafe(method(layoutSubviews))]
        fn layout_updated(&self) {
            // SAFETY: Invoke the inherited UIView lifecycle callback.
            let _: () = unsafe { msg_send![super(self), layoutSubviews] };
            self.notify();
        }
    }
);

impl AreaObserver {
    fn notify(&self) {
        let bounds = self.bounds();
        let next = (Size::new(bounds.size.width as f32, bounds.size.height as f32), safe_area(self));
        if self.ivars().geometry.replace(Some(next)) != Some(next)
            && let Some(refresh) = &self.ivars().refresh
        {
            refresh();
        }
    }
}

pub(super) struct SceneConnection {
    window: Rc<SceneWindow>,
    observer: Retained<AreaObserver>,
}

impl SceneConnection {
    pub(super) fn new(content: &UIView, refresh: Rc<dyn Fn()>, activity: impl Fn(bool) + 'static) -> Result<Self, String> {
        let window = content.window().ok_or("the UIKit view has no window")?;
        let mtm = MainThreadMarker::from(content);
        let window = Rc::new(SceneWindow {
            window: ObjcWeak::new(&*window),
            presented: Cell::new(false),
            refresh: refresh.clone(),
            activity: Rc::new(activity),
        });
        BRIDGE.with_borrow_mut(|bridge| bridge.windows.push(Rc::downgrade(&window)));
        if let Some(scene) = BRIDGE.with_borrow(|bridge| bridge.scene.clone()).or_else(|| application_scene(mtm)) {
            window.attach(&scene);
        }
        let state = AreaState { geometry: Cell::new(None), refresh: Some(refresh) };
        let frame = content.bounds();
        // SAFETY: Initialize the UIView superclass on the UIKit thread.
        let observer: Retained<AreaObserver> =
            unsafe { msg_send![super(mtm.alloc::<AreaObserver>().set_ivars(state)), initWithFrame: frame] };
        observer.setUserInteractionEnabled(false);
        observer.setAutoresizingMask(UIViewAutoresizing::FlexibleWidth | UIViewAutoresizing::FlexibleHeight);
        content.addSubview(&observer);
        Ok(Self { window, observer })
    }

    pub(super) fn show(&self) {
        self.window.presented.set(true);
        if let Some(window) = self.window.window.load() {
            window.makeKeyAndVisible();
        }
    }
}

impl Drop for SceneConnection {
    fn drop(&mut self) {
        self.observer.removeFromSuperview();
    }
}

fn application_scene(mtm: MainThreadMarker) -> Option<Retained<UIWindowScene>> {
    let app = UIApplication::sharedApplication(mtm);
    app.connectedScenes()
        .iter()
        .filter_map(|scene| scene.downcast::<UIWindowScene>().ok())
        .max_by_key(|scene| scene.activationState() == UISceneActivationState::ForegroundActive)
}

fn safe_area(view: &UIView) -> Rect {
    let bounds = view.bounds();
    let NSRect { origin, size } = view.safeAreaLayoutGuide().layoutFrame();
    Rect::new((origin.x - bounds.origin.x) as f32, (origin.y - bounds.origin.y) as f32, size.width as f32, size.height as f32)
}

pub(super) fn viewport(content: &UIView, keyboard_top: Option<f32>, mut viewport: WindowViewport) -> WindowViewport {
    let bounds = content.bounds();
    if bounds.size.width > 0.0 && bounds.size.height > 0.0 {
        let height = bounds.size.height as f32;
        viewport.size = Size::new(bounds.size.width as f32, keyboard_top.unwrap_or(height).min(height));
        viewport.scale = content.contentScaleFactor() as f32;
        viewport.safe_area = Some(safe_area(content));
    }
    viewport
}

pub(super) fn view(window: &Window) -> Result<Retained<UIView>, String> {
    let _mtm = MainThreadMarker::new().ok_or("UIKit windows require the main thread")?;
    let handle = window.window_handle().map_err(|why| why.to_string())?;
    let RawWindowHandle::UiKit(handle) = handle.as_raw() else { return Err("the window has no UIKit view".into()) };
    // SAFETY: The main thread owns the winit window and its view throughout the call.
    unsafe { Retained::retain(handle.ui_view.cast().as_ptr()) }.ok_or_else(|| "the UIKit view is missing".into())
}

pub(super) fn controller(window: &Window) -> Result<Retained<UIViewController>, String> {
    let _mtm = MainThreadMarker::new().ok_or("UIKit windows require the main thread")?;
    let handle = window.window_handle().map_err(|why| why.to_string())?;
    let RawWindowHandle::UiKit(handle) = handle.as_raw() else { return Err("the window has no UIKit controller".into()) };
    let controller = handle.ui_view_controller.ok_or("the UIKit controller is missing")?;
    // SAFETY: The main thread owns the winit window and its controller throughout the call.
    unsafe { Retained::retain(controller.cast().as_ptr()) }.ok_or_else(|| "the UIKit controller is missing".into())
}
