//! Connects winit 0.30's UIKit window to the system's single application scene.

use crate::ShellEvent;
use objc2::rc::Retained;
use objc2::runtime::{NSObject, NSObjectProtocol};
use objc2::{ClassType, MainThreadOnly, Message, define_class};
use objc2_ui_kit::{UIScene, UISceneConnectionOptions, UISceneDelegate, UISceneSession, UIView, UIWindow, UIWindowScene};
use std::cell::RefCell;
use winit::dpi::{LogicalSize, PhysicalSize};
use winit::event_loop::EventLoopProxy;
use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
use winit::window::Window;

struct SceneBridge {
    proxy: EventLoopProxy<ShellEvent>,
    scene: Option<Retained<UIWindowScene>>,
}

thread_local! {
    static BRIDGE: RefCell<Option<SceneBridge>> = const { RefCell::new(None) };
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
                let bridge = bridge.as_mut()?;
                bridge.scene = Some(scene);
                Some(bridge.proxy.clone())
            });
            if let Some(proxy) = proxy {
                let _ = proxy.send_event(ShellEvent::IosSceneConnected);
            }
        }

        #[unsafe(method(sceneDidDisconnect:))]
        fn disconnect(&self, scene: &UIScene) {
            BRIDGE.with_borrow_mut(|bridge| {
                if let Some(bridge) = bridge
                    && bridge.scene.as_ref().is_some_and(|current| std::ptr::eq(current.as_super(), scene))
                {
                    bridge.scene = None;
                }
            });
        }
    }
);

/// UIKit looks up the delegate by class name in the application's scene manifest.
pub(crate) fn register(proxy: EventLoopProxy<ShellEvent>) {
    let _ = SceneDelegate::class();
    BRIDGE.with_borrow_mut(|bridge| *bridge = Some(SceneBridge { proxy, scene: None }));
}

pub(crate) fn connected() -> bool {
    BRIDGE.with_borrow(|bridge| bridge.as_ref().is_some_and(|bridge| bridge.scene.is_some()))
}

pub(crate) fn attach(window: &Window) -> Result<Retained<UIWindow>, String> {
    let scene = BRIDGE.with_borrow(|bridge| bridge.as_ref()?.scene.clone()).ok_or("the iOS window scene has not connected")?;
    let window = ui_view(window)?.window().ok_or("the UIKit view has no window")?;
    if !window.windowScene().is_some_and(|current| Retained::as_ptr(&current) == Retained::as_ptr(&scene)) {
        window.setWindowScene(Some(&scene));
    }
    Ok(window)
}

/// The Metal surface and touch locations both use the UIKit content view's coordinate space.
pub(crate) fn drawable_size(window: &Window) -> PhysicalSize<u32> {
    let view = ui_view(window).expect("a live iOS window owns its UIKit view");
    let bounds = view.bounds();
    LogicalSize::new(bounds.size.width, bounds.size.height).to_physical(view.contentScaleFactor())
}

fn ui_view(window: &Window) -> Result<Retained<UIView>, String> {
    let handle = window.window_handle().map_err(|why| why.to_string())?;
    let RawWindowHandle::UiKit(handle) = handle.as_raw() else { return Err("the window has no UIKit view".into()) };
    // SAFETY: Called on the UI thread. The winit window keeps its UIKit view alive.
    unsafe { Retained::retain(handle.ui_view.cast().as_ptr()) }.ok_or_else(|| "the UIKit view is missing".into())
}
