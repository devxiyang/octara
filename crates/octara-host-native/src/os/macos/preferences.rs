use crate::ShellEvent;
use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObject, NSObjectProtocol, ProtocolObject};
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{
    NSAppearanceNameAqua, NSAppearanceNameDarkAqua, NSApplication, NSWorkspace, NSWorkspaceAccessibilityDisplayOptionsDidChangeNotification,
};
use objc2_foundation::{
    NSArray, NSDictionary, NSKeyValueObservingOptions, NSNotificationCenter, NSObjectNSKeyValueObserverRegistration, NSString, ns_string,
};
use octara_model::theme::ColorScheme;
use std::ffi::c_void;
use winit::event_loop::EventLoopProxy;

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "OctaraApplicationAppearanceObserver"]
    #[ivars = EventLoopProxy<ShellEvent>]
    struct AppearanceObserver;

    impl AppearanceObserver {
        #[unsafe(method(observeValueForKeyPath:ofObject:change:context:))]
        fn changed(&self, _: &NSString, _: Option<&AnyObject>, _: Option<&NSDictionary>, _: *mut c_void) {
            let app = NSApplication::sharedApplication(MainThreadMarker::from(self));
            // SAFETY: These are AppKit's static appearance names.
            let names = unsafe { NSArray::from_slice(&[NSAppearanceNameDarkAqua, NSAppearanceNameAqua]) };
            let scheme = app.effectiveAppearance().bestMatchFromAppearancesWithNames(&names).map(|name| {
                // SAFETY: The name is a static AppKit constant.
                if &*name == unsafe { NSAppearanceNameDarkAqua } { ColorScheme::Dark } else { ColorScheme::Light }
            });
            let _ = self.ivars().send_event(ShellEvent::SystemPreferencesChanged { color_scheme: scheme, reduced_motion: None });
        }
    }
);

pub(super) struct Observer {
    center: Retained<NSNotificationCenter>,
    token: Retained<ProtocolObject<dyn NSObjectProtocol>>,
    appearance: Option<(Retained<NSApplication>, Retained<AppearanceObserver>)>,
}

impl Observer {
    pub(super) fn new(proxy: EventLoopProxy<ShellEvent>) -> Self {
        let center = NSWorkspace::sharedWorkspace().notificationCenter();
        // Windows with an explicit appearance suppress winit's system theme events.
        let appearance = MainThreadMarker::new().map(|main| {
            let app = NSApplication::sharedApplication(main);
            // SAFETY: The observer is an NSObject initialized on AppKit's main thread.
            let observer: Retained<AppearanceObserver> =
                unsafe { msg_send![super(main.alloc::<AppearanceObserver>().set_ivars(proxy.clone())), init] };
            // SAFETY: The observer accepts AppKit's KVO callback and is unregistered before release.
            unsafe {
                app.addObserver_forKeyPath_options_context(
                    &observer,
                    ns_string!("effectiveAppearance"),
                    NSKeyValueObservingOptions::empty(),
                    std::ptr::null_mut(),
                );
            }
            (app, observer)
        });
        let callback = RcBlock::new(move |_| {
            let _ = proxy.send_event(ShellEvent::RefreshReducedMotion);
        });
        // SAFETY: The notification is posted by NSWorkspace and the callback only uses the thread-safe event proxy.
        let token = unsafe {
            center.addObserverForName_object_queue_usingBlock(
                Some(NSWorkspaceAccessibilityDisplayOptionsDidChangeNotification),
                None,
                None,
                &callback,
            )
        };
        Self { center, token, appearance }
    }
}

impl Drop for Observer {
    fn drop(&mut self) {
        // SAFETY: This is the token returned by the same notification center.
        unsafe { self.center.removeObserver((*self.token).as_ref()) };
        if let Some((app, observer)) = &self.appearance {
            // SAFETY: This is the same application, observer and key path registered in new.
            unsafe { app.removeObserver_forKeyPath(observer, ns_string!("effectiveAppearance")) };
        }
    }
}
