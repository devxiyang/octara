//! Ownership transfer and presentation of a Core Animation drawable.

use crate::{DrawableTarget, FrameStageReport, FrameStageTimings};
use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::{NSObjectProtocol, ProtocolObject};
use objc2::{MainThreadMarker, sel};
use objc2_metal::{MTLCommandBuffer, MTLCommandBufferStatus, MTLDrawable};
use objc2_quartz_core::{CACurrentMediaTime, CAMetalDrawable};
use octara_render::OutputColor;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::ptr::NonNull;
use std::sync::{Arc, Mutex};
use std::time::Instant;

type Command = ProtocolObject<dyn MTLCommandBuffer>;
type Drawable = ProtocolObject<dyn CAMetalDrawable>;

pub struct MetalDrawableTarget {
    state: DrawableState,
    transactional: bool,
    pub(super) color: OutputColor,
    stages: Option<StageRequest>,
}

enum DrawableState {
    Acquired(Retained<Drawable>),
    Transaction(Retained<Drawable>, Retained<Command>),
    Submitted,
}

// SAFETY: Only the owner uses the drawable and its command buffer. Metal's queue and
// handlers have their own retained ownership. A transactional present is main-thread only.
unsafe impl Send for MetalDrawableTarget {}

impl MetalDrawableTarget {
    pub fn new(drawable: Retained<Drawable>, color: OutputColor, transactional: bool) -> Self {
        Self { state: DrawableState::Acquired(drawable), transactional, color, stages: None }
    }

    pub(super) fn drawable(&self) -> Option<&Drawable> {
        match &self.state {
            DrawableState::Acquired(drawable) => Some(drawable),
            _ => None,
        }
    }

    /// Record presentation on the rendering buffer, or transfer it to the host transaction.
    pub(super) fn record_presentation(&mut self, command: &Retained<Command>) -> Option<Arc<StageProbe>> {
        let DrawableState::Acquired(drawable) = std::mem::replace(&mut self.state, DrawableState::Submitted) else {
            unreachable!("only an acquired drawable can be submitted");
        };
        let stages = self.stages.take().map(|request| StageProbe::attach(request, &drawable));
        if self.transactional {
            self.state = DrawableState::Transaction(drawable, command.clone());
        } else {
            command.presentDrawable(ProtocolObject::from_ref(&*drawable));
        }
        stages
    }
}

impl DrawableTarget for MetalDrawableTarget {
    fn present(self: Box<Self>) -> Result<(), String> {
        match self.state {
            DrawableState::Acquired(_) => Err("the Metal drawable was not rendered".into()),
            DrawableState::Submitted => Ok(()),
            DrawableState::Transaction(drawable, command) => {
                let _main = MainThreadMarker::new().ok_or("transactional Metal presentation requires the main thread")?;
                command.waitUntilScheduled();
                if command.status() == MTLCommandBufferStatus::Error {
                    return Err(command.error().map_or_else(|| "Metal could not schedule the frame".into(), |error| error.to_string()));
                }
                drawable.present();
                Ok(())
            }
        }
    }

    fn report_stages(&mut self, tick_at: Option<Instant>, cpu_done_at: Instant, report: FrameStageReport) {
        self.stages = Some(StageRequest { tick_at, cpu_done_at, report });
    }
}

struct StageRequest {
    tick_at: Option<Instant>,
    cpu_done_at: Instant,
    report: FrameStageReport,
}

pub(super) struct StageProbe {
    cpu: StageCpuTimes,
    state: Mutex<StageState>,
}

struct StageCpuTimes {
    tick_at: Option<Instant>,
    cpu_done_at: Instant,
    encode_done_at: Instant,
    clock_at: Instant,
    clock_media_s: f64,
}

struct StageState {
    report: Option<FrameStageReport>,
    gpu: Option<(f64, f64)>,
    presented: Option<f64>,
}

impl StageProbe {
    fn attach(request: StageRequest, drawable: &Drawable) -> Arc<Self> {
        let encode_done_at = Instant::now();
        let clock_media_s = CACurrentMediaTime();
        let clock_at = encode_done_at + encode_done_at.elapsed() / 2;
        let cpu = StageCpuTimes { tick_at: request.tick_at, cpu_done_at: request.cpu_done_at, encode_done_at, clock_at, clock_media_s };
        let responds = drawable.respondsToSelector(sel!(addPresentedHandler:));
        let probe = Arc::new(Self {
            cpu,
            state: Mutex::new(StageState { report: Some(request.report), gpu: None, presented: (!responds).then_some(0.0) }),
        });
        if !responds {
            return probe;
        }
        let on_presented = probe.clone();
        let callback = RcBlock::new(move |drawable: NonNull<ProtocolObject<dyn MTLDrawable>>| {
            // SAFETY: Metal keeps its drawable alive through the presented callback.
            let drawable = unsafe { drawable.as_ref() };
            let time = if drawable.respondsToSelector(sel!(presentedTime)) { drawable.presentedTime() } else { 0.0 };
            on_presented.record(|state| state.presented = Some(time));
        });
        // SAFETY: Metal copies the block. It captures only thread-safe timing state.
        unsafe { drawable.addPresentedHandler(RcBlock::as_ptr(&callback)) };
        probe
    }

    pub(super) fn completed(&self, gpu_start_s: f64, gpu_end_s: f64) {
        self.record(|state| state.gpu = Some((gpu_start_s, gpu_end_s)));
    }

    fn record(&self, update: impl FnOnce(&mut StageState)) {
        let mut state = self.state.lock().expect("Metal frame stage timings");
        update(&mut state);
        if let (Some((gpu_start_s, gpu_end_s)), Some(presented_at_s)) = (state.gpu, state.presented)
            && let Some(report) = state.report.take()
        {
            drop(state);
            let times = FrameStageTimings {
                tick_at: self.cpu.tick_at,
                cpu_done_at: self.cpu.cpu_done_at,
                encode_done_at: self.cpu.encode_done_at,
                clock_at: self.cpu.clock_at,
                clock_media_s: self.cpu.clock_media_s,
                gpu_start_s,
                gpu_end_s,
                presented_at_s: (presented_at_s.is_finite() && presented_at_s > 0.0).then_some(presented_at_s),
            };
            let _ = catch_unwind(AssertUnwindSafe(|| report(times)));
        }
    }
}
