//! An Inspector size request goes through the native window, then waits for the host and
//! framework to agree about the size the window system actually applied.

use super::{AppWindow, Shell, trace, ww};
use octara::app::WindowId;
use octara::diagnostics as codes;
use octara::inspect::Reply;
use octara::serde_json::{Value, json};
use std::time::{Duration, Instant};
use winit::dpi::{LogicalSize, PhysicalSize};

const LIMIT: Duration = Duration::from_secs(5);
const MAX_PIXELS: u64 = 32 * 1024 * 1024;

pub(crate) struct Job {
    window: WindowId,
    requested: [f32; 2],
    until: Instant,
    /// A synchronous return is the window system's final answer, including a refusal.
    immediate: bool,
}

pub(crate) struct Completion {
    pub(crate) reply: Reply,
    pub(crate) resize: Value,
}

impl Job {
    pub(crate) fn start(
        shell: &mut Shell,
        target: &mut AppWindow,
        width: f32,
        height: f32,
        timing: Option<trace::ResizeRequest>,
    ) -> Result<Self, Box<Reply>> {
        if cfg!(target_os = "ios") {
            return Err(Box::new(Reply::fail(&codes::BAD_REQUEST, "the iOS scene owns the window size")));
        }
        let scale = ww(target, |w| w.window.scale_factor())
            .ok_or_else(|| Box::new(Reply::fail(&codes::BAD_REQUEST, "the target has no native window")))?;
        let dimension_limit = shell
            .shared
            .gpu
            .borrow()
            .as_ref()
            .map(|g| g.device().limits().max_texture_dimension_2d)
            .ok_or_else(|| Box::new(Reply::fail(&codes::BAD_REQUEST, "the window has no GPU renderer")))?;
        let expected =
            validate_size([width, height], scale, dimension_limit).map_err(|why| Box::new(Reply::fail(&codes::BAD_REQUEST, why)))?;
        let logical = logical_size([width, height]);
        let applied = ww(target, |host| {
            let request = timing.map(|timing| (Instant::now(), timing));
            let applied = host.window.request_inner_size(logical);
            if let Some((started, timing)) = request {
                let returned = Instant::now();
                let epoch = shell.shared.trace_epoch;
                let window = host.window.id();
                trace::resize(epoch, window, expected, started, trace::ResizePhase::Request(timing));
                trace::resize(epoch, window, expected, returned, trace::ResizePhase::RequestReturn);
            }
            applied
        })
        .ok_or_else(|| Box::new(Reply::fail(&codes::BAD_REQUEST, "the target has no native window")))?;
        // Some platforms do not emit Resized when the request completes synchronously.
        // Reuse precisely the handler that updates both the GPU surface and framework input.
        if let Some(size) = applied {
            if let Err(why) = validate_physical(size, dimension_limit) {
                // The native request has already been issued, so this is an applied-size
                // failure, not a validation refusal before delivery.
                let mut reply = Reply::fail(&codes::STEP_FAILED, format!("the native window returned an unsupported size: {why}"));
                reply.actual = Some(json!({ "requested": [width, height], "physical": [size.width, size.height], "scale": scale }));
                return Err(Box::new(reply));
            }
            shell.resize_window(target, size);
        }
        Ok(Self { window: target.id(), requested: [width, height], until: Instant::now() + LIMIT, immediate: applied.is_some() })
    }

    /// No clock advancement or waiting here: the native event loop remains in charge.
    pub(crate) fn poll(&self, shell: &Shell) -> Option<Completion> {
        let Some(window) = shell.app.as_ref().and_then(|app| app.window(self.window)) else {
            return Some(Completion {
                reply: Reply::fail(&codes::STEP_FAILED, "the target window closed while resizing"),
                resize: json!({ "window": self.window.number(), "requested": self.requested, "actual": null, "matched": false }),
            });
        };
        let Some((actual, configured, scale)) = ww(&window, |w| (w.drawable_size(), w.configured_size(), w.window.scale_factor())) else {
            return Some(Completion {
                reply: Reply::fail(&codes::STEP_FAILED, "the target lost its native window while resizing"),
                resize: json!({ "window": self.window.number(), "requested": self.requested, "actual": null, "matched": false }),
            });
        };
        let viewport = window.size();
        let expected = logical_size(self.requested).to_physical::<u32>(scale);
        let matched = actual == expected;
        let synchronized = shell.shared.gpu_available() && configured.is_some_and(|size| synchronized(actual, size, viewport, scale));
        let metadata = json!({
            "window": self.window.number(), "requested": self.requested,
            "actual": [f64::from(actual.width) / scale, f64::from(actual.height) / scale],
            "physical": [actual.width, actual.height], "scale": scale,
            "viewport": [viewport.0, viewport.1], "matched": matched, "synchronized": synchronized,
        });
        if matched && synchronized {
            return Some(Completion { reply: Reply::ok(), resize: metadata });
        }
        if (self.immediate && synchronized) || Instant::now() >= self.until {
            let mut reply = Reply::fail(
                &codes::STEP_FAILED,
                if synchronized {
                    "the window system did not apply the requested size; inspect live.resize for the actual size"
                } else {
                    "the native window and framework viewport did not agree within the resize budget"
                },
            );
            reply.actual = Some(metadata.clone());
            return Some(Completion { reply, resize: metadata });
        }
        None
    }
}

fn logical_size(requested: [f32; 2]) -> LogicalSize<f64> {
    LogicalSize::new(f64::from(requested[0]), f64::from(requested[1]))
}

fn validate_size(requested: [f32; 2], scale: f64, dimension_limit: u32) -> Result<PhysicalSize<u32>, String> {
    let raw = [f64::from(requested[0]) * scale, f64::from(requested[1]) * scale];
    if !scale.is_finite()
        || scale <= 0.0
        || raw.iter().any(|side| !side.is_finite() || side.round() < 1.0 || side.round() > f64::from(dimension_limit))
    {
        return Err(format!("window size must be finite and positive, with physical dimensions between 1 and {dimension_limit}"));
    }
    let physical = logical_size(requested).to_physical::<u32>(scale);
    validate_physical(physical, dimension_limit)?;
    Ok(physical)
}

pub(crate) fn validate_physical(physical: PhysicalSize<u32>, dimension_limit: u32) -> Result<(), String> {
    if physical.width == 0 || physical.height == 0 || physical.width > dimension_limit || physical.height > dimension_limit {
        return Err(format!("physical dimensions must be between 1 and {dimension_limit}"));
    }
    if u64::from(physical.width) * u64::from(physical.height) > MAX_PIXELS {
        return Err("window size must produce at most 32 megapixels".into());
    }
    Ok(())
}

fn synchronized(actual: PhysicalSize<u32>, configured: PhysicalSize<u32>, viewport: (f32, f32), scale: f64) -> bool {
    // Match WindowEvent::Resized's f32 scale/division so physical-pixel rounding is allowed,
    // while a stale Kernel-only resize or GPU configuration can never count as success.
    actual.width > 0
        && actual.height > 0
        && actual == configured
        && scale.is_finite()
        && scale > 0.0
        && (viewport.0 - actual.width as f32 / scale as f32).abs() < 0.01
        && (viewport.1 - actual.height as f32 / scale as f32).abs() < 0.01
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_size_validation_uses_rounded_device_pixels_and_gpu_limits() {
        assert_eq!(validate_size([640.25, 480.25], 2.0, 8192).unwrap(), PhysicalSize::new(1281, 961));
        for size in [[0.0, 20.0], [-1.0, 20.0], [f32::NAN, 20.0], [20.0, f32::INFINITY], [0.1, 20.0], [4097.0, 1.0]] {
            assert!(validate_size(size, 2.0, 8192).is_err(), "{size:?}");
        }
        assert!(validate_size([4096.0, 4096.0], 2.0, 8192).is_err(), "each side fits but the total allocation is too large");
        assert!(validate_size([640.0, 480.0], f64::NAN, 8192).is_err());
    }

    #[test]
    fn success_needs_native_surface_and_framework_sizes_to_agree() {
        let actual = PhysicalSize::new(1281, 961);
        assert!(synchronized(actual, actual, (640.5, 480.5), 2.0));
        assert!(!synchronized(actual, actual, (640.0, 480.0), 2.0), "the requested value alone cannot confirm native resize");
        assert!(!synchronized(actual, PhysicalSize::new(1280, 960), (640.5, 480.5), 2.0), "the GPU surface must catch up too");
        assert!(!synchronized(PhysicalSize::new(0, 0), PhysicalSize::new(1, 1), (0.0, 0.0), 2.0));
    }
}
