use crate::ShellEvent;
use crate::os::apple_preferences;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObject};
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{
    NSAppearanceNameAqua, NSAppearanceNameDarkAqua, NSApplication, NSWorkspace, NSWorkspaceAccessibilityDisplayOptionsDidChangeNotification,
};
use objc2_foundation::{
    NSArray, NSCurrentLocaleDidChangeNotification, NSDictionary, NSKeyValueObservingOptions, NSNotificationCenter,
    NSObjectNSKeyValueObserverRegistration, NSString, ns_string,
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
            let _ = self.ivars().send_event(ShellEvent::SystemPreferencesChanged {
                color_scheme: scheme,
                reduced_motion: None,
                text_scale: None,
                high_contrast: None,
            });
        }
    }
);

pub(super) struct Observer {
    _workspace: apple_preferences::Observer,
    _locale: apple_preferences::Observer,
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
        // SAFETY: AppKit and Foundation expose these as static notification names.
        let (display, locale) =
            unsafe { (NSWorkspaceAccessibilityDisplayOptionsDidChangeNotification, NSCurrentLocaleDidChangeNotification) };
        Self {
            _workspace: apple_preferences::Observer::new(center, &[display], proxy.clone()),
            _locale: apple_preferences::Observer::new(NSNotificationCenter::defaultCenter(), &[locale], proxy),
            appearance,
        }
    }
}

impl Drop for Observer {
    fn drop(&mut self) {
        if let Some((app, observer)) = &self.appearance {
            // SAFETY: This is the same application, observer and key path registered in new.
            unsafe { app.removeObserver_forKeyPath(observer, ns_string!("effectiveAppearance")) };
        }
    }
}
