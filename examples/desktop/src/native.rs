//! Optional local inspection for the example binaries, including iOS fixtures.

use octara::prelude::*;
use octara_host_native::ShellError;

pub fn run<V: IntoView>(options: WindowOptions, root: impl FnOnce(&mut BuildCx<'_>) -> V + 'static) -> Result<(), ShellError> {
    run_app(AppDefinition::new().window(WindowDefinition::new(options, root)))
}

pub fn run_app(app: AppDefinition) -> Result<(), ShellError> {
    #[cfg(unix)]
    if let Some(path) = std::env::var_os("OCTARA_INSPECT") {
        return unix::run(app, &path);
    }
    octara_host_native::run_app(app)
}

#[cfg(unix)]
mod unix {
    use super::*;
    use octara::{CaptureSource, FrameReceipt, WindowRuntime};
    use octara_host_native::{ShellHook, run_app_with_hook};
    use octara_inspect::{
        AgentRequest, InspectHost, Inspector,
        driver::PaintedCapture,
        protocol::{BuildIdentity, Capture},
        transport::unix::UnixTransport,
    };
    use std::time::Duration;

    pub(super) fn run(app: AppDefinition, path: &std::ffi::OsStr) -> Result<(), ShellError> {
        let transport = UnixTransport::bind(path).map_err(|error| ShellError(error.to_string()))?;
        let build = std::env::var("OCTARA_INSPECT_BUILD").ok().map(|digest| BuildIdentity { digest, pid: std::process::id() });
        run_app_with_hook(app, Bridge { transport, build, inspector: Inspector::default(), paint: PaintedCapture::default() })
    }

    struct Bridge {
        transport: UnixTransport,
        build: Option<BuildIdentity>,
        inspector: Inspector,
        paint: PaintedCapture,
    }

    impl ShellHook for Bridge {
        fn tick(&mut self, app: &mut App, presented: &dyn Fn(WindowId) -> Option<FrameReceipt>) -> Result<Option<Duration>, ShellError> {
            let mut host = Observation { presented, paint: &mut self.paint };
            self.transport
                .pump(&mut |request: AgentRequest| {
                    let mut receipt = self.inspector.execute(app, &mut host, request);
                    receipt.build.clone_from(&self.build);
                    receipt
                })
                .map_err(|error| ShellError(error.to_string()))?;
            Ok(Some(Duration::from_millis(20)))
        }
    }

    struct Observation<'a> {
        presented: &'a dyn Fn(WindowId) -> Option<FrameReceipt>,
        paint: &'a mut PaintedCapture,
    }

    impl InspectHost for Observation<'_> {
        fn presented_frame(&self, window: WindowId) -> Option<u64> {
            (self.presented)(window).map(|frame| frame.stamp.frame)
        }

        fn capture(&mut self, window: &WindowRuntime, source: CaptureSource, now: Duration) -> Result<Capture, String> {
            self.paint.capture(window, source, now)
        }
    }
}
