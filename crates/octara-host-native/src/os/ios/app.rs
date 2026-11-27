//! UIApplication owns the main loop. Worker events are delivered on its main queue.

use super::{IosHost, scene};
use crate::os::{HostWake, OsHost, PlatformEvent, PlatformSender, ShellDriver};
use dispatch2::{DispatchQueue, DispatchTime};
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObject, NSObjectProtocol};
use objc2::{ClassType, MainThreadMarker, MainThreadOnly, define_class};
use objc2_foundation::{NSBundle, NSDictionary, NSString, NSURL};
use objc2_ui_kit::{UIApplication, UIApplicationDelegate, UISceneConfiguration, UISceneConnectionOptions, UISceneSession, UIWindowScene};
use octara::module::HostLifecycleEvent;
use std::cell::RefCell;
use std::collections::VecDeque;
use std::ffi::CString;
use std::ptr::NonNull;
use std::sync::{Arc, Mutex};
use std::time::Instant;

#[derive(Default)]
struct Events {
    pending: VecDeque<PlatformEvent>,
    scheduled: bool,
    wake_pending: bool,
}

#[derive(Default)]
pub(super) struct Queue(Mutex<Events>);

impl Queue {
    pub(super) fn sender(self: &Arc<Self>) -> PlatformSender {
        let queue = self.clone();
        PlatformSender::new(move |event| queue.send(event))
    }

    pub(super) fn wake(self: &Arc<Self>) -> HostWake {
        self.sender().wake()
    }

    fn send(self: &Arc<Self>, event: PlatformEvent) {
        let mut events = self.0.lock().expect("UIKit event queue");
        if matches!(event, PlatformEvent::UserWake) {
            if events.wake_pending {
                return;
            }
            events.wake_pending = true;
        }
        events.pending.push_back(event);
        if !events.scheduled {
            events.scheduled = true;
            drop(events);
            DispatchQueue::main().exec_async(drain);
        }
    }

    fn pop(&self) -> Option<PlatformEvent> {
        let mut events = self.0.lock().expect("UIKit event queue");
        let event = events.pending.pop_front();
        if matches!(event, Some(PlatformEvent::UserWake)) {
            events.wake_pending = false;
        }
        event
    }

    fn begin_drain(&self) {
        self.0.lock().expect("UIKit event queue").scheduled = false;
    }
}

struct RunState {
    host: Box<IosHost>,
    driver: Box<dyn ShellDriver>,
    started: bool,
    deadline: Option<Instant>,
    ticket: u64,
}

thread_local! {
    static RUN: RefCell<Option<RunState>> = const { RefCell::new(None) };
    static QUEUE: RefCell<Option<Arc<Queue>>> = const { RefCell::new(None) };
}

pub(super) fn run(host: Box<IosHost>, driver: Box<dyn ShellDriver>) -> Result<(), String> {
    let _main = MainThreadMarker::new().ok_or("UIKit requires the application main thread")?;
    if QUEUE.with_borrow(Option::is_some) {
        return Err("UIKit is already running".into());
    }
    let arguments: Vec<_> =
        std::env::args_os().map(|arg| CString::new(arg.as_encoded_bytes()).expect("process arguments cannot contain NUL")).collect();
    let argc = i32::try_from(arguments.len()).map_err(|_| "too many UIKit launch arguments")?;
    let mut argv: Vec<_> = arguments.iter().map(|arg| arg.as_ptr().cast_mut()).collect();
    argv.push(std::ptr::null_mut());

    QUEUE.with_borrow_mut(|slot| *slot = Some(host.events.clone()));
    RUN.with_borrow_mut(|slot| *slot = Some(RunState { host, driver, started: false, deadline: None, ticket: 0 }));
    let delegate = NSString::from_str(AppDelegate::class().name().to_str().expect("UIKit delegate name is ASCII"));
    let _ = scene::SceneDelegate::class();
    // SAFETY: UIKit runs on the main thread. The arguments and class name live through the call.
    #[allow(deprecated)]
    let code = unsafe { objc2_ui_kit::UIApplicationMain(argc, NonNull::new(argv.as_mut_ptr()).unwrap(), None, Some(&delegate)) };
    shutdown();
    QUEUE.with_borrow_mut(|slot| *slot = None);
    if code == 0 { Ok(()) } else { Err(format!("UIApplicationMain exited with status {code}")) }
}

pub(super) fn send(event: PlatformEvent) {
    if let Some(queue) = QUEUE.with_borrow(Clone::clone) {
        queue.send(event);
    }
}

/// Answer UIKit's responder before it applies its fallback. Reentrant callbacks
/// join the queue while the driver owns the host.
pub(super) fn send_key(event: PlatformEvent) -> bool {
    let Some(mut state) = RUN.with_borrow_mut(Option::take) else {
        send(event);
        return false;
    };
    if !state.started {
        RUN.with_borrow_mut(|slot| *slot = Some(state));
        return false;
    }
    let result = (|| {
        while let Some(pending) = state.host.events.pop() {
            state.driver.event(&mut *state.host, pending)?;
        }
        state.driver.event(&mut *state.host, event)
    })();
    RUN.with_borrow_mut(|slot| *slot = Some(state));
    send(PlatformEvent::UserWake);
    match result {
        Ok(handled) => handled,
        Err(error) => finish(Some(error)),
    }
}

fn drain() {
    let Some(queue) = QUEUE.with_borrow(Clone::clone) else { return };
    queue.begin_drain();
    let Some(mut state) = RUN.with_borrow_mut(Option::take) else { return };
    let result = (|| {
        if !state.started {
            if !scene::ready() {
                return Ok(false);
            }
            state.host.observe_preferences();
            state.driver.start(&mut *state.host, queue.sender())?;
            state.started = true;
        }
        while let Some(event) = queue.pop() {
            state.driver.event(&mut *state.host, event)?;
        }
        let next = state.driver.idle(&mut *state.host)?;
        if state.deadline != next.deadline {
            state.deadline = next.deadline;
            state.ticket = state.ticket.wrapping_add(1);
            if let Some(deadline) = next.deadline {
                let ticket = state.ticket;
                let delay = DispatchTime::try_from(deadline.saturating_duration_since(Instant::now())).unwrap_or(DispatchTime::FOREVER);
                let _ = DispatchQueue::main().after(delay, move || {
                    let current = RUN.with_borrow_mut(|slot| {
                        slot.as_mut().is_some_and(|state| {
                            if state.ticket != ticket {
                                return false;
                            }
                            state.deadline = None;
                            true
                        })
                    });
                    if current {
                        drain();
                    }
                });
            }
        }
        Ok(next.exit)
    })();
    RUN.with_borrow_mut(|slot| *slot = Some(state));
    match result {
        Ok(false) => {}
        Ok(true) => finish(None),
        Err(error) => finish(Some(error)),
    }
}

fn shutdown() {
    if let Some(mut state) = RUN.with_borrow_mut(Option::take)
        && state.started
    {
        if let Err(error) = state.driver.event(&mut *state.host, PlatformEvent::WillTerminate) {
            state.host.report_error(&error);
        }
        if let Err(error) = state.driver.shutdown(&mut *state.host) {
            state.host.report_error(&error);
        }
    }
}

fn finish(error: Option<String>) -> ! {
    if let Some(error) = &error {
        RUN.with_borrow(|slot| {
            if let Some(state) = slot {
                state.host.report_error(error);
            }
        });
    }
    shutdown();
    // UIApplicationMain does not return when an application-driven loop is finished.
    std::process::exit(i32::from(error.is_some()));
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "OctaraAppDelegate"]
    struct AppDelegate;

    unsafe impl NSObjectProtocol for AppDelegate {}

    #[allow(deprecated)]
    unsafe impl UIApplicationDelegate for AppDelegate {
        #[unsafe(method(application:didFinishLaunchingWithOptions:))]
        unsafe fn launch(&self, _: &UIApplication, options: Option<&NSDictionary<NSString, AnyObject>>) -> bool {
            if NSBundle::mainBundle().objectForInfoDictionaryKey(&NSString::from_str("UIApplicationSceneManifest")).is_none() {
                // The application lifecycle is also supported for bundles targeting
                // systems where a scene manifest was not mandatory.
                if let Some(url) = options
                    .and_then(|options| options.objectForKey(&NSString::from_str("UIApplicationLaunchOptionsURLKey")))
                    .and_then(|item| item.downcast::<NSURL>().ok())
                {
                    scene::open_url(&url);
                }
                scene::launch_legacy();
            }
            true
        }

        #[unsafe(method_id(application:configurationForConnectingSceneSession:options:))]
        fn configuration(
            &self,
            _: &UIApplication,
            session: &UISceneSession,
            _: &UISceneConnectionOptions,
        ) -> Retained<UISceneConfiguration> {
            let configuration = UISceneConfiguration::initWithName_sessionRole(self.mtm().alloc(), None, &session.role());
            // SAFETY: The scene and delegate implement UIKit's application-scene contracts.
            unsafe {
                configuration.setSceneClass(Some(UIWindowScene::class()));
                configuration.setDelegateClass(Some(scene::SceneDelegate::class()));
            }
            configuration
        }

        #[unsafe(method(applicationDidBecomeActive:))]
        fn did_become_active(&self, _: &UIApplication) {
            scene::activate(true);
        }

        #[unsafe(method(applicationWillResignActive:))]
        fn will_resign_active(&self, _: &UIApplication) {
            scene::activate(false);
        }

        #[unsafe(method(applicationWillEnterForeground:))]
        fn will_enter_foreground(&self, _: &UIApplication) {
            scene::foreground_changed(true);
        }

        #[unsafe(method(applicationDidEnterBackground:))]
        fn did_enter_background(&self, _: &UIApplication) {
            scene::foreground_changed(false);
        }

        #[unsafe(method(application:openURL:options:))]
        unsafe fn open_url(&self, _: &UIApplication, url: &NSURL, _: &NSDictionary<NSString, AnyObject>) -> bool {
            scene::open_url(url);
            true
        }

        #[unsafe(method(applicationDidReceiveMemoryWarning:))]
        fn memory_warning(&self, _: &UIApplication) {
            send(PlatformEvent::Lifecycle(HostLifecycleEvent::MemoryWarning));
        }

        #[unsafe(method(applicationWillTerminate:))]
        fn will_terminate(&self, _: &UIApplication) {
            shutdown();
        }
    }
);
