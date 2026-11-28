mod accessibility;
mod compose;
mod mime;
mod preferences;
mod wayland;
mod x11;

use crate::os::OsHost;
use octara::module::HostSurfaceKind;

pub(super) const SURFACE_KIND: HostSurfaceKind = HostSurfaceKind::Linux;

pub(super) fn create() -> Box<dyn OsHost> {
    if std::env::var_os("WAYLAND_DISPLAY").is_some_and(|display| !display.is_empty()) || std::env::var_os("WAYLAND_SOCKET").is_some() {
        wayland::create()
    } else {
        x11::create()
    }
}
