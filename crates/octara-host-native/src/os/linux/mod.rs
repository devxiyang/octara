mod accessibility;
mod compose;
mod mime;
mod preferences;
mod wayland;
mod x11;

use crate::os::OsHost;
use octara::module::HostSurfaceKind;
use std::process::{Command, Stdio};
use url::Url;

pub(super) const SURFACE_KIND: HostSurfaceKind = HostSurfaceKind::Linux;

pub(super) fn create() -> Box<dyn OsHost> {
    if std::env::var_os("WAYLAND_DISPLAY").is_some_and(|display| !display.is_empty()) || std::env::var_os("WAYLAND_SOCKET").is_some() {
        wayland::create()
    } else {
        x11::create()
    }
}

fn open_url(url: &str) {
    let Some(url) = launchable_url(url) else { return };
    // Start the process inside the worker so it always has someone to wait for it.
    let _ = std::thread::Builder::new().name("octara-xdg-open".into()).spawn(move || {
        let _ = Command::new("xdg-open").arg(url.as_str()).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).status();
    });
}

fn launchable_url(url: &str) -> Option<Url> {
    if url.chars().any(char::is_control) {
        return None;
    }
    Url::parse(url).ok().filter(|url| matches!(url.scheme(), "http" | "https" | "mailto" | "file"))
}

#[cfg(test)]
mod tests {
    use super::launchable_url;

    #[test]
    fn xdg_open_accepts_only_allowed_absolute_urls() {
        for (input, url) in [
            ("http://example.com/", "http://example.com/"),
            ("HTTPS://example.com/", "https://example.com/"),
            ("mailto:hello@example.com", "mailto:hello@example.com"),
            ("file:///tmp/with a space", "file:///tmp/with%20a%20space"),
        ] {
            assert_eq!(launchable_url(input).unwrap().as_str(), url);
        }
        for input in ["--help", "/tmp/file", "https://", "https://\nexample.com/", "ftp://example.com/", "javascript:alert(1)"] {
            assert!(launchable_url(input).is_none(), "{input:?}");
        }
    }
}
