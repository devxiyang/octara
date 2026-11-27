use crate::os::apple_preferences;
use crate::os::{PlatformEvent, PlatformSender};
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

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "OctaraApplicationAppearanceObserver"]
    #[ivars = PlatformSender]
    struct AppearanceObserver;

    impl AppearanceObserver {
        #[unsafe(method(observeValueForKeyPath:ofObject:change:context:))]
        fn changed(&self, _: &NSString, _: Option<&AnyObject>, _: Option<&NSDictionary>, _: *mut c_void) {
            self.ivars().send_event(PlatformEvent::SystemPreferencesChanged {
                color_scheme: color_scheme(),
                reduced_motion: None,
                text_scale: None,
                high_contrast: None,
            });
        }
    }
);

pub(super) fn color_scheme() -> Option<ColorScheme> {
    let app = NSApplication::sharedApplication(MainThreadMarker::new()?);
    // SAFETY: These are AppKit's static appearance names.
    let names = unsafe { NSArray::from_slice(&[NSAppearanceNameDarkAqua, NSAppearanceNameAqua]) };
    app.effectiveAppearance().bestMatchFromAppearancesWithNames(&names).map(|name| {
        // SAFETY: The comparison is against a static AppKit constant.
        if &*name == unsafe { NSAppearanceNameDarkAqua } { ColorScheme::Dark } else { ColorScheme::Light }
    })
}

pub(super) struct Observer {
    _workspace: apple_preferences::Observer,
    _locale: apple_preferences::Observer,
    appearance: Option<(Retained<NSApplication>, Retained<AppearanceObserver>)>,
}

impl Observer {
    pub(super) fn new(sender: PlatformSender) -> Self {
        let center = NSWorkspace::sharedWorkspace().notificationCenter();
        // Observe the application appearance independently of each window’s explicit theme.
        let appearance = MainThreadMarker::new().map(|main| {
            let app = NSApplication::sharedApplication(main);
            // SAFETY: The observer is an NSObject initialized on AppKit's main thread.
            let observer: Retained<AppearanceObserver> =
                unsafe { msg_send![super(main.alloc::<AppearanceObserver>().set_ivars(sender.clone())), init] };
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
            _workspace: apple_preferences::Observer::new(center, &[display], sender.clone()),
            _locale: apple_preferences::Observer::new(NSNotificationCenter::defaultCenter(), &[locale], sender),
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
