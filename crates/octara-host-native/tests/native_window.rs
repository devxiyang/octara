//! Run on a desktop session with `OCTARA_TEST_NATIVE_WINDOW=1`.

use octara::model::capture::FrameReceipt;
use octara::model::theme::{Color, ColorToken, Theme};
use octara::prelude::*;
use octara::runtime::WindowId;
use octara_host_native::{ShellError, ShellHook, run_app_with_hook};
use std::time::{Duration, Instant};

const OPAQUE: &str = "Octara presentation test (opaque)";
const TRANSPARENT: &str = "Octara presentation test (transparent)";

fn main() -> Result<(), ShellError> {
    if std::env::var_os("OCTARA_TEST_NATIVE_WINDOW").is_none() {
        return Ok(());
    }
    let transparent = Theme::default().color(ColorToken::Background, Color::TRANSPARENT);
    let windows = AppDefinition::new()
        .window(WindowDefinition::new(WindowOptions { title: OPAQUE.into(), ..Default::default() }, |_| text("Opaque")))
        .window(WindowDefinition::new(WindowOptions { title: TRANSPARENT.into(), theme: transparent, ..Default::default() }, |_| {
            text("Transparent")
        }));
    run_app_with_hook(windows, TestWindows { started: Instant::now(), observed_hidden: [false; 2], observed_first: [false; 2] })?;
    println!("Opaque and transparent native windows reached their first GPU-complete frames");
    Ok(())
}

struct TestWindows {
    started: Instant,
    observed_hidden: [bool; 2],
    observed_first: [bool; 2],
}

impl ShellHook for TestWindows {
    fn tick(&mut self, app: &mut App, presented: &dyn Fn(WindowId) -> Option<FrameReceipt>) -> Result<Option<Duration>, ShellError> {
        let mut finished = Vec::new();
        let mut ready = 0;
        for (&id, window) in app.runtime().windows() {
            let title = window.options().title.as_str();
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
                app.close_window(id).map_err(|error| ShellError(error.to_string()))?;
            }
            return Ok(None);
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
