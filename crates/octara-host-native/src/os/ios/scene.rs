//! Attach winit's UIKit window to the application scene and observe its usable area.

use crate::ShellEvent;
use objc2::rc::{Retained, Weak as ObjcWeak};
use objc2::runtime::{NSObject, NSObjectProtocol};
use objc2::{ClassType, DefinedClass, MainThreadMarker, MainThreadOnly, Message, define_class, msg_send};
use objc2_foundation::{NSSet, NSURL, NSUserActivity, NSUserActivityTypeBrowsingWeb};
use objc2_ui_kit::{
    UIApplication, UIOpenURLContext, UIScene, UISceneActivationState, UISceneConnectionOptions, UISceneDelegate, UISceneSession, UIView,
    UIViewController, UIWindow, UIWindowScene,
};
use octara::module::HostLifecycleEvent;
use octara_model::geometry::{Rect, Size, WindowViewport};
use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::{Rc, Weak};
use winit::event_loop::EventLoopProxy;
use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
use winit::window::Window;

#[derive(Default)]
struct SceneBridge {
    proxy: Option<EventLoopProxy<ShellEvent>>,
    scene: Option<Retained<UIWindowScene>>,
    windows: Vec<Weak<SceneWindow>>,
    launch_items: Option<OpenedItems>,
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
        fn connect(&self, scene: &UIScene, _: &UISceneSession, options: &UISceneConnectionOptions) {
            let Ok(scene) = scene.retain().downcast::<UIWindowScene>() else { return };
            // SAFETY: Read UIKit's connection properties as nullable because a launch
            // without URLs or activities can supply nil instead of an empty set.
            let contexts: Option<Retained<NSSet<UIOpenURLContext>>> = unsafe { msg_send![options, URLContexts] };
            let activities: Option<Retained<NSSet<NSUserActivity>>> = unsafe { msg_send![options, userActivities] };
            let mut items = contexts.as_deref().map_or_else(OpenedItems::default, OpenedItems::from_contexts);
            if let Some(activities) = activities {
                for url in activities.iter().filter_map(|activity| browsing_url(&activity)) {
                    items.push(&url);
                }
            }
            let proxy = BRIDGE.with_borrow_mut(|bridge| {
                bridge.scene = Some(scene.clone());
                if let Some(launch) = &mut bridge.launch_items {
                    launch.urls.append(&mut items.urls);
                    launch.files.append(&mut items.files);
                }
                bridge.proxy.clone()
            });
            if let Some(proxy) = proxy {
                let _ = proxy.send_event(ShellEvent::PlatformReady);
                items.forward(&proxy);
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

        #[unsafe(method(sceneWillEnterForeground:))]
        fn enter_foreground(&self, scene: &UIScene) {
            forward_activity(scene, HostLifecycleEvent::AppForeground);
        }

        #[unsafe(method(sceneDidEnterBackground:))]
        fn enter_background(&self, scene: &UIScene) {
            forward_activity(scene, HostLifecycleEvent::AppBackground);
        }

        #[unsafe(method(scene:openURLContexts:))]
        fn open_urls(&self, _: &UIScene, contexts: &NSSet<UIOpenURLContext>) {
            if let Some(proxy) = BRIDGE.with_borrow(|bridge| bridge.proxy.clone()) {
                OpenedItems::from_contexts(contexts).forward(&proxy);
            }
        }

        #[unsafe(method(scene:continueUserActivity:))]
        fn continue_activity(&self, _: &UIScene, activity: &NSUserActivity) {
            if let Some(url) = browsing_url(activity)
                && let Some(proxy) = BRIDGE.with_borrow(|bridge| bridge.proxy.clone())
            {
                let mut items = OpenedItems::default();
                items.push(&url);
                items.forward(&proxy);
            }
        }
    }
);

#[derive(Default)]
struct OpenedItems {
    urls: Vec<String>,
    files: Vec<PathBuf>,
}

impl OpenedItems {
    fn from_contexts(contexts: &NSSet<UIOpenURLContext>) -> Self {
        let mut items = Self::default();
        for context in contexts {
            items.push(&context.URL());
        }
        items
    }

    fn push(&mut self, url: &NSURL) {
        if url.isFileURL() {
            self.files.extend(url.path().map(|path| PathBuf::from(path.to_string())));
        } else {
            self.urls.extend(url.absoluteString().map(|url| url.to_string()));
        }
    }

    fn forward(self, proxy: &EventLoopProxy<ShellEvent>) {
        if !self.urls.is_empty() {
            let _ = proxy.send_event(ShellEvent::Lifecycle(HostLifecycleEvent::OpenUrls(self.urls)));
        }
        if !self.files.is_empty() {
            let _ = proxy.send_event(ShellEvent::Lifecycle(HostLifecycleEvent::OpenFiles(self.files)));
        }
    }
}

fn browsing_url(activity: &NSUserActivity) -> Option<Retained<NSURL>> {
    // SAFETY: This is Foundation's static universal-link activity type.
    if &*activity.activityType() == unsafe { NSUserActivityTypeBrowsingWeb } { activity.webpageURL() } else { None }
}

fn forward_activity(scene: &UIScene, event: HostLifecycleEvent) {
    let proxy = BRIDGE.with_borrow(|bridge| {
        if bridge.scene.as_ref().is_some_and(|current| std::ptr::eq(current.as_super(), scene)) { bridge.proxy.clone() } else { None }
    });
    if let Some(proxy) = proxy {
        let _ = proxy.send_event(ShellEvent::Lifecycle(event));
    }
}

pub(super) fn foreground() -> Option<bool> {
    let scene = BRIDGE.with_borrow(|bridge| bridge.scene.clone())?;
    Some(matches!(scene.activationState(), UISceneActivationState::ForegroundActive | UISceneActivationState::ForegroundInactive))
}

pub(super) fn take_launch_items() -> (Vec<String>, Vec<PathBuf>) {
    let items = BRIDGE.with_borrow_mut(|bridge| bridge.launch_items.take()).unwrap_or_default();
    (items.urls, items.files)
}

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
    BRIDGE.with_borrow_mut(|bridge| {
        bridge.proxy = Some(proxy);
        bridge.launch_items = Some(OpenedItems::default());
    });
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
    geometry: Cell<Option<WindowViewport>>,
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
        let next = self.superview().and_then(|content| view_metrics(&content));
        if self.ivars().geometry.replace(next) != next
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
        observer.setTranslatesAutoresizingMaskIntoConstraints(false);
        content.addSubview(&observer);
        let keyboard = content.keyboardLayoutGuide();
        keyboard.setUsesBottomSafeArea(false);
        // The observer lays out whenever the keyboard guide's top edge moves.
        observer.leadingAnchor().constraintEqualToAnchor(&content.leadingAnchor()).setActive(true);
        observer.trailingAnchor().constraintEqualToAnchor(&content.trailingAnchor()).setActive(true);
        observer.topAnchor().constraintEqualToAnchor(&content.topAnchor()).setActive(true);
        observer.bottomAnchor().constraintEqualToAnchor(&keyboard.topAnchor()).setActive(true);
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

pub(super) fn view_metrics(content: &UIView) -> Option<WindowViewport> {
    let size = content.bounds().size;
    (size.width > 0.0 && size.height > 0.0).then(|| {
        let insets = content.safeAreaInsets();
        let safe_area = Rect::new(
            insets.left as f32,
            insets.top as f32,
            (size.width - insets.left - insets.right).max(0.0) as f32,
            (size.height - insets.top - insets.bottom).max(0.0) as f32,
        );
        WindowViewport {
            size: Size::new(size.width as f32, size.height as f32),
            scale: content.contentScaleFactor() as f32,
            safe_area: Some(safe_area),
            keyboard_inset: keyboard_inset(content),
        }
    })
}

pub(super) fn keyboard_inset(content: &UIView) -> f32 {
    if !content.window().is_some_and(|window| window.isKeyWindow()) {
        return 0.0;
    }
    // The guide and the view bounds share coordinates through scene transitions.
    let height = content.bounds().size.height.max(0.0);
    content.keyboardLayoutGuide().layoutFrame().size.height.clamp(0.0, height) as f32
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
