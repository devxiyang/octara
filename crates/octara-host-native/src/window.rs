//! Per-window negotiation of surfaces, frame admission and native publication.

use crate::os::{ActiveEditorView, Drawable, DrawableAcquisition, FrameClockActivity, OsWindow, surface_drawable};
use crate::presentation::{PendingNativeViews, Presentation};
use octara::module::{ModuleDispatcher, NativeViewInstance, NativeViewMounts};
use octara::runtime::engine::AccessibilityConsumer;
use octara::runtime::{AppRuntime, WindowId, WindowRuntime};
use octara::tree::editor::EditorSession;
use octara::tree::resolve_cursor;
use octara_gpu::{DeviceExecutor, DeviceGeneration, DrawWork, DrawnFrame, FrameProgress, GpuError, GpuTask, SurfaceRendererFactory};
use octara_model::geometry::{Point, Size, WindowViewport};
use octara_model::identity::TargetGuard;
use octara_model::input::{CursorStyle, DroppedFile, Modifiers};
use octara_model::native::NativeChange;
use octara_model::theme::{ColorToken, Theme};
use octara_model::window::{WindowChrome, WindowGeometryRequest};
use octara_pipeline::{NativeFrame, NativePublication};
use octara_render::{gpu::OutputColor, wgpu};
use std::sync::Arc;
use std::time::{Duration, Instant};

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
    pub native_mounts: NativeViewMounts,
    pending_module_views: PendingNativeViews<Box<dyn NativeViewInstance>>,
    module_dispatcher: Option<ModuleDispatcher>,
    // Drop the surface before releasing the native handles it borrows.
    surface: Arc<wgpu::Surface<'static>>,
    pub os: Box<dyn OsWindow>,
    accessibility_active: bool,
    pub publication: NativePublication,
    pub synchronized: Option<u64>,
    pub presented: Option<FrameProgress>,
    pub pointer: Option<Point>,
    pub pointer_modifiers: Modifiers,
    pub file_drop: Option<FileDrop>,
    pub chrome: WindowChrome,
    pub initial_fullscreen: bool,
    pub service_cleanup: std::collections::HashMap<octara_model::ask::ServiceId, crate::os::ServiceCleanup>,
    presentation: Presentation,
    surface_info: Option<SurfaceInfo>,
    transparent: bool,
    draw: Option<GpuTask<DrawnFrame<Box<dyn Drawable>>>>,
    cursor: CursorStyle,
    redraw_at: Option<Instant>,
    suspended: bool,
}

impl HostWindow {
    pub fn new(
        id: WindowId,
        os: Box<dyn OsWindow>,
        surface: Arc<wgpu::Surface<'static>>,
        publication: NativePublication,
        transparent: bool,
    ) -> Self {
        Self {
            id,
            native_mounts: NativeViewMounts::default(),
            pending_module_views: PendingNativeViews::default(),
            module_dispatcher: None,
            os,
            accessibility_active: false,
            publication,
            synchronized: None,
            presented: None,
            pointer: None,
            pointer_modifiers: Modifiers::empty(),
            file_drop: None,
            chrome: WindowChrome::default(),
            initial_fullscreen: false,
            service_cleanup: std::collections::HashMap::new(),
            surface,
            presentation: Presentation::default(),
            surface_info: None,
            transparent,
            draw: None,
            cursor: CursorStyle::Arrow,
            redraw_at: None,
            suspended: false,
        }
    }

    pub fn viewport(&mut self) -> WindowViewport {
        let [width, height] = self.os.physical_size();
        let scale = self.os.scale_factor().max(f32::EPSILON);
        let viewport =
            WindowViewport { size: Size::new(width as f32 / scale, height as f32 / scale), scale, safe_area: None, keyboard_inset: 0.0 };
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

    pub fn externally_paced(&mut self) -> bool {
        self.os.presentation().is_some_and(|presentation| presentation.externally_paced())
    }

    pub fn activate_frame_clock(&mut self) -> bool {
        let paced = self.externally_paced();
        if paced && !self.suspended {
            self.set_frame_clock_activity(FrameClockActivity::Interactive);
        }
        paced
    }

    pub fn idle_frame_clock_if_clean(&mut self) {
        if self.draw.is_none()
            && !self.suspended
            && self.publication.latest().is_none_or(|frame| {
                self.presentation.presented_revision(frame.number)
                    && frame.scene.as_ref().is_none_or(|scene| !scene.continuously_animated(frame.sampled_at.as_secs_f64()))
            })
        {
            self.set_frame_clock_activity(FrameClockActivity::Idle);
        }
    }

    pub fn request_frame(&mut self) {
        if !self.suspended && !self.activate_frame_clock() {
            self.os.request_redraw();
        }
    }

    fn set_frame_clock_activity(&mut self, activity: FrameClockActivity) {
        if let Some(presentation) = self.os.presentation() {
            presentation.set_frame_clock_activity(activity);
        }
    }

    pub fn configure_needed(&self) -> bool {
        let size = self.size();
        !self.suspended && !size.contains(&0) && !self.presentation.ready(size)
    }

    fn size(&self) -> [u32; 2] {
        self.os.physical_size()
    }

    pub fn invalidate_surface(&mut self, rebuild: bool) {
        self.presentation.invalidate(rebuild);
        // The worker retains any target until abandoned job cleanup. Reconfiguration waits on the drain barrier.
        self.draw = None;
        self.abandon_native_view_frame();
    }

    fn abandon_native_view_frame(&mut self) {
        self.pending_module_views.abandon();
        if let Some(presentation) = self.os.presentation() {
            presentation.abandon_native_view_frame();
        }
    }

    pub fn set_suspended(&mut self, suspended: bool) {
        self.suspended = suspended;
        if suspended {
            self.invalidate_surface(true);
            self.set_frame_clock_activity(FrameClockActivity::Paused);
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
            self.surface = create_surface(instance, self.os.as_ref())?;
        }
        let [width, height] = self.size();
        let mut config = self.surface.get_default_config(adapter, width, height).ok_or("no compatible surface format")?;
        let supported = self.surface.get_capabilities(adapter);
        if let Some(format) = supported.formats.iter().find(|format| format.is_srgb()) {
            config.format = *format;
        }
        config.present_mode = wgpu::PresentMode::AutoVsync;
        config.usage |= supported.usages & (wgpu::TextureUsages::COPY_SRC | wgpu::TextureUsages::COPY_DST);
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

    pub fn synchronize_native(&mut self, runtime: &mut AppRuntime, dispatcher: &ModuleDispatcher) {
        self.module_dispatcher.get_or_insert_with(|| dispatcher.clone());
        let changes = self.publication.take_native_changes();
        if !changes.is_empty() {
            self.apply_native_changes(changes, runtime, dispatcher);
        }
        if let Some(frame) = self.publication.latest() {
            for (id, hidden) in self.native_mounts.snapshot_visibility_changes(|id| frame.contains_native_snapshot(id)) {
                self.pending_module_views.set_snapshot_visibility(id, hidden);
                if hidden {
                    self.publication.retain_native_snapshot(id);
                }
            }
        }
        self.capture_native_airspace();
    }

    fn apply_native_changes(&mut self, changes: Vec<NativeChange>, runtime: &mut AppRuntime, dispatcher: &ModuleDispatcher) {
        let surface = self.os.host_surface();
        let mut built_in = Vec::new();
        for change in changes {
            match &change {
                NativeChange::Place { placements } => {
                    for placement in placements {
                        if self.native_mounts.contains(placement.id) {
                            self.pending_module_views.insert(placement.clone());
                        }
                    }
                    built_in.push(change);
                    continue;
                }
                NativeChange::Destroy { id } => {
                    self.publication.forget_native_snapshot(*id);
                    if let Some((view, placed)) = self.native_mounts.retire(*id) {
                        self.pending_module_views.retire(*id, view, placed);
                    } else {
                        built_in.push(change);
                    }
                    continue;
                }
                _ => {}
            }
            match self.native_mounts.apply_change(&change, surface, runtime.native_views_mut(), dispatcher) {
                Ok(false) => built_in.push(change),
                Ok(true) => {}
                Err(error) => runtime.report_native_view_mount_error(&error),
            }
        }
        if !built_in.is_empty()
            && let Some(native) = self.os.native_views()
        {
            native.sync_native_views(&built_in);
        }
    }

    fn capture_native_airspace(&mut self) {
        if !self.suspended
            && let Some(dispatcher) = &self.module_dispatcher
        {
            self.publication.capture_native_airspace(&mut self.native_mounts, dispatcher);
        }
    }

    pub fn synchronize(&mut self, runtime: &WindowRuntime, frame: &NativeFrame) {
        self.synchronized = Some(frame.number);
        let has_underlay = self.native_mounts.has_underlay() || self.os.native_views().is_some_and(|native| native.has_underlay());
        let transparent = transparent_background(&runtime.presentation().theme) || has_underlay || self.os.window_is_vibrant();
        if transparent != self.transparent {
            self.transparent = transparent;
            self.os.set_transparent(transparent);
            self.invalidate_surface(false);
        }
        self.synchronize_editor(runtime);
        self.synchronize_cursor(runtime);
        if let Some(scene) = self.os.scene() {
            scene.sync_back(runtime.back_target());
        }
        self.synchronize_accessibility();
        self.request_frame();
    }

    pub fn set_accessibility_active(&mut self, runtime: &WindowRuntime, active: bool) {
        if active == self.accessibility_active {
            return;
        }
        runtime.set_accessibility_active(AccessibilityConsumer::Platform, active);
        self.accessibility_active = active;
        if active {
            self.synchronize_accessibility();
        } else if let Some(accessibility) = self.os.accessibility() {
            accessibility.clear();
        }
    }

    fn synchronize_accessibility(&mut self) {
        if self.accessibility_active
            && let Some(accessibility) = self.os.accessibility()
        {
            accessibility.synchronize(&self.publication);
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
        let cursor = resolve_cursor(runtime.tree(), hit);
        if cursor != self.cursor {
            self.os.set_cursor(cursor);
            self.cursor = cursor;
        }
    }

    pub fn draw(
        &mut self,
        runtime: &WindowRuntime,
        device: &Device,
        sample: Duration,
        now: Instant,
        from_clock: bool,
    ) -> Result<(), String> {
        if self.draw.is_some() || self.suspended || self.configure_needed() || self.redraw_at.is_some_and(|deadline| deadline > now) {
            return Ok(());
        }
        let externally_paced = self.externally_paced();
        if externally_paced && !from_clock {
            return Ok(());
        }
        let Some(surface) = self.surface_info.filter(|info| self.presentation.ready([info.width, info.height])) else { return Ok(()) };
        // Freeze the native deltas accumulated through this publication before admitting the draw.
        let Some(frame) = self.publication.latest().filter(|frame| self.synchronized == Some(frame.number)) else { return Ok(()) };
        let sample = if frame.scene.as_ref().is_some_and(|scene| scene.continuously_animated(frame.sampled_at.as_secs_f64())) {
            sample
        } else {
            frame.sampled_at
        };
        if !self.presentation.needs_draw(frame.number, sample) {
            return Ok(());
        }
        let transactional = self.pending_module_views.prepare(frame.number);
        let target = match self.os.presentation() {
            Some(presentation) => {
                presentation.set_transactional_presentation(frame.number, transactional);
                presentation.try_acquire_drawable(&self.surface, surface.color, from_clock)
            }
            None => surface_drawable(&self.surface, surface.color),
        };
        match target {
            DrawableAcquisition::Ready(target) => {
                let list = frame.scene.as_ref().map_or_else(|| frame.draw.clone(), |scene| scene.sample(sample.as_secs_f64()));
                let stamp = self.presentation.stamp(runtime.tree().id(), device.executor.generation().number(), frame.number, sample);
                match device.executor.draw(DrawWork { target, list, scale: self.viewport().scale, stamp, valid_until: None, report: None })
                {
                    Ok(draw) => {
                        self.draw = Some(draw);
                        return Ok(());
                    }
                    Err(GpuError::Busy) if !externally_paced => self.redraw_at = Some(now + Duration::from_millis(8)),
                    Err(GpuError::Busy) => {}
                    Err(_) if device.executor.generation().is_lost() => self.invalidate_surface(true),
                    Err(error) => return Err(error.to_string()),
                }
            }
            retry @ (DrawableAcquisition::WaitingForClock | DrawableAcquisition::Retry) => {
                self.redraw_at = acquisition_retry_deadline(&retry, now, externally_paced);
            }
            DrawableAcquisition::Reconfigure => self.invalidate_surface(false),
            DrawableAcquisition::Lost => self.invalidate_surface(true),
            DrawableAcquisition::Suspended => {}
        }
        self.abandon_native_view_frame();
        Ok(())
    }

    pub fn finish_draw(&mut self, runtime: &WindowRuntime, device: &Device, reduced_motion: bool) -> Result<bool, String> {
        let Some(result) = self.draw.as_mut().and_then(GpuTask::try_take) else { return Ok(false) };
        self.draw = None;
        let generation = device.executor.generation();
        if generation.is_lost() {
            self.invalidate_surface(true);
            return Ok(false);
        }
        let frame = result.map_err(|error| error.to_string())?;
        if !self.presentation.accepts(frame.stamp, runtime.tree().id(), generation.number(), self.size()) {
            self.abandon_native_view_frame();
            self.request_frame();
            return Ok(false);
        }
        let suboptimal = frame.target.suboptimal();
        let progress = frame.progress();
        self.os.pre_present_notify();
        let mut commit_native_views = || {
            self.pending_module_views.commit(frame.stamp.native_rev, |id, change, retired| {
                if let Some(view) = retired {
                    if let Some(placement) = change.placement {
                        view.place(&placement);
                    }
                    if let Some(hidden) = change.snapshot_hidden {
                        view.set_occluded_by_snapshot(hidden);
                    }
                } else {
                    if let Some(placement) = change.placement {
                        self.native_mounts.place(&placement);
                    }
                    if let Some(hidden) = change.snapshot_hidden {
                        self.native_mounts.set_occluded_by_snapshot(id, hidden);
                        if !hidden {
                            self.publication.did_restore_native_snapshot(id);
                        }
                    }
                }
            });
        };
        match self.os.presentation() {
            Some(presentation) => {
                presentation.present_with_native_views(frame.target, &device.queue, frame.stamp.native_rev, &mut commit_native_views)?
            }
            None => {
                frame.target.present(&device.queue)?;
                commit_native_views();
            }
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
            self.os.show();
            if std::mem::take(&mut self.initial_fullscreen) {
                self.os.set_window_geometry(WindowGeometryRequest { fullscreen: Some(true), ..Default::default() });
            }
        }
        self.capture_native_airspace();
        if suboptimal {
            self.invalidate_surface(false);
        }
        if let Some(latest) = self.publication.latest() {
            if !self.presentation.presented_revision(latest.number) {
                self.request_frame();
            } else {
                let motion = latest.scene.as_ref().is_some_and(|scene| scene.continuously_animated(latest.sampled_at.as_secs_f64()));
                let activity = match (motion, reduced_motion) {
                    (true, true) => FrameClockActivity::Low,
                    (true, false) => FrameClockActivity::Interactive,
                    _ => FrameClockActivity::Idle,
                };
                self.set_frame_clock_activity(activity);
            }
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

    pub fn retire(&mut self) {
        self.set_suspended(true);
        self.os.close();
        for (_, cancel) in self.service_cleanup.drain() {
            cancel();
        }
        self.native_mounts.clear();
    }

    pub fn deadline(&self) -> Option<Instant> {
        self.redraw_at
    }
}

impl Drop for HostWindow {
    fn drop(&mut self) {
        self.draw = None;
        self.set_frame_clock_activity(FrameClockActivity::Paused);
        for (_, cancel) in self.service_cleanup.drain() {
            cancel();
        }
    }
}

fn acquisition_retry_deadline(acquisition: &DrawableAcquisition, now: Instant, externally_paced: bool) -> Option<Instant> {
    (!externally_paced && matches!(acquisition, DrawableAcquisition::Retry)).then(|| now + Duration::from_millis(16))
}

pub(crate) fn transparent_background(theme: &Theme) -> bool {
    theme.get(ColorToken::Background).components[3] < 1.0
}

fn preferred_alpha_mode(transparent: bool, modes: &[wgpu::CompositeAlphaMode]) -> Option<wgpu::CompositeAlphaMode> {
    use wgpu::CompositeAlphaMode::*;
    let preference = if transparent { &[PreMultiplied, PostMultiplied][..] } else { &[Opaque] };
    preference.iter().copied().find(|mode| modes.contains(mode))
}

pub(crate) fn create_surface(instance: &wgpu::Instance, window: &dyn OsWindow) -> Result<Arc<wgpu::Surface<'static>>, String> {
    let (raw_display_handle, raw_window_handle) = window.raw_handles()?;
    // SAFETY: The OsWindow contract guarantees valid handles. The shell keeps the owner
    // alive until the surface and all GPU jobs referencing it are gone, even on close.
    let surface = unsafe {
        instance
            .create_surface_unsafe(wgpu::SurfaceTargetUnsafe::RawHandle { raw_display_handle: Some(raw_display_handle), raw_window_handle })
    };
    surface.map(Arc::new).map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use octara_model::identity::TreeId;
    use octara_model::theme::{Color, Theme};

    #[test]
    fn overlapping_scroll_and_hover_keep_presenting_on_60_and_120_hz_ticks() {
        let size = [160, 120];
        let tree = TreeId::new([1, 2], 3);
        for hz in [60, 120] {
            let mut presentation = Presentation::default();
            presentation.configure(size);
            let start = Instant::now();
            let interval = Duration::from_secs(1) / hz;
            let mut latest = 1;
            for tick in 0..hz {
                let sample = interval * tick;
                assert!(presentation.needs_draw(latest, sample));
                let stamp = presentation.stamp(tree, 1, latest, sample);
                // Both inputs publish while this tick's GPU work is still in flight.
                let scroll = latest + 1;
                latest = scroll + 1; // Hover
                assert!(presentation.accepts(stamp, tree, 1, size));
                presentation.did_present(stamp);
                assert!(presentation.presented_revision(stamp.native_rev));
                assert!(!presentation.presented_revision(latest));

                let between_ticks = start + sample + interval / 2;
                assert_eq!(acquisition_retry_deadline(&DrawableAcquisition::WaitingForClock, between_ticks, true), None);
                assert_eq!(acquisition_retry_deadline(&DrawableAcquisition::Retry, between_ticks, true), None);
            }
            let final_frame = presentation.stamp(tree, 1, latest, interval * hz);
            assert!(presentation.accepts(final_frame, tree, 1, size));
            assert_eq!(final_frame.frame, u64::from(hz) + 1);
        }
    }

    #[test]
    fn only_unpaced_drawable_retry_sets_a_deadline() {
        let now = Instant::now();
        assert_eq!(acquisition_retry_deadline(&DrawableAcquisition::Retry, now, false), Some(now + Duration::from_millis(16)));
        assert_eq!(acquisition_retry_deadline(&DrawableAcquisition::WaitingForClock, now, false), None);
    }

    #[test]
    fn background_and_surface_alpha_are_opt_in() {
        assert!(!transparent_background(&Theme::default()));
        for alpha in [0, 128] {
            let background = Color::from_rgba8(10, 20, 30, alpha);
            assert!(transparent_background(&Theme::default().color(ColorToken::Background, background)));
        }

        use wgpu::CompositeAlphaMode::*;
        let supported = [PostMultiplied, Opaque, PreMultiplied];
        assert_eq!(preferred_alpha_mode(false, &supported), Some(Opaque));
        assert_eq!(preferred_alpha_mode(true, &supported), Some(PreMultiplied));
        assert_eq!(preferred_alpha_mode(true, &[Opaque, PostMultiplied]), Some(PostMultiplied));
        assert_eq!(preferred_alpha_mode(true, &[Opaque]), None);
    }
}
