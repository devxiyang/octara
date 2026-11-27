use super::events::DesktopEvent;
use super::{DesktopHost, DesktopState, DesktopWindowState};
use crate::os::{PlatformEvent, PlatformSender, ShellDriver};
use octara::module::HostLifecycleEvent;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::window::WindowId;

pub(super) struct DesktopProxy {
    pub native: EventLoopProxy<DesktopEvent>,
    wake_pending: AtomicBool,
}

impl DesktopProxy {
    pub(super) fn send(&self, event: PlatformEvent) {
        if matches!(event, PlatformEvent::UserWake) && self.wake_pending.swap(true, Ordering::AcqRel) {
            return;
        }
        let _ = self.native.send_event(DesktopEvent::Platform(event));
    }
}

pub(super) fn run(state: Rc<DesktopState>, driver: Box<dyn ShellDriver>) -> Result<(), String> {
    let event_loop = EventLoop::<DesktopEvent>::with_user_event().build().map_err(|error| error.to_string())?;
    state
        .proxy
        .set(DesktopProxy { native: event_loop.create_proxy(), wake_pending: AtomicBool::new(false) })
        .map_err(|_| "Desktop event loop already initialized")?;
    let proxy = state.proxy.clone();
    let sender = PlatformSender::new(move |event| proxy.get().expect("desktop event loop proxy").send(event));
    #[cfg(target_os = "linux")]
    let _preferences = super::linux_preferences::Observer::start(sender.clone()).ok();
    let mut app = DesktopLoop { state, sender, driver, started: false, error: None };
    let result = event_loop.run_app(&mut app);
    match app.error {
        Some(error) => Err(error),
        None => result.map_err(|error| error.to_string()),
    }
}

struct DesktopLoop {
    state: Rc<DesktopState>,
    sender: PlatformSender,
    driver: Box<dyn ShellDriver>,
    started: bool,
    error: Option<String>,
}

impl DesktopLoop {
    fn window(&self, id: WindowId) -> Option<Rc<DesktopWindowState>> {
        self.state.windows.borrow().get(&id)?.upgrade()
    }

    fn drive<T>(
        &mut self,
        event_loop: &ActiveEventLoop,
        callback: impl FnOnce(&mut dyn ShellDriver, &mut DesktopHost<'_>) -> Result<T, String>,
    ) -> Option<T> {
        if self.error.is_some() || event_loop.exiting() {
            return None;
        }
        // Winit queues user events and serializes these callbacks. No window or input-state
        // borrow is held while dispatching into the shell, which can close or create windows.
        let mut host = DesktopHost { event_loop: Some(event_loop), state: self.state.clone() };
        match callback(self.driver.as_mut(), &mut host) {
            Ok(result) => Some(result),
            Err(error) => {
                self.error = Some(error);
                event_loop.exit();
                None
            }
        }
    }

    fn dispatch(&mut self, event_loop: &ActiveEventLoop, event: PlatformEvent) {
        let _ = self.drive(event_loop, |driver, host| driver.event(host, event));
    }
}

impl ApplicationHandler<DesktopEvent> for DesktopLoop {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.started {
            self.dispatch(event_loop, PlatformEvent::Resumed);
            return;
        }
        self.started = true;
        let sender = self.sender.clone();
        if self.drive(event_loop, |driver, host| driver.start(host, sender)).is_some() {
            self.dispatch(event_loop, PlatformEvent::Launched);
        }
    }

    fn suspended(&mut self, event_loop: &ActiveEventLoop) {
        self.dispatch(event_loop, PlatformEvent::Suspended);
    }

    fn memory_warning(&mut self, event_loop: &ActiveEventLoop) {
        self.dispatch(event_loop, PlatformEvent::Lifecycle(HostLifecycleEvent::MemoryWarning));
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        let event = self.window(id).and_then(|native| {
            native.accessibility.borrow_mut().adapter.process_event(&native.window, &event);
            native.input.borrow_mut().translate(native.id, &native.window, event)
        });
        if let Some(event) = event {
            self.dispatch(event_loop, event);
        }
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: DesktopEvent) {
        let event = match event {
            DesktopEvent::Platform(event) => {
                if matches!(event, PlatformEvent::UserWake) {
                    self.state.proxy.get().expect("desktop event loop proxy").wake_pending.swap(false, Ordering::AcqRel);
                }
                event
            }
            DesktopEvent::Accessibility(event) => {
                use accesskit_winit::WindowEvent as AccessibilityEvent;

                let Some(window) = self.window(event.window_id).map(|native| native.id) else { return };
                match event.window_event {
                    AccessibilityEvent::InitialTreeRequested => PlatformEvent::AccessibilityEnabledChanged { window, enabled: true },
                    AccessibilityEvent::AccessibilityDeactivated => PlatformEvent::AccessibilityEnabledChanged { window, enabled: false },
                    AccessibilityEvent::ActionRequested(request) => PlatformEvent::AccessKitAction { window, request },
                }
            }
        };
        self.dispatch(event_loop, event);
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        let Some(state) = self.drive(event_loop, |driver, host| driver.idle(host)) else { return };
        if state.exit {
            event_loop.exit();
        } else {
            event_loop.set_control_flow(state.deadline.map_or(ControlFlow::Wait, ControlFlow::WaitUntil));
        }
    }

    fn exiting(&mut self, event_loop: &ActiveEventLoop) {
        let mut host = DesktopHost { event_loop: Some(event_loop), state: self.state.clone() };
        if self.started && self.error.is_none() {
            self.error = self.driver.event(&mut host, PlatformEvent::WillTerminate).err();
        }
        if let Err(error) = self.driver.shutdown(&mut host)
            && self.error.is_none()
        {
            self.error = Some(error);
        }
    }
}
