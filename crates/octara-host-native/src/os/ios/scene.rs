//! Scene lifecycle and viewport geometry from UIKit.

use super::{app, view::NativeWindow};
use crate::os::PlatformEvent;
use objc2::rc::{Retained, Weak};
use objc2::runtime::{AnyObject, NSObject, NSObjectProtocol};
use objc2::{ClassType, DefinedClass, MainThreadMarker, MainThreadOnly, Message, define_class, msg_send, sel};
use objc2_foundation::{NSArray, NSSet, NSURL, NSUserActivity, NSUserActivityTypeBrowsingWeb};
use objc2_ui_kit::{
    UIOpenURLContext, UIScene, UISceneConnectionOptions, UISceneDelegate, UISceneSession, UITraitUserInterfaceStyle, UIView, UIWindowScene,
    UIWindowSceneDelegate,
};
use octara::module::HostLifecycleEvent;
use octara_model::geometry::{Rect, Size, WindowViewport};
use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;

#[derive(Default)]
struct State {
    scene: Option<Retained<UIWindowScene>>,
    windows: Vec<Weak<NativeWindow>>,
    launch_items: Option<OpenedItems>,
    launched: bool,
    legacy: bool,
    active: bool,
    foreground: bool,
}

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State { launch_items: Some(OpenedItems::default()), ..State::default() });
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "OctaraWindowSceneDelegate"]
    pub(super) struct SceneDelegate;

    unsafe impl NSObjectProtocol for SceneDelegate {}

    unsafe impl UIWindowSceneDelegate for SceneDelegate {}

    impl SceneDelegate {
        #[unsafe(method(colorSchemeChanged))]
        fn color_scheme_changed(&self) {
            app::send(PlatformEvent::RefreshSystemPreferences);
        }
    }

    unsafe impl UISceneDelegate for SceneDelegate {
        #[unsafe(method(scene:willConnectToSession:options:))]
        fn connect(&self, scene: &UIScene, _: &UISceneSession, options: &UISceneConnectionOptions) {
            let Ok(scene) = scene.retain().downcast::<UIWindowScene>() else { return };
            if current().is_some() {
                return;
            }
            // SAFETY: UIKit connection properties may be nil on an ordinary launch.
            let contexts: Option<Retained<NSSet<UIOpenURLContext>>> = unsafe { msg_send![options, URLContexts] };
            let activities: Option<Retained<NSSet<NSUserActivity>>> = unsafe { msg_send![options, userActivities] };
            let mut items = contexts.as_deref().map_or_else(OpenedItems::default, OpenedItems::from_contexts);
            if let Some(activities) = activities {
                for url in activities.iter().filter_map(|activity| browsing_url(&activity)) {
                    items.push(&url);
                }
            }
            let traits = NSArray::from_slice(&[UITraitUserInterfaceStyle::class()]);
            // SAFETY: Observe the scene trait with a delegate method of the supported zero-argument form.
            let _: Retained<AnyObject> = unsafe {
                msg_send![&*scene, registerForTraitChanges: &*traits, withTarget: self, action: sel!(colorSchemeChanged)]
            };
            STATE.with_borrow_mut(|state| state.scene = Some(scene.clone()));
            deliver(items);
            for window in windows() {
                window.attach(&scene);
            }
            launch();
        }

        #[unsafe(method(sceneDidDisconnect:))]
        fn disconnect(&self, scene: &UIScene) {
            if owns(scene) {
                activate(false);
                foreground_changed(false);
                STATE.with_borrow_mut(|state| state.scene = None);
                for window in windows() {
                    window.setWindowScene(None);
                }
            }
        }

        #[unsafe(method(sceneDidBecomeActive:))]
        fn did_become_active(&self, scene: &UIScene) {
            if owns(scene) {
                activate(true);
            }
        }

        #[unsafe(method(sceneWillResignActive:))]
        fn will_resign_active(&self, scene: &UIScene) {
            if owns(scene) {
                activate(false);
            }
        }

        #[unsafe(method(sceneWillEnterForeground:))]
        fn enter_foreground(&self, scene: &UIScene) {
            if owns(scene) {
                foreground_changed(true);
            }
        }

        #[unsafe(method(sceneDidEnterBackground:))]
        fn enter_background(&self, scene: &UIScene) {
            if owns(scene) {
                foreground_changed(false);
            }
        }

        #[unsafe(method(scene:openURLContexts:))]
        fn open_urls(&self, scene: &UIScene, contexts: &NSSet<UIOpenURLContext>) {
            if owns(scene) {
                deliver(OpenedItems::from_contexts(contexts));
            }
        }

        #[unsafe(method(scene:continueUserActivity:))]
        fn continue_activity(&self, scene: &UIScene, activity: &NSUserActivity) {
            if owns(scene)
                && let Some(url) = browsing_url(activity)
            {
                open_url(&url);
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
            self.files.extend(url.to_file_path());
        } else {
            self.urls.extend(url.absoluteString().map(|url| url.to_string()));
        }
    }

    fn forward(self) {
        if !self.urls.is_empty() {
            app::send(PlatformEvent::Lifecycle(HostLifecycleEvent::OpenUrls(self.urls)));
        }
        if !self.files.is_empty() {
            app::send(PlatformEvent::Lifecycle(HostLifecycleEvent::OpenFiles(self.files)));
        }
    }
}

fn deliver(mut items: OpenedItems) {
    STATE.with_borrow_mut(|state| {
        if let Some(launch) = &mut state.launch_items {
            launch.urls.append(&mut items.urls);
            launch.files.append(&mut items.files);
        }
    });
    items.forward();
}

pub(super) fn open_url(url: &NSURL) {
    let mut items = OpenedItems::default();
    items.push(url);
    deliver(items);
}

fn browsing_url(activity: &NSUserActivity) -> Option<Retained<NSURL>> {
    // SAFETY: Foundation supplies the universal-link activity type as a static NSString.
    if &*activity.activityType() == unsafe { NSUserActivityTypeBrowsingWeb } { activity.webpageURL() } else { None }
}

fn owns(scene: &UIScene) -> bool {
    STATE.with_borrow(|state| state.scene.as_ref().is_some_and(|current| std::ptr::eq(current.as_super(), scene)))
}

pub(super) fn current() -> Option<Retained<UIWindowScene>> {
    STATE.with_borrow(|state| state.scene.clone())
}

pub(super) fn register(window: &NativeWindow) {
    STATE.with_borrow_mut(|state| {
        state.windows.retain(|window| window.load().is_some());
        state.windows.push(Weak::new(window));
    });
}

fn windows() -> Vec<Retained<NativeWindow>> {
    STATE.with_borrow(|state| state.windows.iter().filter_map(Weak::load).collect())
}

fn launch() {
    let first = STATE.with_borrow_mut(|state| !std::mem::replace(&mut state.launched, true));
    app::send(if first { PlatformEvent::Launched } else { PlatformEvent::PlatformReady });
}

pub(super) fn launch_legacy() {
    STATE.with_borrow_mut(|state| state.legacy = true);
    launch();
}

pub(super) fn ready() -> bool {
    STATE.with_borrow(|state| state.legacy || state.scene.is_some())
}

pub(super) fn foreground() -> bool {
    STATE.with_borrow(|state| state.foreground)
}

pub(super) fn active() -> bool {
    STATE.with_borrow(|state| state.active)
}

pub(super) fn take_launch_items() -> (Vec<String>, Vec<PathBuf>) {
    let items = STATE.with_borrow_mut(|state| state.launch_items.take()).unwrap_or_default();
    (items.urls, items.files)
}

pub(super) fn foreground_changed(foreground: bool) {
    let previous = STATE.with_borrow_mut(|state| std::mem::replace(&mut state.foreground, foreground));
    if previous != foreground {
        let lifecycle = if foreground { HostLifecycleEvent::AppForeground } else { HostLifecycleEvent::AppBackground };
        app::send(PlatformEvent::Lifecycle(lifecycle));
    }
}

pub(super) fn activate(active: bool) {
    let previous = STATE.with_borrow_mut(|state| std::mem::replace(&mut state.active, active));
    if previous != active {
        app::send(if active { PlatformEvent::Resumed } else { PlatformEvent::Suspended });
    }
    if active {
        foreground_changed(true);
    }
    for window in windows() {
        window.update_focus();
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

pub(super) struct AreaConnection(Retained<AreaObserver>);

impl AreaConnection {
    pub(super) fn new(content: &UIView, refresh: Rc<dyn Fn()>) -> Self {
        let mtm = MainThreadMarker::from(content);
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
        observer.leadingAnchor().constraintEqualToAnchor(&content.leadingAnchor()).setActive(true);
        observer.trailingAnchor().constraintEqualToAnchor(&content.trailingAnchor()).setActive(true);
        observer.topAnchor().constraintEqualToAnchor(&content.topAnchor()).setActive(true);
        observer.bottomAnchor().constraintEqualToAnchor(&keyboard.topAnchor()).setActive(true);
        Self(observer)
    }
}

impl Drop for AreaConnection {
    fn drop(&mut self) {
        self.0.removeFromSuperview();
    }
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
