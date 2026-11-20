use super::CanvasPaint;
use rustc_hash::{FxHashMap, FxHashSet};
use std::any::Any;
use std::collections::VecDeque;
use std::sync::Weak;

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub(super) enum OutputSlot {
    Backdrop(usize),
    Canvas(usize),
}

pub(super) struct CanvasKey {
    program: Weak<dyn Any + Send + Sync>,
    inputs: CanvasInputs,
}

#[derive(PartialEq, Eq)]
struct CanvasInputs {
    // The history already belongs to one device and all author outputs use Rgba8Unorm.
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

pub(super) struct SubmittedOutput {
    pub(super) view: wgpu::TextureView,
    pub(super) revision: Option<u64>,
    canvas: Option<CanvasKey>,
}

struct OutputEntry {
    mount: Weak<()>,
    submitted: SubmittedOutput,
    /// An earlier output that a later submission can overwrite in queue order.
    working: Option<wgpu::TextureView>,
}

impl OutputEntry {
    fn bytes(&self) -> u64 {
        texture_bytes(&self.submitted.view) + self.working.as_ref().map_or(0, texture_bytes)
    }
}

/// Submitted outputs and their independent working allocations for one renderer device.
#[derive(Default)]
pub(super) struct OutputHistory {
    entries: FxHashMap<OutputSlot, OutputEntry>,
    canvases: VecDeque<OutputSlot>,
    canvas_bytes: u64,
}

/// Changes owned by the recording until the queue accepts it. Captures have no changes.
#[derive(Default)]
pub(super) struct OutputChanges {
    fresh: Vec<OutputChange>,
    written: FxHashSet<OutputSlot>,
    reused: Vec<OutputSlot>,
}

struct OutputChange {
    slot: OutputSlot,
    mount: Weak<()>,
    view: wgpu::TextureView,
    canvas: Option<CanvasKey>,
}

impl OutputChanges {
    pub(super) fn has_fresh(&self) -> bool {
        !self.fresh.is_empty()
    }

    pub(super) fn record(&mut self, mount: Weak<()>, view: wgpu::TextureView, canvas: Option<CanvasKey>) {
        let address = mount.as_ptr().addr();
        let slot = if canvas.is_some() { OutputSlot::Canvas(address) } else { OutputSlot::Backdrop(address) };
        self.written.insert(slot);
        self.fresh.push(OutputChange { slot, mount, view, canvas });
    }

    pub(super) fn reuse_canvas(&mut self, slot: OutputSlot) {
        self.reused.push(slot);
    }
}

impl OutputHistory {
    const CANVAS_BYTES: u64 = 32 * 1024 * 1024;
    const CANVAS_SLOTS: usize = 64;

    pub(super) fn can_retain(size: [u32; 2]) -> bool {
        rgba_bytes(size) <= Self::CANVAS_BYTES
    }

    pub(super) fn prune(&mut self) {
        for (slot, entry) in self.entries.extract_if(|_, entry| entry.mount.strong_count() == 0) {
            if matches!(slot, OutputSlot::Canvas(_)) {
                self.canvases.retain(|cached| *cached != slot);
                self.canvas_bytes -= entry.bytes();
            }
        }
    }

    pub(super) fn submitted(&self, slot: OutputSlot) -> Option<&SubmittedOutput> {
        self.entries.get(&slot).map(|entry| &entry.submitted)
    }

    pub(super) fn canvas(&self, slot: OutputSlot, key: &CanvasKey) -> Option<&SubmittedOutput> {
        self.submitted(slot).filter(|image| image.canvas.as_ref().is_some_and(|submitted| submitted.matches(key)))
    }

    pub(super) fn working(&self, slot: OutputSlot, changes: Option<&OutputChanges>) -> Option<&wgpu::Texture> {
        // Standalone canvases can all run before either output is composited. A second draw for
        // the same mount needs a separate allocation while the first output is referenced.
        if changes?.written.contains(&slot) {
            return None;
        }
        self.entries.get(&slot)?.working.as_ref().map(wgpu::TextureView::texture)
    }

    pub(super) fn commit(&mut self, changes: OutputChanges, revision: Option<u64>) {
        for slot in changes.reused {
            if let Some(index) = self.canvases.iter().position(|cached| *cached == slot) {
                self.canvases.remove(index);
                self.canvases.push_back(slot);
            }
        }
        for OutputChange { slot, mount, view, canvas } in changes.fresh {
            let previous = self.entries.remove(&slot);
            let submitted = SubmittedOutput { view, revision, canvas };
            let mut working = previous.as_ref().map(|entry| entry.submitted.view.clone());
            if matches!(slot, OutputSlot::Canvas(_)) {
                let bytes = texture_bytes(&submitted.view);
                if let Some(previous) = previous {
                    self.canvas_bytes -= previous.bytes();
                    self.canvases.retain(|cached| *cached != slot);
                }
                // Keep the optional working allocation when it can serve another draw at
                // the current extent without exceeding this mount's share of the cache.
                working = working.filter(|view| {
                    view.texture().size() == submitted.view.texture().size() && bytes + texture_bytes(view) <= Self::CANVAS_BYTES
                });
                self.canvas_bytes += bytes + working.as_ref().map_or(0, texture_bytes);
                self.canvases.push_back(slot);
            }
            self.entries.insert(slot, OutputEntry { mount, submitted, working });
            while self.canvas_bytes > Self::CANVAS_BYTES || self.canvases.len() > Self::CANVAS_SLOTS {
                let oldest = self.canvases.pop_front().expect("a canvas over budget");
                self.canvas_bytes -= self.entries.remove(&oldest).expect("a cached canvas").bytes();
            }
        }
    }

    #[cfg(test)]
    pub(super) fn is_empty(&self) -> bool {
        self.entries.is_empty()
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
    use std::sync::Arc;

    #[test]
    fn canvas_cache_evicts_the_least_recent_optional_output() {
        assert!(OutputHistory::can_retain([2048, 4096]));
        assert!(!OutputHistory::can_retain([2048, 4097]));
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
        let mounts: Vec<_> = (0..OutputHistory::CANVAS_SLOTS).map(|_| Arc::new(())).collect();
        let backdrop = Arc::new(());
        let mut changes = OutputChanges::default();
        changes.record(Arc::downgrade(&backdrop), view.clone(), None);
        for mount in &mounts {
            changes.record(Arc::downgrade(mount), view.clone(), Some(key()));
        }
        let mut history = OutputHistory::default();
        history.commit(changes, Some(1));
        let slot = |mount: &Arc<()>| OutputSlot::Canvas(Arc::as_ptr(mount).addr());
        let newest = Arc::new(());
        let mut changes = OutputChanges::default();
        changes.reuse_canvas(slot(&mounts[0]));
        changes.record(Arc::downgrade(&newest), view, Some(key()));
        history.commit(changes, Some(2));
        assert_eq!(history.canvas(slot(&mounts[0]), &key()).unwrap().revision, Some(1));
        assert!(history.canvas(slot(&mounts[1]), &key()).is_none());
        assert_eq!(history.canvas(slot(&newest), &key()).unwrap().revision, Some(2));
        assert!(history.submitted(OutputSlot::Backdrop(Arc::as_ptr(&backdrop).addr())).is_some());
        assert_eq!(history.canvases.len(), OutputHistory::CANVAS_SLOTS);
        assert_eq!(history.canvas_bytes, OutputHistory::CANVAS_SLOTS as u64 * 4);
        drop(mounts);
        drop(newest);
        history.prune();
        assert_eq!(history.entries.len(), 1);
        assert_eq!(history.canvas_bytes, 0);
    }
}
