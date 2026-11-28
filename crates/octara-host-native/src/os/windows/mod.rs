use crate::os::{HostWake, OsHost, OsWindow, OsWindowContext, ShellDriver};
use octara::{WindowId, WindowOptions, module::HostSurfaceKind};
use std::sync::Arc;

pub(super) const SURFACE_KIND: HostSurfaceKind = HostSurfaceKind::Windows;

pub(super) fn create() -> Box<dyn OsHost> {
    Box::new(WindowsHost)
}

const UNAVAILABLE: &str = if cfg!(target_os = "windows") {
    "The direct Win32 host is scheduled for Octara 0.2.0"
} else {
    "Octara has no native window host for this platform"
};

struct WindowsHost;

impl OsHost for WindowsHost {
    fn run(self: Box<Self>, _: Box<dyn ShellDriver>) -> Result<(), String> {
        Err(UNAVAILABLE.into())
    }

    fn wake(&self) -> HostWake {
        Arc::new(|| {})
    }

    fn create_window(&mut self, _: WindowId, _: &WindowOptions, _: OsWindowContext<'_>) -> Result<Box<dyn OsWindow>, String> {
        Err(UNAVAILABLE.into())
    }
}
