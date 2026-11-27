//! Asynchronously renders committed custom paint plans onto browser WebGPU canvases.

use octara::tree::tables::CustomPaintBinding;
use octara_model::geometry::{LayoutBox, Rect};
use octara_model::identity::NodeId;
use octara_model::paint::{CustomPaint, Quad};
use octara_model::theme::Color;
use octara_render::gpu::{DeviceLoss, ErrorScopes, SurfaceRendererFactory, local};
use octara_render::wgpu::{self, CurrentSurfaceTexture, Surface};
use rustc_hash::FxHashMap;
use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::{Rc, Weak};
use std::sync::Arc;
use std::time::Duration;
use wasm_bindgen::{JsCast, JsValue, closure::Closure, prelude::wasm_bindgen};
use wasm_bindgen_futures::{JsFuture, spawn_local};
use web_sys::{HtmlCanvasElement, HtmlElement, MessageChannel, MessageEvent, MessagePort};

pub(crate) struct Plan {
    element: HtmlElement,
    frame: local::Frame,
}

impl Plan {
    pub(crate) fn new(
        node: NodeId,
        paint: &CustomPaintBinding,
        element: &HtmlElement,
        layout: LayoutBox,
        radius: [f32; 4],
        scale: f32,
        now: Duration,
    ) -> Option<Self> {
        if matches!(element.local_name().as_str(), "input" | "textarea" | "canvas" | "img") {
            return None;
        }
        let rect = layout.rect;
        let border = layout.border;
        let inner =
            Rect::new(rect.x + border.left, rect.y + border.top, rect.w - border.left - border.right, rect.h - border.top - border.bottom);
        if inner.is_empty() {
            return None;
        }
        let fallback =
            Quad { source: node, rect: inner, radius, fill: None, gradient: None, border: [0.0; 4], border_color: Color::TRANSPARENT };
        let paint = CustomPaint {
            fallback,
            program: paint.program.clone(),
            mount: paint.mount.clone(),
            exiting: false,
            input: paint.input.clone(),
            opacity: 1.0,
        };
        let frame = local::Frame::new(paint, scale, now)?;
        Some(Self { element: element.clone(), frame })
    }
}

pub(crate) struct Canvases {
    state: RefCell<State>,
    messages: Option<WakeChannel>,
    surface_renderer: Option<SurfaceRendererFactory>,
}

struct WakeChannel {
    receiver: MessagePort,
    sender: MessagePort,
    _on_message: Closure<dyn FnMut(MessageEvent)>,
}

impl WakeChannel {
    fn new(owner: Weak<Canvases>) -> Option<Self> {
        let pair = MessageChannel::new().ok()?;
        let receiver = pair.port1();
        let sender = pair.port2();
        let on_message = Closure::new(move |_: MessageEvent| {
            if let Some(owner) = owner.upgrade() {
                owner.schedule(false);
            }
        });
        receiver.set_onmessage(Some(on_message.as_ref().unchecked_ref()));
        Some(Self { receiver, sender, _on_message: on_message })
    }
}

impl Drop for WakeChannel {
    fn drop(&mut self) {
        self.receiver.set_onmessage(None);
        self.receiver.close();
        self.sender.close();
    }
}

#[derive(Default)]
struct State {
    mounts: FxHashMap<NodeId, Target>,
    pending: VecDeque<NodeId>,
    device: Option<Device>,
    running: bool,
    unavailable: bool,
    recovering: bool,
}

impl Canvases {
    pub(crate) fn new(surface_renderer: Option<SurfaceRendererFactory>) -> Rc<Self> {
        Rc::new_cyclic(|owner| Self { state: RefCell::default(), messages: WakeChannel::new(owner.clone()), surface_renderer })
    }

    pub(crate) fn publish(self: &Rc<Self>, revision: u64, changes: impl IntoIterator<Item = (NodeId, Option<Plan>)>) {
        let mut state = self.state.borrow_mut();
        let mut retired = Vec::new();
        let mut replaced = Vec::new();
        for (id, plan) in changes {
            let previous = state.mounts.get(&id);
            if previous.is_some_and(|target| plan.as_ref().is_none_or(|plan| !target.element.is_same_node(Some(&plan.element)))) {
                if let Some(mut target) = state.mounts.remove(&id) {
                    target.detach();
                    retired.push(target);
                }
                state.pending.retain(|&key| key != id);
            }
            let Some(Plan { element, frame }) = plan else { continue };
            let State { mounts, pending, unavailable, .. } = &mut *state;
            match mounts.entry(id) {
                std::collections::hash_map::Entry::Vacant(entry) => {
                    if let Ok(target) = Target::new(element.clone(), frame, revision) {
                        target.status(if *unavailable { "unavailable" } else { "pending" });
                        entry.insert(target);
                        if !*unavailable {
                            pending.push_back(id);
                        }
                    } else {
                        let _ = element.set_attribute("data-octara-gpu", "failed");
                    }
                }
                std::collections::hash_map::Entry::Occupied(mut entry) => {
                    let target = entry.get_mut();
                    if *target.frame != frame {
                        if target.frame.extent(u32::MAX) != frame.extent(u32::MAX) {
                            target.show(false);
                        }
                        replaced.push(std::mem::replace(&mut target.frame, Rc::new(frame)));
                        target.revision = revision;
                        if !*unavailable {
                            target.status("pending");
                            if !pending.contains(&id) {
                                pending.push_back(id);
                            }
                        }
                    }
                }
            }
        }
        let device = if state.mounts.is_empty() {
            state.unavailable = false;
            state.recovering = false;
            state.device.take()
        } else {
            None
        };
        let prune = !retired.is_empty();
        drop(state);
        if prune || !replaced.is_empty() || device.is_some() {
            // Paint program destructors run outside the active runtime turn.
            spawn_local(async move { drop((retired, replaced, device)) });
        }
        self.schedule(prune);
    }

    fn schedule(self: &Rc<Self>, prune: bool) {
        let mut state = self.state.borrow_mut();
        let lost = state.device.as_ref().is_some_and(Device::lost);
        if state.running || state.unavailable || state.mounts.is_empty() || (state.pending.is_empty() && !lost && !prune) {
            return;
        }
        state.running = true;
        let device = state.device.take();
        spawn_local(Self::drive(Rc::downgrade(self), device));
    }

    async fn drive(owner: Weak<Self>, mut device: Option<Device>) {
        loop {
            let work = {
                let Some(host) = owner.upgrade() else { return };
                let mut state = host.state.borrow_mut();
                if state.mounts.is_empty() {
                    state.running = false;
                    return;
                }
                if device.as_ref().is_some_and(Device::lost) {
                    state.reset_surfaces();
                    drop(device.take());
                    if state.recovering {
                        state.fail("the replacement WebGPU device was lost before drawing");
                        return;
                    }
                    state.recovering = true;
                }
                if state.pending.is_empty() {
                    drop(state);
                    if let Some(device) = &mut device {
                        device.renderer.prune();
                    }
                    {
                        let mut state = host.state.borrow_mut();
                        if !state.mounts.is_empty() {
                            state.device = device.take();
                        }
                        state.running = false;
                    }
                    host.schedule(false);
                    return;
                }
                device.as_ref().map(|_| {
                    let key = state.pending.pop_front().expect("a pending Canvas");
                    let target = &state.mounts[&key];
                    (key, target.frame.clone(), target.revision)
                })
            };
            let Some((key, frame, revision)) = work else {
                let sender = owner.upgrade().and_then(|host| host.messages.as_ref().map(|messages| messages.sender.clone()));
                match Device::open(&owner, sender).await {
                    Ok(gpu) => device = gpu,
                    Err(error) => {
                        if let Some(host) = owner.upgrade() {
                            host.state.borrow_mut().fail(&error);
                        }
                        return;
                    }
                }
                continue;
            };
            let gpu = device.as_mut().expect("a Canvas has a device");
            let loss = gpu.loss.clone();
            if let Some(Some(report)) = loss.until_lost(Self::draw(&owner, gpu, key, &frame, revision)).await
                && let Some(host) = owner.upgrade()
            {
                let mut state = host.state.borrow_mut();
                if let Some(target) = state.current(key, &frame) {
                    let submitted = report.is_ok();
                    target.accept(report);
                    if submitted {
                        state.recovering = false;
                    }
                }
            }
        }
    }

    async fn draw(
        owner: &Weak<Self>,
        gpu: &mut Device,
        key: NodeId,
        frame: &Rc<local::Frame>,
        revision: u64,
    ) -> Option<Result<(), String>> {
        let output = match frame.extent(gpu.device.limits().max_texture_dimension_2d) {
            Some(size) => gpu.renderer.draw(&gpu.device, &gpu.queue, frame, size, revision).await,
            None => Err("the Canvas size exceeds this WebGPU device's texture limit".into()),
        };
        let report = match output {
            Ok(view) => {
                let receipt = owner.upgrade().and_then(|host| {
                    let mut state = host.state.borrow_mut();
                    let target = state.current(key, frame)?;
                    if gpu.lost() {
                        return None;
                    }
                    let scopes = ErrorScopes::new(&gpu.device);
                    let presented = target.present(gpu, &view);
                    Some((presented, scopes.pop()))
                });
                if let Some((published, validation)) = receipt {
                    match validation.await {
                        Some(error) => Some(Err(format!("composing a local Canvas: {error}"))),
                        None => Some(published),
                    }
                } else {
                    None
                }
            }
            Err(error) => Some(Err(error)),
        };
        // A healthy queue retains one attempt even when the DOM has already replaced it.
        gpu.complete().await;
        report
    }
}

impl State {
    fn current(&mut self, key: NodeId, frame: &Rc<local::Frame>) -> Option<&mut Target> {
        self.mounts.get_mut(&key).filter(|target| Rc::ptr_eq(&target.frame, frame))
    }

    fn reset_surfaces(&mut self) {
        self.pending.clear();
        for (&key, target) in &mut self.mounts {
            target.reset();
            target.status("pending");
            self.pending.push_back(key);
        }
    }

    fn fail(&mut self, error: &str) {
        self.running = false;
        self.pending.clear();
        self.unavailable = !self.mounts.is_empty();
        if self.unavailable {
            web_sys::console::warn_1(&format!("Octara WebGPU: {error}").into());
        }
        for target in self.mounts.values() {
            target.show(false);
            target.status("unavailable");
        }
    }
}

struct Target {
    element: HtmlElement,
    canvas: HtmlCanvasElement,
    frame: Rc<local::Frame>,
    revision: u64,
    surface: Option<Surface<'static>>,
}

impl Target {
    fn new(element: HtmlElement, frame: local::Frame, revision: u64) -> Result<Self, JsValue> {
        let document = element.owner_document().ok_or("the canvas mount must belong to a document")?;
        let canvas: HtmlCanvasElement = document.create_element("canvas")?.unchecked_into();
        canvas.set_attribute("aria-hidden", "true")?;
        canvas.set_attribute("data-octara-gpu-layer", "")?;
        canvas.set_attribute("style", "position:absolute;inset:0;width:100%;height:100%;display:block;box-sizing:border-box;border-radius:inherit;z-index:-1;pointer-events:none;visibility:hidden;")?;
        element.append_child(&canvas)?;
        Ok(Self { element, canvas, frame: Rc::new(frame), revision, surface: None })
    }

    fn status(&self, status: &str) {
        let _ = self.element.set_attribute("data-octara-gpu", status);
    }

    fn show(&self, visible: bool) {
        let _ = self.canvas.style().set_property("visibility", if visible { "visible" } else { "hidden" });
    }

    fn reset(&mut self) {
        if self.surface.take().is_some()
            && let Ok(Some(context)) = self.canvas.get_context("webgpu")
        {
            context.unchecked_ref::<GpuCanvasContext>().unconfigure();
        }
        self.show(false);
    }

    fn detach(&mut self) {
        self.reset();
        self.canvas.remove();
        let _ = self.element.remove_attribute("data-octara-gpu");
    }

    fn accept(&mut self, report: Result<(), String>) {
        match report {
            Ok(()) => {
                self.show(true);
                self.status("submitted");
            }
            Err(error) => {
                self.reset();
                self.status("failed");
                web_sys::console::warn_1(&format!("Octara WebGPU: {error}").into());
            }
        }
    }

    fn present(&mut self, gpu: &mut Device, source: &wgpu::TextureView) -> Result<(), String> {
        let surface = match &self.surface {
            Some(surface) => surface,
            None => self
                .surface
                .insert(gpu.instance.create_surface(wgpu::SurfaceTarget::Canvas(self.canvas.clone())).map_err(|e| e.to_string())?),
        };
        let texture = source.texture();
        let (width, height) = (texture.width(), texture.height());
        if surface.get_configuration().is_none_or(|config| (config.width, config.height) != (width, height)) {
            let mut config =
                surface.get_default_config(&gpu.adapter, width, height).ok_or("the WebGPU device does not support this canvas")?;
            config.alpha_mode = wgpu::CompositeAlphaMode::PreMultiplied;
            surface.configure(&gpu.device, &config);
        }
        let target = match surface.get_current_texture() {
            CurrentSurfaceTexture::Success(target) | CurrentSurfaceTexture::Suboptimal(target) => target,
            _ => return Err("could not acquire the WebGPU canvas surface".into()),
        };
        let view = target.texture.create_view(&Default::default());
        gpu.queue.submit([gpu.renderer.resolve(&gpu.device, source, &view)]);
        gpu.queue.present(target);
        Ok(())
    }
}

#[wasm_bindgen]
extern "C" {
    type GpuCanvasContext;

    #[wasm_bindgen(method)]
    fn unconfigure(this: &GpuCanvasContext);
}

struct Device {
    instance: wgpu::Instance,
    adapter: wgpu::Adapter,
    device: wgpu::Device,
    queue: wgpu::Queue,
    renderer: local::Renderer,
    loss: Arc<DeviceLoss>,
}

impl Device {
    async fn open(owner: &Weak<Canvases>, sender: Option<MessagePort>) -> Result<Option<Self>, String> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::BROWSER_WEBGPU,
            ..wgpu::InstanceDescriptor::new_without_display_handle()
        });
        let adapter = instance.request_adapter(&wgpu::RequestAdapterOptions::default()).await.map_err(|error| error.to_string())?;
        if owner.upgrade().is_none_or(|host| host.state.borrow().mounts.is_empty()) {
            return Ok(None);
        }
        let descriptor = wgpu::DeviceDescriptor { label: Some("Octara WebGPU"), ..Default::default() };
        let (device, queue) = adapter.request_device(&descriptor).await.map_err(|error| error.to_string())?;
        let loss = Arc::new(DeviceLoss::default());
        let live = Arc::downgrade(&loss);
        device.set_device_lost_callback(move |reason, message| {
            if let Some(loss) = live.upgrade()
                && loss.record(reason, message)
                && let Some(sender) = &sender
            {
                let _ = sender.post_message(&JsValue::UNDEFINED);
            }
        });
        let factory = owner.upgrade().and_then(|host| host.surface_renderer.clone());
        let renderer = factory.map_or_else(local::Renderer::default, |factory| local::Renderer::with_surface_renderer(factory(&device)));
        Ok(Some(Self { instance, adapter, device, queue, renderer, loss }))
    }

    fn lost(&self) -> bool {
        self.loss.get().is_some()
    }

    async fn complete(&self) {
        let done = js_sys::Promise::new(&mut |resolve, _| {
            self.queue.on_submitted_work_done(move || drop(resolve.call0(&JsValue::UNDEFINED)));
        });
        let _ = JsFuture::from(done).await;
    }
}

impl Drop for Device {
    fn drop(&mut self) {
        self.device.destroy();
    }
}

impl Drop for Canvases {
    fn drop(&mut self) {
        for target in self.state.get_mut().mounts.values_mut() {
            target.detach();
        }
    }
}
