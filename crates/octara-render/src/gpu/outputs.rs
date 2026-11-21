use super::CanvasPaint;
use rustc_hash::{FxHashMap, FxHashSet};
use std::any::Any;
use std::collections::VecDeque;
use std::sync::{Arc, Weak};

pub(super) struct CanvasKey {
    program: Weak<dyn Any + Send + Sync>,
    inputs: CanvasInputs,
}

#[derive(PartialEq, Eq)]
struct CanvasInputs {
    // The cache belongs to one device and all author outputs use Rgba8Unorm.
    params: [u32; 4],
    logical_size: [u32; 2],
    radius: u32,
    extent: [u32; 2],
    scale: u32,
}

impl CanvasKey {
    pub(super) fn new(paint: &CanvasPaint, scale: f32) -> Self {
        Self {
            program: paint.program.downgrade(),
            inputs: CanvasInputs {
                params: paint.params.map(f32::to_bits),
                logical_size: [paint.rect.w.to_bits(), paint.rect.h.to_bits()],
                radius: paint.radius.to_bits(),
                extent: paint.size,
                scale: scale.to_bits(),
            },
        }
    }

    fn matches(&self, other: &Self) -> bool {
        self.program.ptr_eq(&other.program) && self.inputs == other.inputs
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
        !self.canvases.fresh.is_empty() || !self.backdrops.is_empty()
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
    const CANVAS_BYTES: u64 = 32 * 1024 * 1024;
    const CANVAS_SLOTS: usize = 64;

    pub(super) fn can_retain(size: [u32; 2]) -> bool {
        rgba_bytes(size) <= Self::CANVAS_BYTES
    }

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
            let working = working.filter(|previous| {
                previous.texture().size() == view.texture().size() && bytes + texture_bytes(previous) <= Self::CANVAS_BYTES
            });
            self.bytes += bytes + working.as_ref().map_or(0, texture_bytes);
            self.lru.push_back(slot);
            self.entries.insert(slot, CanvasEntry { mount, submitted: OutputImage { view, revision }, key, working });
            while self.bytes > Self::CANVAS_BYTES || self.lru.len() > Self::CANVAS_SLOTS {
                let oldest = self.lru.pop_front().expect("a canvas over budget");
                self.bytes -= self.entries.remove(&oldest).expect("a cached canvas").bytes();
            }
        }
    }
}

/// Backdrop outputs from a submitted frame, awaiting the host's acceptance.
///
/// Dropping the token discards its outputs. Use
/// [`GpuRenderer::accept_presentation`](super::GpuRenderer::accept_presentation) to retain them
/// for exit animations after the host accepts the frame.
#[must_use = "accept the frame to publish its backdrop history"]
pub struct PendingPresentation(Option<PendingBackdrops>);

struct PendingBackdrops {
    renderer: Arc<()>,
    changes: Vec<BackdropChange>,
    revision: Option<u64>,
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

/// History for exiting backdrops comes from frames the host accepted.
#[derive(Default)]
pub(super) struct BackdropHistory {
    renderer: Arc<()>,
    entries: FxHashMap<usize, BackdropEntry>,
}

impl BackdropHistory {
    pub(super) fn prune(&mut self) {
        self.entries.retain(|_, entry| entry.mount.strong_count() != 0);
    }

    pub(super) fn get(&self, mount: &Weak<()>) -> Option<&OutputImage> {
        self.entries.get(&mount.as_ptr().addr()).map(|entry| &entry.accepted)
    }

    pub(super) fn take_working(&mut self, mount: &Weak<()>) -> Option<wgpu::TextureView> {
        // A frame can remain pending while another records. Give the recording exclusive use
        // of this allocation until it becomes accepted or is discarded.
        self.entries.get_mut(&mount.as_ptr().addr())?.working.take()
    }

    pub(super) fn pending(&self, changes: Vec<BackdropChange>, revision: Option<u64>) -> PendingPresentation {
        PendingPresentation((!changes.is_empty()).then(|| PendingBackdrops { renderer: self.renderer.clone(), changes, revision }))
    }

    pub(super) fn accept(&mut self, pending: PendingPresentation) {
        let Some(PendingBackdrops { renderer, changes, revision }) = pending.0 else {
            return;
        };
        assert!(Arc::ptr_eq(&self.renderer, &renderer), "presentation belongs to another GPU renderer");
        for BackdropChange { mount, view } in changes {
            if mount.strong_count() == 0 {
                continue;
            }
            let slot = mount.as_ptr().addr();
            // Earlier readers have submitted their commands, so later frames can reuse the
            // previous image's allocation in queue order.
            let working = self.entries.remove(&slot).map(|previous| previous.accepted.view);
            self.entries.insert(slot, BackdropEntry { mount, accepted: OutputImage { view, revision }, working });
        }
    }
}

fn rgba_bytes([width, height]: [u32; 2]) -> u64 {
    (u64::from(width) * u64::from(height)).saturating_mul(4)
}

fn texture_bytes(view: &wgpu::TextureView) -> u64 {
    let texture = view.texture();
    rgba_bytes([texture.width(), texture.height()])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::GpuRenderer;

    impl CanvasCache {
        pub(in crate::gpu) fn is_empty(&self) -> bool {
            self.entries.is_empty()
        }
    }

    impl BackdropHistory {
        pub(in crate::gpu) fn is_empty(&self) -> bool {
            self.entries.is_empty()
        }
    }

    #[test]
    fn canvas_cache_evicts_the_least_recent_optional_output() {
        assert!(CanvasCache::can_retain([2048, 4096]));
        assert!(!CanvasCache::can_retain([2048, 4097]));
        let Some(gpu) = GpuRenderer::new() else {
            eprintln!("no GPU adapter: skipped");
            return;
        };
        let view = gpu
            .device()
            .create_texture(&wgpu::TextureDescriptor {
                label: None,
                size: wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: super::super::FORMAT,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            })
            .create_view(&Default::default());
        let program = octara_primitives::paint::PaintProgram::new(());
        let key = || CanvasKey {
            program: program.downgrade(),
            inputs: CanvasInputs { params: [0; 4], logical_size: [0; 2], radius: 0, extent: [1, 1], scale: 0 },
        };
        let mounts: Vec<_> = (0..CanvasCache::CANVAS_SLOTS).map(|_| Arc::new(())).collect();
        let backdrop = Arc::new(());
        let backdrop_mount = Arc::downgrade(&backdrop);
        let mut history = BackdropHistory::default();
        let pending = history.pending(vec![BackdropChange { mount: backdrop_mount.clone(), view: view.clone() }], Some(1));
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
        assert_eq!(cache.bytes, CanvasCache::CANVAS_SLOTS as u64 * 4);
        drop(mounts);
        drop(newest);
        cache.prune();
        assert!(cache.is_empty());
        assert_eq!(cache.bytes, 0);
    }
}
