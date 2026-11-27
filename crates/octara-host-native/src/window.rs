//! Per-window negotiation of surfaces, frame admission and native publication.

use crate::os::{ActiveEditorView, Drawable, DrawableAcquisition, OsWindow, surface_drawable};
use crate::presentation::Presentation;
use octara::runtime::{WindowId, WindowOptions, WindowRuntime};
use octara::tree::editor::EditorSession;
use octara::tree::resolve_cursor;
use octara_gpu::{DeviceExecutor, DeviceGeneration, DrawWork, DrawnFrame, FrameProgress, GpuError, GpuTask, SurfaceRendererFactory};
use octara_model::geometry::{Point, Size, WindowViewport};
use octara_model::identity::TargetGuard;
use octara_model::input::{CursorStyle, DroppedFile, Modifiers};
use octara_model::theme::ColorToken;
use octara_model::window::{WindowChrome, WindowGeometryRequest};
use octara_pipeline::{NativeFrame, NativePublication};
use octara_render::{gpu::OutputColor, wgpu};
use std::sync::Arc;
use std::time::{Duration, Instant};
use winit::window::{CursorIcon, Window};

pub(crate) struct Device {
    pub adapter: wgpu::Adapter,
    pub queue: wgpu::Queue,
    pub executor: DeviceExecutor,
}

impl Device {
    pub fn create(
        instance: &wgpu::Instance,
        surface: &wgpu::Surface<'static>,
        generation: Arc<DeviceGeneration>,
        profile: bool,
        surface_renderer: Option<&SurfaceRendererFactory>,
        thread_setup: fn(),
    ) -> Result<Self, String> {
        pollster::block_on(async {
            let adapter = instance
                .request_adapter(&wgpu::RequestAdapterOptions {
                    power_preference: wgpu::PowerPreference::HighPerformance,
                    compatible_surface: Some(surface),
                    force_fallback_adapter: false,
                    ..Default::default()
                })
                .await
                .map_err(|error| error.to_string())?;
            let features = if profile { adapter.features() & wgpu::Features::TIMESTAMP_QUERY } else { wgpu::Features::empty() };
            let (device, queue) = adapter
                .request_device(&wgpu::DeviceDescriptor {
                    label: Some("Octara native device"),
                    required_features: features,
                    required_limits: adapter.limits(),
                    ..Default::default()
                })
                .await
                .map_err(|error| error.to_string())?;
            let executor =
                DeviceExecutor::spawn_for_device_with_thread_setup(device, queue.clone(), generation, surface_renderer, thread_setup)
                    .map_err(|error| error.to_string())?;
            Ok(Self { adapter, queue, executor })
        })
    }
}

#[derive(Clone, Copy)]
struct SurfaceInfo {
    color: OutputColor,
    width: u32,
    height: u32,
}

pub(crate) struct FileDrop {
    pub target: Option<TargetGuard>,
    pub files: Vec<DroppedFile>,
}

pub(crate) struct HostWindow {
    pub id: WindowId,
    pub window: Arc<Window>,
    pub os: Box<dyn OsWindow>,
    pub a11y: accesskit_winit::Adapter,
    pub publication: NativePublication,
    pub synchronized: Option<u64>,
    pub presented: Option<FrameProgress>,
    pub pointer: Option<Point>,
    pub file_drop: Option<FileDrop>,
    pub modifiers: Modifiers,
    pub chrome: WindowChrome,
    pub initial_fullscreen: bool,
    pub service_cleanup: std::collections::HashMap<octara_model::ask::ServiceId, crate::os::ServiceCleanup>,
    surface: Arc<wgpu::Surface<'static>>,
    presentation: Presentation,
    surface_info: Option<SurfaceInfo>,
    transparent: bool,
    draw: Option<GpuTask<DrawnFrame<Box<dyn Drawable>>>>,
    cursor: Option<CursorIcon>,
    redraw_at: Option<Instant>,
    suspended: bool,
}

impl HostWindow {
    pub fn new(
        id: WindowId,
        window: Arc<Window>,
        os: Box<dyn OsWindow>,
        a11y: accesskit_winit::Adapter,
        surface: Arc<wgpu::Surface<'static>>,
        publication: NativePublication,
        transparent: bool,
    ) -> Self {
        Self {
            id,
            window,
            os,
            a11y,
            publication,
            synchronized: None,
            presented: None,
            pointer: None,
            file_drop: None,
            modifiers: Modifiers::empty(),
            chrome: WindowChrome::default(),
            initial_fullscreen: false,
            service_cleanup: std::collections::HashMap::new(),
            surface,
            presentation: Presentation::default(),
            surface_info: None,
            transparent,
            draw: None,
            cursor: Some(CursorIcon::Default),
            redraw_at: None,
            suspended: false,
        }
    }

    pub fn viewport(&mut self) -> WindowViewport {
        let size = self.window.inner_size();
        let scale = self.window.scale_factor().max(f64::EPSILON) as f32;
        let viewport = WindowViewport { size: Size::new(size.width as f32 / scale, size.height as f32 / scale), scale, safe_area: None };
        self.os.scene().map_or(viewport, |scene| scene.viewport(viewport))
    }

    pub fn surface(&self) -> Arc<wgpu::Surface<'static>> {
        self.surface.clone()
    }

    pub fn can_configure(&self, now: Instant) -> bool {
        self.configure_needed() && self.redraw_at.is_none_or(|deadline| deadline <= now)
    }

    pub fn needs_unpaced_frame(&mut self, now: Instant) -> bool {
        !self.externally_paced()
            && self.draw.is_none()
            && self.redraw_at.is_none_or(|deadline| deadline <= now)
            && self.publication.latest().is_some_and(|frame| !self.presentation.presented_revision(frame.number))
    }

    fn externally_paced(&mut self) -> bool {
        self.os.presentation().is_some_and(|presentation| presentation.externally_paced())
    }

    fn set_frame_clock_active(&mut self, active: bool) {
        if let Some(presentation) = self.os.presentation() {
            presentation.set_frame_clock_active(active);
        }
    }

    pub fn configure_needed(&self) -> bool {
        let size = self.size();
        !self.suspended && !size.contains(&0) && !self.presentation.ready(size)
    }

    fn size(&self) -> [u32; 2] {
        let extent = self.window.inner_size();
        [extent.width, extent.height]
    }

    pub fn invalidate_surface(&mut self, rebuild: bool) {
        self.presentation.invalidate(rebuild);
        // The worker retains any target until abandoned job cleanup. Reconfiguration waits on the drain barrier.
        self.draw = None;
    }

    pub fn set_suspended(&mut self, suspended: bool) {
        self.suspended = suspended;
        if suspended {
            self.invalidate_surface(true);
            self.set_frame_clock_active(false);
        }
        if let Some(scene) = self.os.scene() {
            scene.set_suspended(suspended);
        } else if suspended {
            self.os.text_input().focused(false);
        }
    }

    pub fn configure(&mut self, instance: &wgpu::Instance, adapter: &wgpu::Adapter, device: &wgpu::Device) -> Result<bool, String> {
        if !self.configure_needed() {
            return Ok(true);
        }
        if self.os.presentation().is_some_and(|presentation| !presentation.surface_will_reconfigure()) {
            self.redraw_at = Some(Instant::now() + Duration::from_millis(8));
            return Ok(false);
        }
        if self.presentation.rebuild() {
            self.surface = Arc::new(instance.create_surface(self.window.clone()).map_err(|error| error.to_string())?);
        }
        let [width, height] = self.size();
        let mut config = self.surface.get_default_config(adapter, width, height).ok_or("no compatible surface format")?;
        let supported = self.surface.get_capabilities(adapter);
        if let Some(format) = supported.formats.iter().find(|format| format.is_srgb()) {
            config.format = *format;
        }
        config.present_mode = wgpu::PresentMode::AutoVsync;
        config.usage |= supported.usages & wgpu::TextureUsages::COPY_SRC;
        if let Some(alpha) = preferred_alpha_mode(self.transparent, &supported.alpha_modes) {
            config.alpha_mode = alpha;
        }
        let color = match config.alpha_mode {
            wgpu::CompositeAlphaMode::PreMultiplied => OutputColor::SrgbPremultiplied,
            wgpu::CompositeAlphaMode::PostMultiplied => OutputColor::SrgbStraight,
            _ => OutputColor::SrgbOpaque,
        };
        self.surface.configure(device, &config);
        self.surface_info = Some(SurfaceInfo { color, width, height });
        self.presentation.configure([width, height]);
        self.redraw_at = None;
        Ok(true)
    }

    pub fn synchronize(&mut self, runtime: &WindowRuntime, frame: &NativeFrame) {
        if self.synchronized == Some(frame.number) {
            return;
        }
        self.synchronized = Some(frame.number);
        let native_changes = self.publication.take_native_changes();
        let has_underlay = self.os.native_views().is_some_and(|native| {
            native.sync_native_views(&native_changes);
            native.has_underlay()
        });
        let transparent = transparent_background(runtime.options()) || has_underlay || self.os.window_is_vibrant();
        if transparent != self.transparent {
            self.transparent = transparent;
            self.window.set_transparent(transparent);
            self.invalidate_surface(false);
        }
        self.synchronize_editor(runtime);
        self.synchronize_cursor(runtime);
        if let Some(scene) = self.os.scene() {
            scene.sync_back(runtime.back_target());
        }
        self.a11y.update_if_active(|| frame.accessibility.clone());
        self.set_frame_clock_active(true);
        if !self.externally_paced() {
            self.window.request_redraw();
        }
    }

    pub fn synchronize_editor(&mut self, runtime: &WindowRuntime) {
        let frame = self.publication.latest();
        let editor = EditorSession::active(runtime.tree());
        let active = editor.as_ref().map(|session| {
            let editor = session.editor();
            ActiveEditorView {
                session: session.id(),
                document_rev: editor.document_rev(),
                value: editor.value(),
                selection: editor.selection(),
                composition: editor.composition(),
                editor,
                multiline: editor.multiline,
                secure: editor.is_secure(),
                caret: frame.as_ref().and_then(|frame| frame.editor_caret),
                geometry: frame.as_ref().and_then(|frame| frame.editor_geometry.as_ref()),
            }
        });
        self.os.text_input().sync_text_input(active.as_ref());
    }

    pub fn synchronize_cursor(&mut self, runtime: &WindowRuntime) {
        let hit = self.pointer.and_then(|pos| runtime.hit_test(pos)).and_then(|target| target.hit.node());
        let cursor = cursor_icon(resolve_cursor(runtime.tree(), hit));
        if cursor == self.cursor {
            return;
        }
        if let Some(icon) = cursor {
            self.window.set_cursor(icon);
        }
        if cursor.is_some() != self.cursor.is_some() {
            self.window.set_cursor_visible(cursor.is_some());
        }
        self.cursor = cursor;
    }

    pub fn draw(&mut self, runtime: &WindowRuntime, device: &Device, sample: Duration, now: Instant) -> Result<(), String> {
        if self.draw.is_some() || self.suspended || self.configure_needed() || self.redraw_at.is_some_and(|deadline| deadline > now) {
            return Ok(());
        }
        let Some(surface) = self.surface_info.filter(|info| self.presentation.ready([info.width, info.height])) else { return Ok(()) };
        let Some(frame) = self.publication.latest() else { return Ok(()) };
        let sample = if frame.scene.as_ref().is_some_and(|scene| scene.continuously_animated(frame.sampled_at.as_secs_f64())) {
            sample
        } else {
            frame.sampled_at
        };
        if !self.presentation.needs_draw(frame.number, sample) {
            return Ok(());
        }
        let target = match self.os.presentation() {
            Some(presentation) => presentation.try_acquire_drawable(&self.surface, surface.color),
            None => surface_drawable(&self.surface, surface.color),
        };
        match target {
            DrawableAcquisition::Ready(target) => {
                let list = frame.scene.as_ref().map_or_else(|| frame.draw.clone(), |scene| scene.sample(sample.as_secs_f64()));
                let stamp = self.presentation.stamp(runtime.tree().id(), device.executor.generation().number(), frame.number, sample);
                match device.executor.draw(DrawWork { target, list, scale: self.viewport().scale, stamp, valid_until: None, report: None })
                {
                    Ok(draw) => self.draw = Some(draw),
                    Err(GpuError::Busy) => self.redraw_at = Some(now + Duration::from_millis(8)),
                    Err(_) if device.executor.generation().is_lost() => return Ok(()),
                    Err(error) => return Err(error.to_string()),
                }
            }
            DrawableAcquisition::Retry => self.redraw_at = Some(now + Duration::from_millis(16)),
            DrawableAcquisition::Reconfigure => self.invalidate_surface(false),
            DrawableAcquisition::Lost => self.invalidate_surface(true),
            DrawableAcquisition::Suspended => {}
        }
        Ok(())
    }

    pub fn finish_draw(&mut self, runtime: &WindowRuntime, device: &Device) -> Result<bool, String> {
        let Some(result) = self.draw.as_mut().and_then(GpuTask::try_take) else { return Ok(false) };
        self.draw = None;
        let frame = match result {
            Ok(frame) => frame,
            Err(_) if device.executor.generation().is_lost() => return Ok(false),
            Err(error) => return Err(error.to_string()),
        };
        let latest = self.publication.latest().map_or(0, |frame| frame.number);
        if !self.presentation.accepts(frame.stamp, runtime.tree().id(), device.executor.generation().number(), latest, self.size()) {
            self.window.request_redraw();
            return Ok(false);
        }
        let suboptimal = frame.target.suboptimal();
        let progress = frame.progress();
        self.window.pre_present_notify();
        match self.os.presentation() {
            Some(presentation) => presentation.present(frame.target, &device.queue)?,
            None => frame.target.present(&device.queue)?,
        }
        if let Err(error) = device.executor.accept_presentation(frame.presentation)
            && !device.executor.generation().is_lost()
        {
            return Err(error.to_string());
        }
        if let Some(scene) = self.os.scene() {
            scene.did_present();
        }
        let first = self.presentation.did_present(frame.stamp);
        self.presented = Some(progress);
        if first {
            self.os.show(&self.window);
            if std::mem::take(&mut self.initial_fullscreen) {
                self.os.set_window_geometry(&self.window, WindowGeometryRequest { fullscreen: Some(true), ..Default::default() });
            }
        }
        if suboptimal {
            self.invalidate_surface(false);
        }
        if let Some(frame) = self.publication.latest() {
            let motion = frame.scene.as_ref().is_some_and(|scene| scene.continuously_animated(frame.sampled_at.as_secs_f64()));
            self.set_frame_clock_active(motion);
        }
        Ok(true)
    }

    pub fn take_retry(&mut self, now: Instant) -> bool {
        if self.redraw_at.is_some_and(|deadline| deadline <= now) {
            self.redraw_at = None;
            return true;
        }
        false
    }

    pub fn deadline(&self) -> Option<Instant> {
        self.redraw_at
    }
}

impl Drop for HostWindow {
    fn drop(&mut self) {
        self.set_frame_clock_active(false);
        for (_, cancel) in self.service_cleanup.drain() {
            cancel();
        }
    }
}

pub(crate) fn transparent_background(options: &WindowOptions) -> bool {
    options.theme.get(ColorToken::Background).components[3] < 1.0
}

fn preferred_alpha_mode(transparent: bool, modes: &[wgpu::CompositeAlphaMode]) -> Option<wgpu::CompositeAlphaMode> {
    use wgpu::CompositeAlphaMode::*;
    let preference = if transparent { &[PreMultiplied, PostMultiplied][..] } else { &[Opaque] };
    preference.iter().copied().find(|mode| modes.contains(mode))
}

fn cursor_icon(style: CursorStyle) -> Option<CursorIcon> {
    Some(match style {
        CursorStyle::Arrow | CursorStyle::Auto => CursorIcon::Default,
        CursorStyle::IBeam => CursorIcon::Text,
        CursorStyle::PointingHand => CursorIcon::Pointer,
        CursorStyle::Crosshair => CursorIcon::Crosshair,
        CursorStyle::OpenHand => CursorIcon::Grab,
        CursorStyle::ClosedHand => CursorIcon::Grabbing,
        CursorStyle::ResizeLeftRight => CursorIcon::EwResize,
        CursorStyle::ResizeUpDown => CursorIcon::NsResize,
        CursorStyle::ResizeColumn => CursorIcon::ColResize,
        CursorStyle::ResizeRow => CursorIcon::RowResize,
        CursorStyle::OperationNotAllowed => CursorIcon::NotAllowed,
        CursorStyle::Wait => CursorIcon::Wait,
        CursorStyle::Move => CursorIcon::Move,
        CursorStyle::Help => CursorIcon::Help,
        CursorStyle::None => return None,
        CursorStyle::ContextMenu => CursorIcon::ContextMenu,
        CursorStyle::Progress => CursorIcon::Progress,
        CursorStyle::Cell => CursorIcon::Cell,
        CursorStyle::VerticalText => CursorIcon::VerticalText,
        CursorStyle::Alias => CursorIcon::Alias,
        CursorStyle::Copy => CursorIcon::Copy,
        CursorStyle::NoDrop => CursorIcon::NoDrop,
        CursorStyle::AllScroll => CursorIcon::AllScroll,
        CursorStyle::ResizeNorth => CursorIcon::NResize,
        CursorStyle::ResizeEast => CursorIcon::EResize,
        CursorStyle::ResizeSouth => CursorIcon::SResize,
        CursorStyle::ResizeWest => CursorIcon::WResize,
        CursorStyle::ResizeNorthEast => CursorIcon::NeResize,
        CursorStyle::ResizeNorthWest => CursorIcon::NwResize,
        CursorStyle::ResizeSouthEast => CursorIcon::SeResize,
        CursorStyle::ResizeSouthWest => CursorIcon::SwResize,
        CursorStyle::ResizeNorthEastSouthWest => CursorIcon::NeswResize,
        CursorStyle::ResizeNorthWestSouthEast => CursorIcon::NwseResize,
        CursorStyle::ZoomIn => CursorIcon::ZoomIn,
        CursorStyle::ZoomOut => CursorIcon::ZoomOut,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use octara_model::theme::{Color, Theme};

    #[test]
    fn maps_visible_and_hidden_cursors_for_winit() {
        assert_eq!(cursor_icon(CursorStyle::Auto), Some(CursorIcon::Default));
        assert_eq!(cursor_icon(CursorStyle::Arrow), Some(CursorIcon::Default));
        assert_eq!(cursor_icon(CursorStyle::IBeam), Some(CursorIcon::Text));
        assert_eq!(cursor_icon(CursorStyle::PointingHand), Some(CursorIcon::Pointer));
        assert_eq!(cursor_icon(CursorStyle::OperationNotAllowed), Some(CursorIcon::NotAllowed));
        assert_eq!(cursor_icon(CursorStyle::ResizeLeftRight), Some(CursorIcon::EwResize));
        assert_eq!(cursor_icon(CursorStyle::None), None);
    }

    #[test]
    fn background_and_surface_alpha_are_opt_in() {
        let options = WindowOptions::default();
        assert!(!transparent_background(&options));
        for alpha in [0, 128] {
            let background = Color::from_rgba8(10, 20, 30, alpha);
            let options = WindowOptions { theme: Theme::default().color(ColorToken::Background, background), ..options.clone() };
            assert!(transparent_background(&options));
        }

        use wgpu::CompositeAlphaMode::*;
        let supported = [PostMultiplied, Opaque, PreMultiplied];
        assert_eq!(preferred_alpha_mode(false, &supported), Some(Opaque));
        assert_eq!(preferred_alpha_mode(true, &supported), Some(PreMultiplied));
        assert_eq!(preferred_alpha_mode(true, &[Opaque, PostMultiplied]), Some(PostMultiplied));
        assert_eq!(preferred_alpha_mode(true, &[Opaque]), None);
    }
}
