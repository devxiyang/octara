//! The shared device and the notification that ends each device generation.

use crate::platform::RenderedWindow;
use crate::render_worker::{RenderWorker, Work};
use crate::{ShellEvent, trace};
use octara::capture::{PaintFrame, PaintOrigin};
use octara::platform::PublishedFrame;
use octara_primitives::{
    capture::{PaintSample, PaintedPicture, Painter},
    paint::DisplayList,
};
use octara_render::gpu::{FrameInput, FrameProfile, GpuTimings, PendingPresentation};
use octara_render::{GpuRenderer, wgpu};
use std::any::Any;
use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};
use winit::event_loop::EventLoopProxy;

const TRACE_FEATURES: wgpu::Features = wgpu::Features::TIMESTAMP_QUERY.union(wgpu::Features::TIMESTAMP_QUERY_INSIDE_ENCODERS);

pub(crate) struct Gpu {
    pub(crate) instance: wgpu::Instance,
    pub(crate) adapter: wgpu::Adapter,
    renderer: Option<GpuRenderer>,
    worker: Option<RenderWorker<RenderCompletion>>,
    proxy: EventLoopProxy<ShellEvent>,
    threaded_render: bool,
    device: wgpu::Device,
    queue: wgpu::Queue,
    renderer_name: &'static str,
    pub(crate) format: wgpu::TextureFormat,
    pub(crate) generation: Arc<DeviceGeneration>,
    descriptor: wgpu::DeviceDescriptor<'static>,
    /// A replacement must accept work before another recovery attempt.
    awaiting_work: bool,
    #[cfg(debug_assertions)]
    destroy: Option<TestDestroy>,
}

#[cfg(debug_assertions)]
#[derive(Clone, Copy)]
enum TestDestroy {
    Requested,
    AwaitingLoss(Instant),
}

pub(crate) enum RenderCompletion {
    Window(Box<RenderedWindow>),
    Capture,
    SurfaceDrain(Result<bool, String>),
}

pub(crate) struct FrameSubmission {
    pub(crate) paint: PaintFrame,
    pub(crate) profile: Option<(FrameProfile, Duration)>,
    pub(crate) presentation: PendingPresentation,
}

pub(crate) struct DeviceGeneration {
    pub(crate) number: u64,
    loss: OnceLock<(wgpu::DeviceLostReason, String)>,
}

impl DeviceGeneration {
    /// Only the current device's callback can initiate recovery.
    pub(crate) fn loss_for(&self, number: u64) -> Option<&(wgpu::DeviceLostReason, String)> {
        self.loss.get().filter(|_| self.number == number)
    }

    pub(crate) fn is_lost(&self) -> bool {
        self.loss.get().is_some()
    }

    /// wgpu 30 can unwind when polling a lost device or using its invalidated handles.
    /// Require confirmation from this device before discarding those operations.
    pub(crate) fn run<T>(&self, operation: impl FnOnce() -> T) -> Option<T> {
        if self.is_lost() {
            return None;
        }
        match catch_unwind(AssertUnwindSafe(operation)) {
            Ok(value) => (!self.is_lost()).then_some(value),
            Err(payload) if self.is_lost() && is_wgpu_loss_panic(payload.as_ref()) => None,
            Err(payload) => resume_unwind(payload),
        }
    }
}

fn is_wgpu_loss_panic(payload: &(dyn Any + Send)) -> bool {
    let Some(message) = payload.downcast_ref::<String>() else { return false };
    if message.starts_with("Error in Device::poll:") {
        return true;
    }
    let error = message.strip_prefix("wgpu error:").or_else(|| message.strip_prefix("Octara frame submission failed:"));
    error.is_some_and(|error| error.trim() == "Out of Memory" || error.lines().any(|line| line.trim_end().ends_with(" is invalid")))
}

impl Gpu {
    pub(crate) fn new(
        instance: wgpu::Instance,
        surface: &wgpu::Surface<'static>,
        trace_gpu: bool,
        proxy: &EventLoopProxy<ShellEvent>,
    ) -> Result<Self, String> {
        let adapter = request_adapter(&instance, surface)?;
        let mut descriptor = wgpu::DeviceDescriptor::default();
        if trace_gpu {
            descriptor.required_features = TRACE_FEATURES;
        }
        let caps = surface.get_capabilities(&adapter);
        // Colors are blended in sRGB-encoded space: present a non-sRGB format.
        let format = caps
            .formats
            .iter()
            .copied()
            .find(|f| matches!(f, wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Rgba8Unorm))
            .or_else(|| caps.formats.first().copied())
            .ok_or("no GPU surface format for the window")?;
        let (renderer, generation) = request_renderer(&adapter, &descriptor, format, 0, proxy)?;
        let worker = render_worker(proxy, generation.number)?;
        let (device, queue, renderer_name) = (renderer.device().clone(), renderer.queue().clone(), renderer.renderer());
        Ok(Self {
            instance,
            adapter,
            renderer: Some(renderer),
            worker: Some(worker),
            proxy: proxy.clone(),
            threaded_render: std::env::var_os("OCTARA_RENDER_WORKER").is_some_and(|value| value == "1"),
            device,
            queue,
            renderer_name,
            format,
            generation,
            descriptor,
            awaiting_work: false,
            #[cfg(debug_assertions)]
            destroy: None,
        })
    }

    pub(crate) fn available(&self) -> bool {
        #[cfg(debug_assertions)]
        if self.destroy.is_some() {
            return false;
        }
        !self.generation.is_lost()
    }

    pub(crate) fn replacement(&self, surface: &wgpu::Surface<'static>, proxy: &EventLoopProxy<ShellEvent>) -> Result<Self, String> {
        if self.awaiting_work {
            return Err("the replacement device was lost before it accepted any drawing".into());
        }
        let number = self.generation.number.checked_add(1).expect("device generation exhausted");
        let (renderer, generation, adapter) = match request_renderer(&self.adapter, &self.descriptor, self.format, number, proxy) {
            Ok((renderer, generation)) => (renderer, generation, self.adapter.clone()),
            Err(previous) => {
                let adapter = request_adapter(&self.instance, surface)
                    .map_err(|error| format!("the previous adapter failed ({previous}) and a fresh adapter was unavailable ({error})"))?;
                if !surface.get_capabilities(&adapter).formats.contains(&self.format) {
                    return Err(format!("the previous adapter failed ({previous}) and the new adapter lacks {:?}", self.format));
                }
                let (renderer, generation) = request_renderer(&adapter, &self.descriptor, self.format, number, proxy)
                    .map_err(|error| format!("the previous adapter failed ({previous}) and the new adapter failed ({error})"))?;
                (renderer, generation, adapter)
            }
        };
        let worker = render_worker(proxy, number)?;
        let (device, queue, renderer_name) = (renderer.device().clone(), renderer.queue().clone(), renderer.renderer());
        Ok(Self {
            instance: self.instance.clone(),
            adapter,
            renderer: Some(renderer),
            worker: Some(worker),
            proxy: proxy.clone(),
            threaded_render: self.threaded_render,
            device,
            queue,
            renderer_name,
            format: self.format,
            generation,
            descriptor: self.descriptor.clone(),
            awaiting_work: true,
            #[cfg(debug_assertions)]
            destroy: None,
        })
    }

    pub(crate) fn submitted(&mut self) {
        self.awaiting_work = false;
    }

    pub(crate) fn accept_presentation(&mut self, pending: PendingPresentation) {
        self.renderer.as_mut().expect("the host owns the renderer when accepting a frame").accept_presentation(pending);
        self.submitted();
    }

    pub(crate) fn device(&self) -> &wgpu::Device {
        &self.device
    }

    #[cfg(target_os = "macos")]
    pub(crate) fn queue(&self) -> &wgpu::Queue {
        &self.queue
    }

    pub(crate) fn renderer_name(&self) -> &'static str {
        self.renderer_name
    }

    pub(crate) fn idle(&self) -> bool {
        self.renderer.is_some()
    }

    pub(crate) fn threaded(&self) -> bool {
        self.threaded_render && self.worker.is_some()
    }

    /// Hands the only renderer to the worker. This admits one job and the caller can keep
    /// routing input or surface events until the owner returns.
    pub(crate) fn start_work(&mut self, work: Work<RenderCompletion>) -> bool {
        let (Some(worker), Some(renderer)) = (&self.worker, self.renderer.take()) else {
            unreachable!("admit work only while the renderer is idle")
        };
        match worker.try_start(renderer, work) {
            Ok(()) => true,
            Err(renderer) => {
                self.renderer = Some(renderer);
                self.worker = None;
                self.threaded_render = false;
                eprintln!("octara-shell: the render worker disconnected before admission");
                false
            }
        }
    }

    pub(crate) fn finish_work(&mut self) -> Result<Option<RenderCompletion>, String> {
        if self.idle() {
            return Ok(None);
        }
        let worker = self.worker.as_ref().expect("the worker owns the renderer");
        let Some((renderer, result)) = worker.try_finish().map_err(|error| error.to_string())? else { return Ok(None) };
        self.renderer = Some(renderer);
        Ok(Some(match result {
            Ok(output) => output,
            Err(payload) => resume_unwind(payload),
        }))
    }

    pub(crate) fn cancel_work(&mut self) {
        self.worker = None;
    }

    /// Record and submit as one operation on the renderer's owner. Staged uploads, custom
    /// draws and resource history all follow this submission. The host is responsible for
    /// presenting the target and publishing its receipt.
    pub(crate) fn submit_frame(
        &mut self,
        published: &PublishedFrame,
        target: &wgpu::Texture,
        frame: u32,
        surface_generation: u64,
        report: Option<Box<dyn FnOnce(GpuTimings) + Send>>,
    ) -> Option<FrameSubmission> {
        if !self.available() {
            return None;
        }
        submit_frame(self.renderer.as_mut()?, &self.generation, published, target, frame, surface_generation, report)
    }

    pub(crate) fn present(&self, frame: wgpu::SurfaceTexture) {
        self.queue.present(frame);
    }

    /// Reserve the renderer while the worker waits for the shared device's submissions.
    pub(crate) fn start_surface_drain(&mut self, trace_epoch: Option<Instant>) -> Result<(), String> {
        if self.worker.is_none() {
            self.worker = Some(render_worker(&self.proxy, self.generation.number)?);
        }
        let generation = self.generation.clone();
        let work: Work<RenderCompletion> = Box::new(move |renderer| {
            if let Some(epoch) = trace_epoch {
                trace::surface_drain(epoch, generation.number, Instant::now(), "start", None);
            }
            #[cfg(debug_assertions)]
            if let Some(milliseconds) = std::env::var("OCTARA_SURFACE_DRAIN_DELAY_MS").ok().and_then(|value| value.parse::<u64>().ok())
                && milliseconds != 0
                && !generation.is_lost()
            {
                std::thread::sleep(Duration::from_millis(milliseconds.min(1000)));
            }
            let wait = wgpu::PollType::Wait { submission_index: None, timeout: Some(Duration::from_millis(50)) };
            let ready = match generation.run(|| renderer.device().poll(wait)) {
                Some(Ok(status)) => Ok(status.is_queue_empty()),
                None | Some(Err(wgpu::PollError::Timeout)) => Ok(false),
                Some(Err(error)) => Err(format!("cannot drain the GPU for surface configuration: {error}")),
            };
            if let Some(epoch) = trace_epoch {
                trace::surface_drain(epoch, generation.number, Instant::now(), "finish", Some(matches!(ready, Ok(true))));
            }
            RenderCompletion::SurfaceDrain(ready)
        });
        let enqueued = trace_epoch.map(|epoch| (epoch, Instant::now()));
        if !self.start_work(work) {
            return Err("the render worker did not accept the surface drain".into());
        }
        if let Some((epoch, at)) = enqueued {
            trace::surface_drain(epoch, self.generation.number, at, "enqueue", None);
        }
        Ok(())
    }

    /// Apply on the window thread after a successful drain, before admitting another job.
    pub(crate) fn apply_surface_configuration(&self, surface: &wgpu::Surface<'static>, config: &wgpu::SurfaceConfiguration) -> bool {
        self.idle()
            && self.available()
            && self
                .generation
                .run(|| {
                    surface.configure(self.device(), config);
                    // Surface::configure has no result. Give wgpu a chance to deliver its callback.
                    self.instance.poll_all(false);
                })
                .is_some()
    }

    pub(crate) fn start_capture(
        &mut self,
        list: Arc<DisplayList>,
        scale: f32,
        sample: PaintSample,
        active: Arc<AtomicBool>,
        proxy: EventLoopProxy<ShellEvent>,
    ) -> Result<Receiver<Result<PaintedPicture, String>>, String> {
        let (reply, pixels) = channel();
        let waking = active.clone();
        // Wake after the final sender is dropped, including when readback unwinds.
        let reply = Arc::new(std::mem::DropGuard::new(reply, move |sender| {
            drop(sender);
            if waking.load(Ordering::Relaxed) {
                let _ = proxy.send_event(ShellEvent::Capture);
            }
        }));
        let work = prepare_readback(list, scale, sample, active, self.generation.clone(), move |result| {
            let _ = reply.send(result);
        });
        if self.threaded() {
            if !self.start_work(work) {
                return Err("the render worker did not accept the capture".into());
            }
        } else {
            work(self.renderer.as_mut().expect("the renderer is idle"));
        }
        Ok(pixels)
    }

    pub(crate) fn poll(&mut self, profile: bool) -> Result<bool, String> {
        let Some(renderer) = self.renderer.as_mut() else { return Ok(false) };
        if self.generation.is_lost() {
            return Ok(false);
        }
        #[cfg(debug_assertions)]
        if let Some(destroy) = self.destroy {
            let deadline = match destroy {
                TestDestroy::Requested => {
                    let deadline = Instant::now() + Duration::from_secs(2);
                    self.destroy = Some(TestDestroy::AwaitingLoss(deadline));
                    self.device.destroy();
                    deadline
                }
                TestDestroy::AwaitingLoss(deadline) => deadline,
            };
            self.instance.poll_all(false);
            if self.generation.is_lost() {
                return Ok(false);
            }
            if Instant::now() >= deadline {
                return Err("wgpu did not report the destroyed device".into());
            }
            return Ok(true);
        }
        Ok(profile && self.generation.run(|| renderer.poll_profile()).unwrap_or(false))
    }

    #[cfg(debug_assertions)]
    pub(crate) fn destroy_for_test(&mut self) {
        self.destroy.get_or_insert(TestDestroy::Requested);
    }
}

fn prepare_readback(
    list: Arc<DisplayList>,
    scale: f32,
    sample: PaintSample,
    active: Arc<AtomicBool>,
    generation: Arc<DeviceGeneration>,
    reply: impl Fn(Result<PaintedPicture, String>) + Clone + Send + 'static,
) -> Work<RenderCompletion> {
    Box::new(move |renderer| {
        let result = generation
            .run(|| renderer.start_render_with_receipts(&list, scale, Some(sample)))
            .unwrap_or_else(|| Err("the GPU device was lost while starting the capture".into()));
        match result {
            Ok(mut readback) => {
                let device = renderer.device().clone();
                let complete = reply.clone();
                if let Err(error) = std::thread::Builder::new().name("octara-readback".into()).spawn(move || {
                    while active.load(Ordering::Relaxed) {
                        if generation.is_lost() {
                            complete(Err("the GPU device was lost during readback".into()));
                            return;
                        }
                        if let Some(result) = readback.try_finish() {
                            complete(result);
                            return;
                        }
                        match generation.run(|| device.poll(wgpu::PollType::Poll)) {
                            Some(Ok(_)) => {}
                            Some(Err(error)) => {
                                complete(Err(error.to_string()));
                                return;
                            }
                            None => continue,
                        }
                        std::thread::sleep(Duration::from_millis(1));
                    }
                }) {
                    reply(Err(error.to_string()));
                }
            }
            Err(error) => reply(Err(error)),
        }
        RenderCompletion::Capture
    })
}

pub(crate) fn submit_frame(
    renderer: &mut GpuRenderer,
    generation: &DeviceGeneration,
    published: &PublishedFrame,
    target: &wgpu::Texture,
    frame: u32,
    surface_generation: u64,
    report: Option<Box<dyn FnOnce(GpuTimings) + Send>>,
) -> Option<FrameSubmission> {
    generation
        .run(|| {
            let view = target.create_view(&Default::default());
            let encoder = renderer.device().create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("Octara window frame") });
            if generation.is_lost() {
                return None;
            }
            let input = FrameInput {
                list: &published.list,
                scale: published.scale,
                size: (target.width(), target.height()),
                sample: Some(published.paint_sample()),
            };
            let (recorded, profile) = renderer.record_sampled(encoder, &view, input, report);
            if generation.is_lost() {
                return None;
            }
            let origin = PaintOrigin::Submitted { frame, surface_generation, device_generation: generation.number };
            let paint = published.paint_frame(origin, recorded.paint_receipts());
            let started = profile.as_ref().map(|_| Instant::now());
            let (_, presentation) = recorded.submit_for_presentation();
            let profile = profile.zip(started).map(|(profile, started)| (profile, started.elapsed()));
            Some(FrameSubmission { paint, profile, presentation })
        })
        .flatten()
}

fn render_worker(proxy: &EventLoopProxy<ShellEvent>, generation: u64) -> Result<RenderWorker<RenderCompletion>, String> {
    let proxy = proxy.clone();
    RenderWorker::spawn(move || {
        let _ = proxy.send_event(ShellEvent::RenderReady(generation));
    })
    .map_err(|error| error.to_string())
}

fn request_adapter(instance: &wgpu::Instance, surface: &wgpu::Surface<'static>) -> Result<wgpu::Adapter, String> {
    pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions { compatible_surface: Some(surface), ..Default::default() }))
        .map_err(|e| format!("no GPU adapter for the window: {e}"))
}

fn request_renderer(
    adapter: &wgpu::Adapter,
    descriptor: &wgpu::DeviceDescriptor<'static>,
    format: wgpu::TextureFormat,
    number: u64,
    proxy: &EventLoopProxy<ShellEvent>,
) -> Result<(GpuRenderer, Arc<DeviceGeneration>), String> {
    let mut descriptor = descriptor.clone();
    let features = adapter.features();
    let limits = adapter.limits();
    // A replacement adapter can offer fewer optional timestamp features.
    descriptor.required_features.remove(TRACE_FEATURES.difference(features));
    // The renderer uses nine varyings. The iOS simulator can expose fifteen instead of the default sixteen.
    let varyings = &mut descriptor.required_limits.max_inter_stage_shader_variables;
    *varyings = (*varyings).min(limits.max_inter_stage_shader_variables);
    if *varyings < 9 || !descriptor.required_limits.check_limits(&limits) || !features.contains(descriptor.required_features) {
        return Err("the adapter lacks the renderer's required GPU capabilities".into());
    }
    let (device, queue) = pollster::block_on(adapter.request_device(&descriptor)).map_err(|e| e.to_string())?;
    let generation = Arc::new(DeviceGeneration { number, loss: OnceLock::new() });
    let signal = generation.clone();
    let proxy = proxy.clone();
    device.set_device_lost_callback(move |reason, message| {
        if signal.loss.set((reason, message)).is_ok() {
            let _ = proxy.send_event(ShellEvent::GpuLost(signal.number));
        }
    });
    let renderer =
        generation.run(|| GpuRenderer::with_device(device, queue, format)).ok_or("the GPU device was lost while creating its renderer")?;
    Ok((renderer, generation))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn late_loss_belongs_to_its_device_generation() {
        let retired = DeviceGeneration { number: 2, loss: OnceLock::new() };
        let current = DeviceGeneration { number: 3, loss: OnceLock::new() };
        retired.loss.set((wgpu::DeviceLostReason::Destroyed, String::new())).unwrap();
        assert!(retired.loss_for(2).is_some());
        assert!(current.loss_for(2).is_none());
        assert_eq!(current.run(|| 42), Some(42));
        assert_eq!(retired.run(|| panic!("a lost device must stay idle")), None::<()>);
    }

    #[test]
    fn confirmed_loss_recovers_a_wgpu_poll_panic_and_preserves_author_panics() {
        let generation = DeviceGeneration { number: 0, loss: OnceLock::new() };
        let message = "Parent device is lost";
        let result = generation.run(|| {
            generation.loss.set((wgpu::DeviceLostReason::Unknown, message.to_owned())).unwrap();
            panic!("Error in Device::poll: {message}");
        });
        assert_eq!(result, None::<()>);

        let generation = DeviceGeneration { number: 1, loss: OnceLock::new() };
        let result = catch_unwind(|| {
            generation.run(|| {
                generation.loss.set((wgpu::DeviceLostReason::Unknown, message.to_owned())).unwrap();
                panic!("author drawing failed");
            });
        });
        assert_eq!(result.unwrap_err().downcast_ref::<&str>(), Some(&"author drawing failed"));
    }
}
