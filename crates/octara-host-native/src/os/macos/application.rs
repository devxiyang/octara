//! AppKit application delegate and main run-loop integration.

use super::MacHost;
use super::view::DragEvent;
use crate::os::{HostWake, LoopState, OsHost, PlatformEvent, PlatformSender, ShellDriver};
use objc2::rc::{Retained, autoreleasepool};
use objc2::runtime::{NSObject, NSObjectProtocol, ProtocolObject};
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{
    NSApplication, NSApplicationActivationPolicy, NSApplicationDelegate, NSApplicationDelegateReply, NSApplicationTerminateReply, NSEvent,
    NSEventModifierFlags, NSEventType, NSRunningApplication,
};
use objc2_core_foundation::{
    CFAbsoluteTimeGetCurrent, CFRetained, CFRunLoop, CFRunLoopSource, CFRunLoopSourceContext, CFRunLoopTimer, CFRunLoopTimerContext,
    kCFRunLoopCommonModes,
};
use objc2_foundation::{NSArray, NSNotification, NSPoint, NSString, NSURL};
use octara::module::HostLifecycleEvent;
use std::any::Any;
use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::ffi::{CStr, OsStr, c_void};
use std::os::unix::ffi::OsStrExt;
use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};
use std::path::PathBuf;
use std::rc::{Rc, Weak};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::time::Instant;

static APPKIT_LAUNCHED: AtomicBool = AtomicBool::new(false);

struct RunLoopSignal {
    source: CFRetained<CFRunLoopSource>,
    run_loop: CFRetained<CFRunLoop>,
}

// SAFETY: The retained CF objects are used across threads only to signal the source and wake the
// loop. Its callback is installed in the main run loop and removed before its context is released.
unsafe impl Send for RunLoopSignal {}

impl RunLoopSignal {
    fn wake(&self) {
        self.source.signal();
        self.run_loop.wake_up();
    }
}

#[derive(Default)]
struct Inbox {
    events: VecDeque<PlatformEvent>,
    user_wake: bool,
    signal: Option<RunLoopSignal>,
    closed: bool,
}

#[derive(Clone, Default)]
pub(super) struct EventQueue(Arc<Mutex<Inbox>>);

impl EventQueue {
    fn push(&self, event: PlatformEvent) {
        let mut inbox = self.0.lock().unwrap();
        if inbox.closed {
            return;
        }
        if matches!(event, PlatformEvent::UserWake) && std::mem::replace(&mut inbox.user_wake, true) {
            return;
        }
        inbox.events.push_back(event);
        if let Some(signal) = &inbox.signal {
            signal.wake();
        }
    }

    pub(super) fn sender(&self) -> PlatformSender {
        let queue = self.clone();
        PlatformSender::new(move |event| queue.push(event))
    }

    pub(super) fn wake(&self) -> HostWake {
        self.sender().wake()
    }

    fn take(&self) -> VecDeque<PlatformEvent> {
        let mut inbox = self.0.lock().unwrap();
        inbox.user_wake = false;
        std::mem::take(&mut inbox.events)
    }

    fn resignal(&self) {
        let inbox = self.0.lock().unwrap();
        if !inbox.events.is_empty()
            && let Some(signal) = &inbox.signal
        {
            signal.wake();
        }
    }

    fn close(&self) {
        let mut inbox = self.0.lock().unwrap();
        inbox.closed = true;
        inbox.signal = None;
        inbox.events.clear();
    }
}

pub(super) struct Launch {
    ready: Cell<bool>,
    termination_requested: Cell<bool>,
    items: RefCell<Option<(Vec<String>, Vec<PathBuf>)>>,
}

impl Default for Launch {
    fn default() -> Self {
        Self { ready: Cell::default(), termination_requested: Cell::default(), items: RefCell::new(Some((Vec::new(), Vec::new()))) }
    }
}

impl Launch {
    pub(super) fn take_items(&self) -> (Vec<String>, Vec<PathBuf>) {
        self.items.take().unwrap_or_default()
    }
}

struct AppDelegateState {
    launch: Rc<Launch>,
    sender: PlatformSender,
}

impl AppDelegateState {
    fn open(&self, urls: Vec<String>, files: Vec<PathBuf>) {
        if let Some(items) = self.launch.items.borrow_mut().as_mut() {
            items.0.extend(urls);
            items.1.extend(files);
            return;
        }
        if !urls.is_empty() {
            self.sender.send_event(PlatformEvent::Lifecycle(HostLifecycleEvent::OpenUrls(urls)));
        }
        if !files.is_empty() {
            self.sender.send_event(PlatformEvent::Lifecycle(HostLifecycleEvent::OpenFiles(files)));
        }
    }

    fn launched(&self) {
        if !self.launch.ready.replace(true) {
            self.sender.send_event(PlatformEvent::Launched);
        }
    }
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements, and the delegate is retained by run().
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "OctaraAppDelegate"]
    #[ivars = AppDelegateState]
    struct AppDelegate;

    unsafe impl NSObjectProtocol for AppDelegate {}

    unsafe impl NSApplicationDelegate for AppDelegate {
        #[unsafe(method(applicationDidFinishLaunching:))]
        fn did_finish_launching(&self, _: &NSNotification) {
            APPKIT_LAUNCHED.store(true, Ordering::Relaxed);
            self.ivars().launched();
        }

        #[unsafe(method(application:openURLs:))]
        fn open_urls(&self, _: &NSApplication, urls: &NSArray<NSURL>) {
            let mut links = Vec::new();
            let mut files = Vec::new();
            for url in urls {
                if url.isFileURL() {
                    // SAFETY: NSURL supplies a NUL-terminated path for this live file URL.
                    let bytes = unsafe { CStr::from_ptr(url.fileSystemRepresentation().as_ptr()) }.to_bytes();
                    files.push(PathBuf::from(OsStr::from_bytes(bytes)));
                } else if let Some(link) = url.absoluteString() {
                    links.push(link.to_string());
                }
            }
            self.ivars().open(links, files);
        }

        #[unsafe(method(application:openFiles:))]
        fn open_files(&self, app: &NSApplication, files: &NSArray<NSString>) {
            self.ivars().open(Vec::new(), files.iter().map(|path| PathBuf::from(path.to_string())).collect());
            app.replyToOpenOrPrint(NSApplicationDelegateReply::Success);
        }

        #[unsafe(method(applicationShouldHandleReopen:hasVisibleWindows:))]
        fn should_handle_reopen(&self, _: &NSApplication, has_visible_windows: bool) -> bool {
            self.ivars().sender.send_event(PlatformEvent::Lifecycle(HostLifecycleEvent::DockReopen { has_visible_windows }));
            true
        }

        #[unsafe(method(applicationDidBecomeActive:))]
        fn did_become_active(&self, _: &NSNotification) {
            self.ivars().sender.send_event(PlatformEvent::Lifecycle(HostLifecycleEvent::AppForeground));
        }

        #[unsafe(method(applicationDidResignActive:))]
        fn did_resign_active(&self, _: &NSNotification) {
            self.ivars().sender.send_event(PlatformEvent::Lifecycle(HostLifecycleEvent::AppBackground));
        }

        #[unsafe(method(applicationShouldTerminate:))]
        fn should_terminate(&self, _: &NSApplication) -> NSApplicationTerminateReply {
            if !self.ivars().launch.ready.get() {
                return NSApplicationTerminateReply::TerminateNow;
            }
            // AppKit waits in its modal run-loop mode until the shell finishes cleanup.
            if !self.ivars().launch.termination_requested.replace(true) {
                self.ivars().sender.send_event(PlatformEvent::WillTerminate);
            }
            NSApplicationTerminateReply::TerminateLater
        }
    }
);

struct Session {
    host: Box<MacHost>,
    driver: Box<dyn ShellDriver>,
    started: bool,
}

impl Session {
    fn shutdown(&mut self) -> Result<(), String> {
        if !std::mem::take(&mut self.started) {
            return Ok(());
        }
        self.driver.shutdown(self.host.as_mut())
    }
}

pub(super) struct Application {
    app: Retained<NSApplication>,
    launch: Rc<Launch>,
    queue: EventQueue,
    session: RefCell<Option<Session>>,
    stopped: Cell<bool>,
    error: Cell<Option<String>>,
    panic: Cell<Option<Box<dyn Any + Send>>>,
    timer: RefCell<Option<(Instant, CFRetained<CFRunLoopTimer>)>>,
}

impl Application {
    pub(super) fn dispatch(&self, event: PlatformEvent, drag: Option<DragEvent<'_>>) -> bool {
        self.process(Some((event, drag)))
    }

    fn process(&self, synchronous: Option<(PlatformEvent, Option<DragEvent<'_>>)>) -> bool {
        if self.stopped.get() {
            return false;
        }
        if !self.launch.ready.get() {
            if let Some((event, _)) = synchronous {
                self.queue.push(event);
            }
            return false;
        }
        let session = self.session.borrow_mut().take();
        let Some(mut session) = session else {
            // AppKit can reenter from modal loops or synchronous delegate calls during the shell.
            if let Some((event, _)) = synchronous {
                self.queue.push(event);
            }
            return false;
        };
        // Hand a Rust panic back after AppKit leaves its native run loop.
        let result = catch_unwind(AssertUnwindSafe(|| self.cycle(&mut session, synchronous)));
        self.session.replace(Some(session));
        match result {
            Ok(Ok((consumed, state))) => {
                if state.exit {
                    self.stop();
                } else {
                    self.arm_deadline(state.deadline);
                    self.queue.resignal();
                }
                consumed
            }
            Ok(Err(error)) => {
                self.error.set(Some(error));
                self.stop();
                false
            }
            Err(panic) => {
                self.panic.set(Some(panic));
                self.stop();
                false
            }
        }
    }

    fn cycle(
        &self,
        session: &mut Session,
        synchronous: Option<(PlatformEvent, Option<DragEvent<'_>>)>,
    ) -> Result<(bool, LoopState), String> {
        let Session { host, driver, started } = session;
        let host = host.as_mut();
        if !*started {
            *started = true;
            driver.start(host, self.queue.sender())?;
        }
        let mut consumed = false;
        let queued = self.queue.take().into_iter().map(|event| (event, None, false));
        let direct = synchronous.map(|(event, drag)| (event, drag, true));
        for (event, drag, direct) in queued.chain(direct) {
            let terminating = matches!(event, PlatformEvent::WillTerminate);
            let deliver = || driver.event(host, event);
            let handled = match drag {
                Some(drag) => drag.dispatch(deliver),
                None => deliver(),
            }?;
            if terminating {
                return Ok((false, LoopState { exit: true, deadline: None }));
            }
            if direct {
                consumed = handled;
            }
        }
        Ok((consumed, driver.idle(host)?))
    }

    fn stop(&self) {
        self.stopped.set(true);
        if self.launch.termination_requested.replace(false) {
            if let Some(mut session) = self.session.take()
                && let Err(error) = session.shutdown()
            {
                session.host.report_error(&error);
            }
            self.app.replyToApplicationShouldTerminate(true);
        }
        self.app.stop(None);
        // stop() takes effect after AppKit dispatches an event. Wake its event fetch as well.
        if let Some(event) = NSEvent::otherEventWithType_location_modifierFlags_timestamp_windowNumber_context_subtype_data1_data2(
            NSEventType::ApplicationDefined,
            NSPoint::ZERO,
            NSEventModifierFlags::empty(),
            0.0,
            0,
            None,
            0,
            0,
            0,
        ) {
            self.app.postEvent_atStart(&event, true);
        }
    }

    fn arm_deadline(&self, deadline: Option<Instant>) {
        if self.timer.borrow().as_ref().is_some_and(|(current, timer)| Some(*current) == deadline && timer.is_valid()) {
            return;
        }
        if let Some((_, timer)) = self.timer.take() {
            timer.invalidate();
        }
        let Some(deadline) = deadline else { return };
        let mut context = CFRunLoopTimerContext {
            version: 0,
            info: std::ptr::from_ref(self).cast_mut().cast(),
            retain: None,
            release: None,
            copyDescription: None,
        };
        let fire = CFAbsoluteTimeGetCurrent() + deadline.saturating_duration_since(Instant::now()).as_secs_f64();
        // SAFETY: The timer runs on the main loop and is invalidated before Application is dropped.
        let timer = unsafe { CFRunLoopTimer::new(None, fire, 0.0, 0, 0, Some(timer_fired), &mut context) };
        if let Some(run_loop) = CFRunLoop::main() {
            // SAFETY: kCFRunLoopCommonModes is a process-lifetime Core Foundation constant.
            run_loop.add_timer(timer.as_deref(), unsafe { kCFRunLoopCommonModes });
        }
        self.timer.replace(timer.map(|timer| (deadline, timer)));
    }
}

unsafe extern "C-unwind" fn perform(info: *mut c_void) {
    // SAFETY: run() retains the Application, and this source is only scheduled on the main loop.
    let application = unsafe { &*info.cast::<Application>() };
    autoreleasepool(|_| application.process(None));
}

unsafe extern "C-unwind" fn timer_fired(_: *mut CFRunLoopTimer, info: *mut c_void) {
    // SAFETY: Application is retained by run() and invalidates its timer before it is dropped.
    let application = unsafe { &*info.cast::<Application>() };
    application.arm_deadline(None);
    autoreleasepool(|_| application.process(None));
}

pub(super) fn run(mut host: Box<MacHost>, driver: Box<dyn ShellDriver>) -> Result<(), String> {
    let mtm = MainThreadMarker::new().ok_or("AppKit must run on the main thread")?;
    let app = NSApplication::sharedApplication(mtm);
    if app.isRunning() {
        return Err("AppKit is already running".into());
    }
    super::super::scheduling::interactive();
    let application = Rc::new_cyclic(|application: &Weak<Application>| {
        host.application = application.clone();
        Application {
            app: app.clone(),
            launch: host.launch.clone(),
            queue: host.queue.clone(),
            session: RefCell::new(Some(Session { host, driver, started: false })),
            stopped: Cell::new(false),
            error: Cell::default(),
            panic: Cell::default(),
            timer: RefCell::default(),
        }
    });
    let run_loop = CFRunLoop::main().ok_or("AppKit main run loop is unavailable")?;
    let mut context = CFRunLoopSourceContext {
        version: 0,
        info: Rc::as_ptr(&application).cast_mut().cast(),
        retain: None,
        release: None,
        copyDescription: None,
        equal: None,
        hash: None,
        schedule: None,
        cancel: None,
        perform: Some(perform),
    };
    // SAFETY: The source context borrows the retained Application until the source is invalidated.
    let source = unsafe { CFRunLoopSource::new(None, 0, &mut context) }.ok_or("could not create AppKit wake source")?;
    // SAFETY: kCFRunLoopCommonModes is a process-lifetime Core Foundation constant.
    run_loop.add_source(Some(&source), unsafe { kCFRunLoopCommonModes });
    application.queue.0.lock().unwrap().signal = Some(RunLoopSignal { source: source.clone(), run_loop });
    let _cleanup = std::mem::DropGuard::new((), |()| {
        app.setDelegate(None);
        app.setWindowsMenu(None);
        app.setMainMenu(None);
        application.arm_deadline(None);
        application.queue.close();
        source.invalidate();
    });
    let sender = application.queue.sender();
    let delegate = mtm.alloc::<AppDelegate>().set_ivars(AppDelegateState { launch: application.launch.clone(), sender: sender.clone() });
    // SAFETY: The allocated AppDelegate has NSObject as its superclass and initialized ivars.
    let delegate: Retained<AppDelegate> = unsafe { msg_send![super(delegate), init] };
    app.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
    if NSRunningApplication::currentApplication().bundleIdentifier().is_none() {
        app.setActivationPolicy(NSApplicationActivationPolicy::Regular);
    }
    let _preferences = super::preferences::Observer::new(sender);
    application.session.borrow_mut().as_mut().unwrap().host.set_menus(&[]);
    if APPKIT_LAUNCHED.load(Ordering::Relaxed) {
        delegate.ivars().launched();
    }
    app.run();
    application.stopped.set(true);
    let result = application.session.take().map_or(Ok(()), |mut session| session.shutdown());
    app.setDelegate(None);
    if let Some(panic) = application.panic.take() {
        resume_unwind(panic);
    }
    application.error.take().map_or(result, Err)
}
