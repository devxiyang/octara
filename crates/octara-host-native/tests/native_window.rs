//! Run on a desktop session with `OCTARA_TEST_NATIVE_WINDOW=1`.

use octara::model::capture::FrameReceipt;
use octara::model::identity::NativeId;
use octara::model::native::{NativePlacement, NativeSlot, NativeSnapshotRequest, TypedNativeProps};
use octara::model::theme::{Color, ColorToken, Theme};
use octara::module::{
    CallTicket, HostLifecycleEvent, HostSurface, HostSurfaceKind, ModuleCallCx, ModuleDispatcher, ModuleError, ModuleId, ModuleInstallCx,
    ModuleLifecycleCx, NativeViewEmitter, NativeViewFactory, NativeViewInstance, OctaraModule, RawNativeSurface,
};
use octara::prelude::*;
use octara::runtime::WindowId;
use octara_host_native::{ShellError, ShellHook, run_app_with_hook};
use std::any::Any;
use std::cell::RefCell;
use std::collections::BTreeSet;
use std::rc::Rc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

const OPAQUE: &str = "Octara presentation test (opaque)";
const TRANSPARENT: &str = "Octara presentation test (transparent)";
const PROBE: &str = "test.native-window";
const PROBE_FINISHED: &str = "finished";

fn main() -> Result<(), ShellError> {
    if std::env::var_os("OCTARA_TEST_NATIVE_WINDOW").is_none() {
        return Ok(());
    }
    let transparent = Theme::default().color(ColorToken::Background, Color::TRANSPARENT);
    let probe = Rc::new(RefCell::new(Probe::default()));
    let windows = AppDefinition::new()
        .with_module(ProbeModule(probe.clone()))
        .window(WindowDefinition::new(
            WindowOptions { presentation: WindowPresentation { title: OPAQUE.into(), ..Default::default() }, ..Default::default() },
            |_| probe_view("Opaque"),
        ))
        .window(WindowDefinition::new(
            WindowOptions {
                presentation: WindowPresentation { title: TRANSPARENT.into(), theme: transparent, ..Default::default() },
                ..Default::default()
            },
            |_| probe_view("Transparent"),
        ));
    run_app_with_hook(
        windows,
        TestWindows { started: Instant::now(), observed_hidden: [false; 2], observed_first: [false; 2], commands_sent: false },
    )?;
    probe.borrow().verify();
    println!("Opaque and transparent native windows reached their first GPU-complete frames");
    Ok(())
}

fn probe_view(label: &'static str) -> El {
    el().child(text(label)).child(
        typed_native_view(PROBE, TypedNativeProps::new(label, None)).test_id(PROBE).w(px(16.0)).h(px(16.0)).on_typed_native(
            move |write, event, reply: &&str| {
                assert_eq!(event, PROBE_FINISHED);
                assert_eq!(*reply, label);
                write.close_window(write.window_id().expect("native view event has a window"));
            },
        ),
    )
}

#[derive(Default)]
struct Probe {
    lifecycle: Vec<HostLifecycleEvent>,
    created: BTreeSet<WindowId>,
    placed: BTreeSet<WindowId>,
    dropped: BTreeSet<WindowId>,
}

impl Probe {
    fn verify(&self) {
        assert!(matches!(self.lifecycle.first(), Some(HostLifecycleEvent::AppLaunched { .. })));
        assert_eq!(self.lifecycle.last(), Some(&HostLifecycleEvent::WillTerminate));
        assert_eq!(self.created.len(), 2);
        assert_eq!(self.created, self.placed);
        assert_eq!(self.created, self.dropped);
        for &window in &self.created {
            let attached = HostLifecycleEvent::WindowAttached { window };
            let detached = HostLifecycleEvent::WindowDetached { window };
            let events: Vec<_> = self.lifecycle.iter().filter(|&event| event == &attached || event == &detached).collect();
            assert_eq!(events, [&attached, &detached]);
        }
    }
}

struct ProbeModule(Rc<RefCell<Probe>>);

impl OctaraModule for ProbeModule {
    fn id(&self) -> ModuleId {
        ModuleId(PROBE)
    }

    fn install(&mut self, cx: &mut ModuleInstallCx<'_>) -> Result<(), ModuleError> {
        cx.native_views().register(ProbeFactory(self.0.clone()))
    }

    fn call(&mut self, _: CallTicket, _: Box<dyn Any>, _: &mut ModuleCallCx<'_>) {}

    fn on_lifecycle(&mut self, event: &HostLifecycleEvent, _: &mut ModuleLifecycleCx<'_>) {
        self.0.borrow_mut().lifecycle.push(event.clone());
    }
}

struct ProbeFactory(Rc<RefCell<Probe>>);

impl NativeViewFactory for ProbeFactory {
    fn kind(&self) -> &'static str {
        PROBE
    }

    fn create(
        &mut self,
        surface: HostSurface<'_>,
        _: NativeId,
        slot: NativeSlot,
        props: Box<dyn Any>,
        emitter: NativeViewEmitter,
    ) -> Result<Box<dyn NativeViewInstance>, ModuleError> {
        assert!(matches!(
            (surface.kind(), surface.raw_native()),
            (HostSurfaceKind::MacOs, Some(RawNativeSurface::AppKit { .. }))
                | (HostSurfaceKind::Ios, Some(RawNativeSurface::UiKit { .. }))
                | (HostSurfaceKind::Windows, Some(RawNativeSurface::Win32 { .. }))
                | (HostSurfaceKind::Linux, Some(RawNativeSurface::Wayland { .. } | RawNativeSurface::Xlib { .. }))
        ));
        let window = surface.window().expect("native view has an attached window");
        assert_eq!(slot, NativeSlot::Overlay);
        let label = *props.downcast::<&'static str>().expect("typed props");
        assert!(matches!(label, "Opaque" | "Transparent"));
        self.0.borrow_mut().created.insert(window);
        Ok(Box::new(ProbeView { state: self.0.clone(), window, label, emitter, callback: None }))
    }
}

struct ProbeView {
    state: Rc<RefCell<Probe>>,
    window: WindowId,
    label: &'static str,
    emitter: NativeViewEmitter,
    callback: Option<JoinHandle<()>>,
}

impl NativeViewInstance for ProbeView {
    fn update(&mut self, _: Box<dyn Any>) {}

    fn place(&mut self, _: &NativePlacement) {
        self.state.borrow_mut().placed.insert(self.window);
    }

    fn call(&mut self, _: CallTicket, command: Box<dyn Any>, _: ModuleDispatcher) {
        command.downcast::<()>().expect("typed close command");
        let emitter = self.emitter.clone();
        let label = self.label;
        self.callback = Some(std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(30));
            emitter.emit(PROBE_FINISHED, label, None).expect("send a native view event from the worker");
        }));
    }

    fn snapshot(&mut self, _: NativeSnapshotRequest, _: ModuleDispatcher) {}
}

impl Drop for ProbeView {
    fn drop(&mut self) {
        if let Some(callback) = self.callback.take() {
            callback.join().expect("native view callback finished");
        }
        self.state.borrow_mut().dropped.insert(self.window);
    }
}

struct TestWindows {
    started: Instant,
    observed_hidden: [bool; 2],
    observed_first: [bool; 2],
    commands_sent: bool,
}

impl ShellHook for TestWindows {
    fn tick(&mut self, app: &mut App, presented: &dyn Fn(WindowId) -> Option<FrameReceipt>) -> Result<Option<Duration>, ShellError> {
        if self.commands_sent {
            return Err(ShellError("native view callbacks did not close both windows".into()));
        }
        let mut finished = Vec::new();
        let mut ready = 0;
        for (&id, window) in app.runtime().windows() {
            let title = window.presentation().title.as_ref();
            let index = usize::from(title == TRANSPARENT);
            let receipt = presented(id);
            if let Some((visible, opaque)) = appkit_window(title) {
                assert_eq!(visible, receipt.is_some(), "{title}");
                assert_eq!(opaque, index == 0, "{title}");
                self.observed_hidden[index] |= !visible;
            }
            if let Some(receipt) = receipt {
                if !self.observed_first[index] {
                    assert_eq!(receipt.stamp.frame, 1, "{title}");
                    self.observed_first[index] = true;
                }
                ready += usize::from(receipt.gpu_complete);
            }
            finished.push(id);
        }
        if ready == 2 {
            if cfg!(target_os = "macos") {
                assert_eq!(self.observed_hidden, [true; 2]);
            }
            for id in finished {
                app.update_window(id, |write| assert!(write.send_typed_native_command(PROBE, (), None)))
                    .map_err(|error| ShellError(error.to_string()))?;
            }
            self.commands_sent = true;
            // The callbacks wake the host while this hook waits. Its next tick is a timeout.
            return Ok(Some(Duration::from_secs(15)));
        }
        if self.started.elapsed() > Duration::from_secs(15) {
            return Err(ShellError(format!("only {ready} of two windows presented a first frame")));
        }
        Ok(Some(Duration::from_millis(8)))
    }
}

#[cfg(target_os = "macos")]
fn appkit_window(title: &str) -> Option<(bool, bool)> {
    use objc2::MainThreadMarker;
    use objc2_app_kit::NSApplication;
    NSApplication::sharedApplication(MainThreadMarker::new().expect("main thread"))
        .windows()
        .iter()
        .find(|window| window.title().to_string() == title)
        .map(|window| (window.isVisible(), window.isOpaque()))
}

#[cfg(not(target_os = "macos"))]
fn appkit_window(_: &str) -> Option<(bool, bool)> {
    None
}
