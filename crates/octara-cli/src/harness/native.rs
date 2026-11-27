// Compiled in the generated runner against the app's native host and inspection protocol.

use octara::{
    CaptureSource, FrameReceipt, WindowRuntime,
    prelude::{App, AppDefinition, WindowId},
};
use octara_host_native::{ShellError, ShellHook, run_app_with_hook};
use octara_inspect::{
    AgentRequest, InspectHost, Inspector,
    driver::PaintedCapture,
    protocol::{BuildIdentity, Capture},
    transport::unix::UnixTransport,
};
use std::{process::ExitCode, time::Duration};

pub fn run(startup: fn() -> AppDefinition) -> ExitCode {
    let result = (|| {
        let socket = std::env::var_os("OCTARA_INSPECT").ok_or("OCTARA_INSPECT was not set")?;
        let digest = std::env::var("OCTARA_INSPECT_BUILD")?;
        let bridge = Bridge {
            transport: UnixTransport::bind(&socket)?,
            inspector: Inspector::default(),
            painted: PaintedCapture::default(),
            build: BuildIdentity { digest, pid: std::process::id() },
            #[cfg(feature = "dev-restart")]
            shutdown: false,
        };
        run_app_with_hook(startup(), bridge)?;
        Ok::<(), Box<dyn std::error::Error>>(())
    })();
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::from(2)
        }
    }
}

struct Bridge {
    transport: UnixTransport,
    inspector: Inspector,
    painted: PaintedCapture,
    build: BuildIdentity,
    #[cfg(feature = "dev-restart")]
    shutdown: bool,
}

impl ShellHook for Bridge {
    fn tick(&mut self, app: &mut App, presented: &dyn Fn(WindowId) -> Option<FrameReceipt>) -> Result<Option<Duration>, ShellError> {
        let mut host = NativeObservation {
            presented,
            painted: &mut self.painted,
            #[cfg(feature = "dev-restart")]
            shutdown: &mut self.shutdown,
        };
        let inspector = &mut self.inspector;
        let build = &self.build;
        self.transport
            .pump(&mut |request: AgentRequest| {
                let mut receipt = inspector.execute(app, &mut host, request);
                receipt.build = Some(build.clone());
                receipt
            })
            .map_err(|error| ShellError(error.to_string()))?;
        Ok(Some(Duration::from_millis(20)))
    }

    #[cfg(feature = "dev-restart")]
    fn shutdown_requested(&self) -> bool {
        self.shutdown && !self.transport.connections().any(|(_, writing)| writing)
    }
}

struct NativeObservation<'a> {
    presented: &'a dyn Fn(WindowId) -> Option<FrameReceipt>,
    painted: &'a mut PaintedCapture,
    #[cfg(feature = "dev-restart")]
    shutdown: &'a mut bool,
}

impl InspectHost for NativeObservation<'_> {
    #[cfg(feature = "dev-restart")]
    fn supports_restart_state(&self) -> bool {
        true
    }

    #[cfg(feature = "dev-restart")]
    fn request_shutdown(&mut self) -> bool {
        *self.shutdown = true;
        true
    }

    fn presented_frame(&self, window: WindowId) -> Option<u64> {
        (self.presented)(window).map(|frame| frame.stamp.native_rev)
    }

    fn capture(&mut self, window: &WindowRuntime, source: CaptureSource, now: Duration) -> Result<Capture, String> {
        self.painted.capture(window, source, now)
    }
}
