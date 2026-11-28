use super::custom::TransitionImage;
use crate::plan::CanvasPaint;
use crate::plan::paint::CANVAS_BYTES;
pub(super) use crate::plan::paint::rgba_bytes;
use octara_model::CacheLine;
use octara_model::capture::PaintSample;
use rustc_hash::{FxHashMap, FxHashSet};
use std::any::Any;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Weak};

pub(super) struct CanvasKey {
    program: Weak<dyn Any + Send + Sync>,
    data: Option<Weak<dyn Any + Send + Sync>>,
    inputs: CanvasInputs,
}

#[derive(PartialEq, Eq)]
struct CanvasInputs {
    // The cache belongs to one device and all author outputs use the working color format.
    params: [u32; 4],
    logical_size: [u32; 2],
    radius: [u32; 4],
    extent: [u32; 2],
    scale: u32,
}

impl CanvasKey {
    pub(super) fn new(paint: &CanvasPaint, scale: f32) -> Self {
        Self {
            program: paint.program.downgrade(),
            data: paint.input.downgrade_data(),
            inputs: CanvasInputs {
                params: paint.input.params.map(f32::to_bits),
                logical_size: [paint.rect.w.to_bits(), paint.rect.h.to_bits()],
                radius: paint.radius.map(f32::to_bits),
                extent: paint.size,
                scale: scale.to_bits(),
            },
        }
    }

    fn matches(&self, other: &Self) -> bool {
        self.program.ptr_eq(&other.program)
            && self.inputs == other.inputs
            && match (&self.data, &other.data) {
                (None, None) => true,
                (Some(a), Some(b)) => a.ptr_eq(b),
                _ => false,
            }
    }
}

pub(super) struct OutputImage {
    pub(super) view: wgpu::TextureView,
    pub(super) revision: Option<u64>,
}

struct CanvasEntry {
    mount: Weak<()>,
    submitted: OutputImage,
    key: CanvasKey,
    /// An earlier output that a later submission can overwrite in queue order.
    working: Option<wgpu::TextureView>,
}

impl CanvasEntry {
    fn bytes(&self) -> u64 {
        texture_bytes(&self.submitted.view) + self.working.as_ref().map_or(0, texture_bytes)
    }
}

/// Compatible retained-canvas output becomes reusable on submission.
#[derive(Default)]
pub(super) struct CanvasCache {
    entries: FxHashMap<usize, CanvasEntry>,
    lru: VecDeque<usize>,
    bytes: u64,
}

#[derive(Default)]
pub(super) struct OutputChanges {
    pub(super) canvases: CanvasChanges,
    pub(super) backdrops: Vec<BackdropChange>,
    pub(super) transitions: TransitionChanges,
}

#[derive(Default)]
pub(super) struct CanvasChanges {
    fresh: Vec<CanvasChange>,
    written: FxHashSet<usize>,
    reused: Vec<usize>,
}

struct CanvasChange {
    mount: Weak<()>,
    view: wgpu::TextureView,
    key: CanvasKey,
}

impl OutputChanges {
    pub(super) fn has_fresh(&self) -> bool {
        !self.canvases.fresh.is_empty() || !self.backdrops.is_empty() || self.transitions.region_count != 0
    }
}

impl CanvasChanges {
    pub(super) fn record(&mut self, mount: Weak<()>, view: wgpu::TextureView, key: CanvasKey) {
        self.written.insert(mount.as_ptr().addr());
        self.fresh.push(CanvasChange { mount, view, key });
    }

    pub(super) fn reuse(&mut self, slot: usize) {
        self.reused.push(slot);
    }
}

impl CanvasCache {
    pub(super) const CANVAS_SLOTS: usize = 64;

    pub(super) fn prune(&mut self) {
        for (slot, entry) in self.entries.extract_if(|_, entry| entry.mount.strong_count() == 0) {
            self.lru.retain(|cached| *cached != slot);
            self.bytes -= entry.bytes();
        }
    }

    pub(super) fn get(&self, slot: usize, key: &CanvasKey) -> Option<&OutputImage> {
        let entry = self.entries.get(&slot)?;
        entry.key.matches(key).then_some(&entry.submitted)
    }

    pub(super) fn working(&self, slot: usize, changes: Option<&CanvasChanges>) -> Option<&wgpu::Texture> {
        // Standalone canvases can all run before either output is composited. A second draw for
        // the same mount needs a separate allocation while the first output is referenced.
        if changes?.written.contains(&slot) {
            return None;
        }
        self.entries.get(&slot)?.working.as_ref().map(wgpu::TextureView::texture)
    }

    pub(super) fn submit(&mut self, changes: CanvasChanges, revision: Option<u64>) {
        for slot in changes.reused {
            if let Some(index) = self.lru.iter().position(|cached| *cached == slot) {
                self.lru.remove(index);
                self.lru.push_back(slot);
            }
        }
        for CanvasChange { mount, view, key } in changes.fresh {
            let slot = mount.as_ptr().addr();
            let bytes = texture_bytes(&view);
            let working = self.entries.remove(&slot).map(|previous| {
                self.bytes -= previous.bytes();
                self.lru.retain(|cached| *cached != slot);
                previous.submitted.view
            });
            // Keep the optional working allocation when it can serve another draw at
            // the current extent without exceeding this mount's share of the cache.
            let working = working
                .filter(|previous| previous.texture().size() == view.texture().size() && bytes + texture_bytes(previous) <= CANVAS_BYTES);
            self.bytes += bytes + working.as_ref().map_or(0, texture_bytes);
            self.lru.push_back(slot);
            self.entries.insert(slot, CanvasEntry { mount, submitted: OutputImage { view, revision }, key, working });
            while self.bytes > CANVAS_BYTES || self.lru.len() > Self::CANVAS_SLOTS {
                let oldest = self.lru.pop_front().expect("a canvas over budget");
                self.bytes -= self.entries.remove(&oldest).expect("a cached canvas").bytes();
            }
        }
    }
}

/// Local paint outputs from a submitted frame, awaiting the host's acceptance.
///
/// Dropping the token discards its outputs. Use
/// [`WgpuRenderer::accept_presentation`](super::WgpuRenderer::accept_presentation) to retain them
/// for exits and transitions after the host accepts the frame.
#[must_use = "accept the frame to publish its paint history"]
pub struct PendingPresentation(Option<PendingOutputs>);

struct PendingOutputs {
    renderer: Arc<CacheLine<AtomicU64>>,
    submission: u64,
    backdrops: Vec<BackdropChange>,
    transitions: Option<Box<TransitionChanges>>,
    sample: Option<PaintSample>,
}

pub(super) struct BackdropChange {
    pub(super) mount: Weak<()>,
    pub(super) view: wgpu::TextureView,
}

struct BackdropEntry {
    mount: Weak<()>,
    accepted: OutputImage,
    working: Option<wgpu::TextureView>,
}

/// History for exiting backdrops and local transitions comes from frames the host accepted.
#[derive(Default)]
pub(super) struct PresentationHistory {
    renderer: Arc<CacheLine<AtomicU64>>,
    backdrops: FxHashMap<usize, BackdropEntry>,
    transitions: TransitionHistory,
}

impl PresentationHistory {
    pub(super) fn prune(&mut self) {
        self.backdrops.retain(|_, entry| entry.mount.strong_count() != 0);
        self.transitions.prune();
    }

    pub(super) fn get(&self, mount: &Weak<()>) -> Option<&OutputImage> {
        self.backdrops.get(&mount.as_ptr().addr()).map(|entry| &entry.accepted)
    }

    pub(super) fn take_working(&mut self, mount: &Weak<()>) -> Option<wgpu::TextureView> {
        // A frame can remain pending while another records. Give the recording exclusive use
        // of this allocation until it becomes accepted or is discarded.
        self.backdrops.get_mut(&mount.as_ptr().addr())?.working.take()
    }

    pub(super) fn transition(
        &self,
        mount: &Weak<()>,
        run: &Weak<()>,
        revision: u64,
        changes: Option<&mut TransitionChanges>,
    ) -> Option<(RegionImage, PaintSample)> {
        self.transitions.old(mount, run, revision, changes)
    }

    pub(super) fn pending(
        &self,
        backdrops: Vec<BackdropChange>,
        transitions: TransitionChanges,
        sample: Option<PaintSample>,
    ) -> PendingPresentation {
        let transitions = (!transitions.regions.is_empty()).then(|| Box::new(transitions));
        let changed = !backdrops.is_empty() || transitions.is_some();
        PendingPresentation(changed.then(|| {
            let submission = transitions.as_ref().map_or(0, |_| {
                self.renderer
                    .try_update(Ordering::Relaxed, Ordering::Relaxed, |id| id.checked_add(1))
                    .expect("presentation submission sequence exhausted")
                    + 1
            });
            PendingOutputs { renderer: self.renderer.clone(), submission, backdrops, transitions, sample }
        }))
    }

    pub(super) fn accept(&mut self, pending: PendingPresentation) {
        let Some(PendingOutputs { renderer, submission, backdrops, transitions, sample }) = pending.0 else { return };
        assert!(Arc::ptr_eq(&self.renderer, &renderer), "presentation belongs to another GPU renderer");
        for BackdropChange { mount, view } in backdrops {
            if mount.strong_count() == 0 {
                continue;
            }
            let slot = mount.as_ptr().addr();
            // Earlier readers have submitted their commands, so later frames can reuse the
            // previous image's allocation in queue order.
            let working = self.backdrops.remove(&slot).map(|previous| previous.accepted.view);
            let accepted = OutputImage { view, revision: sample.map(|sample| sample.revision) };
            self.backdrops.insert(slot, BackdropEntry { mount, accepted, working });
        }
        if let Some(transitions) = transitions {
            self.transitions.accept(*transitions, sample, submission);
        }
    }
}

/// A local image can stay referenced by accepted history, multiple transition runs, and
/// submitted command buffers. Its texture gets no additional writes after recording.
#[derive(Clone)]
pub(super) struct RegionImage {
    pub(super) view: wgpu::TextureView,
    pub(super) origin: [i32; 2],
    pub(super) scale: f32,
}

impl RegionImage {
    pub(super) fn new(view: &wgpu::TextureView, origin: [i32; 2], scale: f32) -> Self {
        let view = view
            .texture()
            .create_view(&wgpu::TextureViewDescriptor { usage: Some(wgpu::TextureUsages::TEXTURE_BINDING), ..Default::default() });
        Self { view, origin, scale }
    }

    pub(super) fn size(&self) -> [u32; 2] {
        let texture = self.view.texture();
        [texture.width(), texture.height()]
    }

    pub(super) fn input(&self) -> TransitionImage<'_> {
        TransitionImage { view: &self.view, origin: self.origin, size: self.size(), scale: self.scale }
    }

    pub(super) fn can_retain(size: [u32; 2]) -> bool {
        rgba_bytes(size) <= TransitionHistory::BYTES
    }

    fn bytes(&self) -> u64 {
        texture_bytes(&self.view)
    }
}

#[derive(Default)]
pub(super) struct TransitionChanges {
    regions: Vec<RegionChange>,
    region_count: usize,
    region_bytes: u64,
    pins: FxHashMap<usize, TransitionPin>,
    pin_bytes: u64,
}

struct RegionChange {
    mount: Weak<()>,
    revision: u64,
    image: Option<RegionImage>,
}

impl TransitionChanges {
    pub(super) fn record(&mut self, mount: Weak<()>, revision: u64, image: Option<RegionImage>) {
        let image = image.filter(|image| {
            let bytes = image.bytes();
            if !TransitionHistory::fits(self.region_count, self.region_bytes, bytes) {
                return false;
            }
            self.region_count += 1;
            self.region_bytes += bytes;
            true
        });
        self.regions.push(RegionChange { mount, revision, image });
    }
}

struct AcceptedRegion {
    mount: Weak<()>,
    revision: u64,
    submission: u64,
    pixels: Option<RegionPixels>,
}

struct RegionPixels {
    image: RegionImage,
    sample: PaintSample,
}

struct TransitionPin {
    run: Weak<()>,
    mount: Weak<()>,
    revision: u64,
    image: RegionImage,
    sample: PaintSample,
}

impl TransitionPin {
    fn input(&self, mount: &Weak<()>, revision: u64) -> Option<(RegionImage, PaintSample)> {
        (self.mount.ptr_eq(mount) && self.revision == revision).then(|| (self.image.clone(), self.sample))
    }
}

#[derive(Default)]
struct TransitionHistory {
    // Keep the accepted order of each live mount even when its pixels aren't reusable.
    regions: FxHashMap<usize, AcceptedRegion>,
    recent: VecDeque<usize>,
    region_bytes: u64,
    pins: FxHashMap<usize, TransitionPin>,
    pin_bytes: u64,
}

impl TransitionHistory {
    // Give previous images and active runs separate allowances so a fresh image cannot evict
    // the old endpoint of a run that is still in use.
    const BYTES: u64 = 32 * 1024 * 1024;
    const SLOTS: usize = 64;

    fn fits(count: usize, already_used: u64, incoming: u64) -> bool {
        count < Self::SLOTS && already_used.saturating_add(incoming) <= Self::BYTES
    }

    fn old(
        &self,
        mount: &Weak<()>,
        run: &Weak<()>,
        revision: u64,
        changes: Option<&mut TransitionChanges>,
    ) -> Option<(RegionImage, PaintSample)> {
        let key = run.as_ptr().addr();
        if let Some(pin) = self.pins.get(&key) {
            return pin.input(mount, revision);
        }
        // Captures can read an established pin, including after a subsequent cut. Only a
        // presentation can establish a pin from the last accepted image of this region.
        let changes = changes?;
        if let Some(pin) = changes.pins.get(&key) {
            return pin.input(mount, revision);
        }
        let previous = self.regions.get(&mount.as_ptr().addr()).filter(|previous| previous.revision < revision)?.pixels.as_ref()?;
        let bytes = previous.image.bytes();
        if !Self::fits(self.pins.len() + changes.pins.len(), self.pin_bytes + changes.pin_bytes, bytes) {
            return None;
        }
        let image = previous.image.clone();
        let sample = previous.sample;
        changes.pins.insert(key, TransitionPin { run: run.clone(), mount: mount.clone(), revision, image: image.clone(), sample });
        changes.pin_bytes += bytes;
        Some((image, sample))
    }

    fn prune(&mut self) {
        for (slot, region) in self.regions.extract_if(|_, region| region.mount.strong_count() == 0) {
            if let Some(pixels) = region.pixels {
                self.recent.retain(|cached| *cached != slot);
                self.region_bytes -= pixels.image.bytes();
            }
        }
        for (_, pin) in self.pins.extract_if(|_, pin| pin.run.strong_count() == 0 || pin.mount.strong_count() == 0) {
            self.pin_bytes -= pin.image.bytes();
        }
    }

    fn accept(&mut self, changes: TransitionChanges, sample: Option<PaintSample>, submission: u64) {
        for RegionChange { mount, revision, image } in changes.regions {
            let key = mount.as_ptr().addr();
            if mount.strong_count() == 0
                || self.regions.get(&key).is_some_and(|region| region.submission > submission || region.revision > revision)
            {
                continue;
            }
            if let Some(pixels) = self.regions.remove(&key).and_then(|old| old.pixels) {
                self.region_bytes -= pixels.image.bytes();
                self.recent.retain(|cached| *cached != key);
            }
            let pixels = image.zip(sample).map(|(image, sample)| {
                let bytes = image.bytes();
                while !Self::fits(self.recent.len(), self.region_bytes, bytes) {
                    let oldest = self.recent.pop_front().expect("a transition region over budget");
                    let evicted = self.regions.get_mut(&oldest).and_then(|region| region.pixels.take()).expect("cached transition pixels");
                    self.region_bytes -= evicted.image.bytes();
                }
                self.region_bytes += bytes;
                self.recent.push_back(key);
                RegionPixels { image, sample }
            });
            self.regions.insert(key, AcceptedRegion { mount, revision, submission, pixels });
        }
        for (key, pin) in changes.pins {
            let bytes = pin.image.bytes();
            if !self.pins.contains_key(&key)
                && pin.run.strong_count() != 0
                && self.regions.get(&pin.mount.as_ptr().addr()).is_some_and(|region| region.submission == submission)
                && Self::fits(self.pins.len(), self.pin_bytes, bytes)
            {
                self.pin_bytes += bytes;
                self.pins.insert(key, pin);
            }
        }
    }
}

fn texture_bytes(view: &wgpu::TextureView) -> u64 {
    let texture = view.texture();
    rgba_bytes([texture.width(), texture.height()])
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;
    use crate::WgpuRenderer;
    use std::time::Duration;

    impl CanvasCache {
        pub(in crate::gpu) fn is_empty(&self) -> bool {
            self.entries.is_empty()
        }
    }

    impl PresentationHistory {
        pub(in crate::gpu) fn is_empty(&self) -> bool {
            self.backdrops.is_empty()
        }
    }

    fn test_view(gpu: &WgpuRenderer) -> wgpu::TextureView {
        gpu.device()
            .create_texture(&wgpu::TextureDescriptor {
                label: None,
                size: wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: super::super::WORKING_FORMAT,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            })
            .create_view(&Default::default())
    }

    fn sample(revision: u64) -> Option<PaintSample> {
        Some(PaintSample { revision, sampled_at: Duration::from_millis(revision) })
    }

    #[test]
    fn canvas_cache_evicts_the_least_recent_optional_output() {
        assert!(crate::plan::paint::can_retain_canvas([2048, 2048]));
        assert!(!crate::plan::paint::can_retain_canvas([2048, 2049]));
        let Some(gpu) = WgpuRenderer::new() else {
            eprintln!("no GPU adapter: skipped");
            return;
        };
        let view = test_view(&gpu);
        let program = octara_model::paint::PaintProgram::new(());
        let key = || CanvasKey {
            program: program.downgrade(),
            data: None,
            inputs: CanvasInputs { params: [0; 4], logical_size: [0; 2], radius: [0; 4], extent: [1, 1], scale: 0 },
        };
        let mounts: Vec<_> = (0..CanvasCache::CANVAS_SLOTS).map(|_| Arc::new(())).collect();
        let backdrop = Arc::new(());
        let backdrop_mount = Arc::downgrade(&backdrop);
        let mut history = PresentationHistory::default();
        let pending = history.pending(
            vec![BackdropChange { mount: backdrop_mount.clone(), view: view.clone() }],
            TransitionChanges::default(),
            sample(1),
        );
        history.accept(pending);
        let mut changes = CanvasChanges::default();
        for mount in &mounts {
            changes.record(Arc::downgrade(mount), view.clone(), key());
        }
        let mut cache = CanvasCache::default();
        cache.submit(changes, Some(1));
        let slot = |mount: &Arc<()>| Arc::as_ptr(mount).addr();
        let newest = Arc::new(());
        let mut changes = CanvasChanges::default();
        changes.reuse(slot(&mounts[0]));
        changes.record(Arc::downgrade(&newest), view, key());
        cache.submit(changes, Some(2));
        assert_eq!(cache.get(slot(&mounts[0]), &key()).unwrap().revision, Some(1));
        assert!(cache.get(slot(&mounts[1]), &key()).is_none());
        assert_eq!(cache.get(slot(&newest), &key()).unwrap().revision, Some(2));
        assert!(history.get(&backdrop_mount).is_some());
        assert_eq!(cache.lru.len(), CanvasCache::CANVAS_SLOTS);
        assert_eq!(cache.bytes, CanvasCache::CANVAS_SLOTS as u64 * 8);
        drop(mounts);
        drop(newest);
        cache.prune();
        assert!(cache.is_empty());
        assert_eq!(cache.bytes, 0);
    }

    #[test]
    fn transition_runs_pin_only_accepted_history_at_their_cut() {
        assert!(RegionImage::can_retain([2048, 2048]));
        assert!(!RegionImage::can_retain([2048, 2049]));
        let Some(gpu) = WgpuRenderer::new() else {
            eprintln!("no GPU adapter: skipped");
            return;
        };
        let mount = Arc::new(());
        let independent_mount = Arc::new(());
        let first_run = Arc::new(());
        let next_run = Arc::new(());
        let independent_run_owner = Arc::new(());
        let region = Arc::downgrade(&mount);
        let independent = Arc::downgrade(&independent_mount);
        let first = Arc::downgrade(&first_run);
        let next = Arc::downgrade(&next_run);
        let mut history = PresentationHistory::default();
        let image = |x| RegionImage::new(&test_view(&gpu), [x, 0], 1.0);
        let old = |output: Option<(RegionImage, PaintSample)>| output.map(|(image, sample)| (image.origin[0], sample.revision));

        let mut initial = TransitionChanges::default();
        initial.record(region.clone(), 0, Some(image(0)));
        let pending = history.pending(Vec::new(), initial, sample(10));
        assert!(history.transition(&region, &first, 1, Some(&mut TransitionChanges::default())).is_none());
        history.accept(pending);

        let mut discarded = TransitionChanges::default();
        assert_eq!(old(history.transition(&region, &first, 1, Some(&mut discarded))), Some((0, 10)));
        discarded.record(region.clone(), 1, Some(image(1)));
        drop(history.pending(Vec::new(), discarded, sample(11)));
        assert!(history.transition(&region, &first, 1, None).is_none());

        let mut delayed = TransitionChanges::default();
        delayed.record(region.clone(), 1, Some(image(4)));
        delayed.record(independent.clone(), 0, Some(image(5)));
        let delayed = history.pending(Vec::new(), delayed, sample(11));
        let mut accepted = TransitionChanges::default();
        assert_eq!(old(history.transition(&region, &first, 1, Some(&mut accepted))), Some((0, 10)));
        accepted.record(region.clone(), 1, Some(image(2)));
        let pending = history.pending(Vec::new(), accepted, sample(12));
        history.accept(pending);
        history.accept(delayed);
        assert_eq!(old(history.transition(&region, &first, 1, None)), Some((0, 10)));
        assert!(history.transition(&region, &next, 2, None).is_none());
        let independent_run = Arc::downgrade(&independent_run_owner);
        assert_eq!(old(history.transition(&independent, &independent_run, 1, Some(&mut TransitionChanges::default()))), Some((5, 11)),);

        let mut delayed = TransitionChanges::default();
        delayed.record(region.clone(), 2, Some(image(6)));
        let delayed = history.pending(Vec::new(), delayed, sample(13));
        let mut interrupted = TransitionChanges::default();
        assert_eq!(old(history.transition(&region, &next, 2, Some(&mut interrupted))), Some((2, 12)));
        interrupted.record(region.clone(), 2, None);
        let pending = history.pending(Vec::new(), interrupted, sample(14));
        history.accept(pending);
        history.accept(delayed);
        assert_eq!(old(history.transition(&region, &first, 1, None)), Some((0, 10)));
        assert_eq!(old(history.transition(&region, &next, 2, None)), Some((2, 12)));
        assert!(history.transitions.regions[&region.as_ptr().addr()].pixels.is_none());

        drop(first_run);
        drop(next_run);
        history.prune();
        assert!(history.transitions.pins.is_empty());
        assert_eq!(history.transitions.pin_bytes, 0);
    }
}
