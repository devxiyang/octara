//! Read process settings once, before entering the event loop.

use std::ffi::OsString;
use std::path::PathBuf;
use std::time::Duration;

#[derive(Clone, Debug)]
pub struct LaunchCapture {
    pub path: PathBuf,
    pub after: Duration,
}

#[derive(Clone, Debug)]
pub struct ShellConfig {
    pub exit_after_frames: Option<u64>,
    pub capture: Option<LaunchCapture>,
    pub restart_state: Option<PathBuf>,
    pub manual_clock: bool,
    pub trace: bool,
    pub trace_gpu: bool,
    pub trace_native_input: bool,
    pub trace_scroll: bool,
    pub trace_frame_clock: bool,
    pub trace_presented: bool,
    pub metal_frame_latency: u32,
    pub metal_acquire_delay: Duration,
}

impl Default for ShellConfig {
    fn default() -> Self {
        Self::read(|_| None)
    }
}

impl ShellConfig {
    pub fn from_env() -> Self {
        Self::read(|name| std::env::var_os(name))
    }

    fn read(get: impl Fn(&str) -> Option<OsString>) -> Self {
        let enabled = |name| get(name).is_some_and(|value| value != "0");
        let duration = |name, fallback| Duration::from_millis(get(name).and_then(|value| value.to_str()?.parse().ok()).unwrap_or(fallback));
        let capture =
            get("OCTARA_CAPTURE").map(|path| LaunchCapture { path: path.into(), after: duration("OCTARA_CAPTURE_AFTER_MS", 300) });
        let trace_gpu = enabled("OCTARA_TRACE_GPU");
        Self {
            exit_after_frames: get("OCTARA_EXIT_AFTER_FRAMES").and_then(|value| value.to_str()?.parse().ok()).filter(|&frames| frames > 0),
            capture,
            restart_state: get("OCTARA_RESTART_STATE").map(Into::into),
            manual_clock: get("OCTARA_INSPECT_CLOCK").is_some_and(|value| value == "manual"),
            trace: trace_gpu || enabled("OCTARA_TRACE_INPUT"),
            trace_gpu,
            trace_native_input: enabled("OCTARA_TRACE_NATIVE_INPUT"),
            trace_scroll: enabled("OCTARA_TRACE_SCROLL"),
            trace_frame_clock: enabled("OCTARA_TRACE_FRAME_CLOCK"),
            trace_presented: enabled("OCTARA_TRACE_PRESENTED"),
            metal_frame_latency: if get("OCTARA_METAL_FRAME_LATENCY").is_some_and(|value| value == "1") { 1 } else { 2 },
            metal_acquire_delay: duration("OCTARA_METAL_ACQUIRE_DELAY_MS", 0).min(Duration::from_secs(1)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capture_and_frame_limit_are_parsed_independently() {
        let config = ShellConfig::read(|name| match name {
            "OCTARA_CAPTURE" => Some("capture.png".into()),
            "OCTARA_CAPTURE_AFTER_MS" => Some("12".into()),
            "OCTARA_EXIT_AFTER_FRAMES" => Some("3".into()),
            "OCTARA_TRACE_GPU" => Some("1".into()),
            _ => None,
        });
        assert_eq!(config.exit_after_frames, Some(3));
        let capture = config.capture.unwrap();
        assert_eq!(capture.path, PathBuf::from("capture.png"));
        assert_eq!(capture.after, Duration::from_millis(12));
        assert!(config.trace && config.trace_gpu);
    }
}
