//! Canvas preparation for a recording or ahead of the next drawable.

use super::GpuRenderer;
use super::batch::{Draw, NO_CLIP, Pass, quad};
use super::custom::{CanvasMode, CanvasPaint, CanvasTarget};
use super::outputs::{CanvasCache, CanvasChanges, CanvasKey, rgba_bytes};
use super::plan::CanvasRequest;
use super::recording::FrameRecording;
use crate::shape::ClipStack;
use octara_model::capture::PaintOutcome;
use octara_model::geometry::Rect;
use octara_model::paint::{DisplayList, Item};
use std::collections::VecDeque;
use std::sync::Weak;

/// The local portions of one published scene that may be prepared without a drawable.
#[doc(hidden)]
pub struct CanvasUpdates {
    scale: f32,
    pending: VecDeque<(Weak<()>, CanvasPaint)>,
}

impl CanvasUpdates {
    pub fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }
}

impl GpuRenderer {
    pub(super) async fn prepare_canvases(
        &mut self,
        passes: &mut [Pass],
        requests: Vec<CanvasRequest>,
        scale: f32,
        recording: &mut FrameRecording<'_>,
    ) -> Vec<wgpu::TextureView> {
        self.programs.prune_canvases();
        if requests.is_empty() {
            self.programs.finish_canvases(0);
            return Vec::new();
        }
        let mut slots = Vec::with_capacity(requests.len());
        let mut views = Vec::with_capacity(requests.len());
        let mut transient_views = 0;
        recording.before.reserve(requests.len() + 1);
        // Preparation follows display-list order, including canvases inside child layers.
        for (request_index, request) in requests.into_iter().enumerate() {
            let index = views.len();
            let retained = request.retained_mount.map(|mount| {
                let slot = mount.as_ptr().addr();
                (slot, mount, CanvasKey::new(&request.paint, scale))
            });
            if let Some((slot, _, key)) = &retained
                && let Some(image) = self.canvas_cache.get(*slot, key)
            {
                views.push(image.view.clone());
                slots.push(Some(index));
                recording.receipts[request.receipt].outcome = PaintOutcome::Retained { revision: image.revision };
                if let Some(changes) = &mut recording.outputs {
                    changes.canvases.reuse(*slot);
                }
                if let Some(measurement) = recording.measurement.as_deref_mut() {
                    measurement.profile.retained_canvases += 1;
                }
                continue;
            }
            let target = match &retained {
                Some((slot, _, _)) => {
                    CanvasTarget::Working(self.canvas_cache.working(*slot, recording.outputs.as_ref().map(|changes| &changes.canvases)))
                }
                None => CanvasTarget::Transient(transient_views),
            };
            let product =
                self.programs.encode_canvas(&self.device, &request.paint, scale, target, recording.measurement.as_deref_mut()).await;
            let outcome = match product {
                Ok((view, command)) => {
                    if let Some((_, mount, key)) = retained {
                        if let Some(changes) = &mut recording.outputs {
                            changes.canvases.record(mount, view.clone(), key);
                        }
                    } else {
                        transient_views += 1;
                    }
                    views.push(view);
                    recording.before.push(command);
                    slots.push(Some(index));
                    PaintOutcome::Rendered
                }
                Err(reason) => {
                    let pass = &mut passes[request.pass];
                    let instance = pass
                        .batches
                        .iter()
                        .find(|batch| batch.draw == Draw::Canvas(request_index))
                        .expect("one batch belongs to the requested canvas")
                        .instances
                        .start;
                    pass.paints[instance] = request.fallback;
                    slots.push(None);
                    PaintOutcome::Fallback { reason }
                }
            };
            recording.receipts[request.receipt].outcome = outcome;
        }
        self.programs.finish_canvases(transient_views);
        for pass in passes {
            for batch in &mut pass.batches {
                if let Draw::Canvas(request) = batch.draw {
                    batch.draw = slots[request].map_or(Draw::Quad, Draw::Canvas);
                }
            }
        }
        views
    }

    #[doc(hidden)]
    pub fn canvas_updates(&self, list: &DisplayList, scale: f32) -> CanvasUpdates {
        let mut updates = CanvasUpdates { scale, pending: VecDeque::new() };
        let Ok((width, height)) = crate::pixels::dimensions((list.size.width, list.size.height), scale) else { return updates };
        let window = Rect::new(0.0, 0.0, width as f32, height as f32);
        let mut clips = ClipStack::default();
        let mut remaining = CanvasCache::CANVAS_BYTES;
        let max_dimension = self.device.limits().max_texture_dimension_2d;
        for placed in list.placed_items_at(scale) {
            if !matches!(placed.item, Item::PushClip { .. } | Item::PushAxisClip { .. } | Item::PopClip | Item::Custom(_)) {
                continue;
            }
            match placed.resolve().as_ref() {
                Item::PushClip { rect, .. } => clips.push(*rect, [0.0; 4], scale),
                Item::PopClip => clips.pop(),
                Item::Custom(paint)
                    if !paint.exiting
                        && paint.program.downcast_ref::<super::custom::Canvas>().is_some_and(|c| c.mode == CanvasMode::Retained) =>
                {
                    let clip = clips.current().bounds.map_or(NO_CLIP, |r| [r.x, r.y, r.w, r.h]);
                    let fallback = quad(&paint.fallback, scale, clip, 0);
                    if fallback.visible_bounds(&window).is_empty() {
                        continue;
                    }
                    let Some(canvas) = CanvasPaint::from_custom(paint, fallback, CanvasMode::Retained, scale, None, max_dimension) else {
                        continue;
                    };
                    let mount = paint.mount.downgrade();
                    if updates.pending.iter().any(|(prepared, _)| prepared.ptr_eq(&mount)) {
                        continue;
                    }
                    let Some(left) = remaining.checked_sub(rgba_bytes(canvas.size)) else { continue };
                    remaining = left;
                    updates.pending.push_back((mount, canvas));
                    if updates.pending.len() == CanvasCache::CANVAS_SLOTS || remaining == 0 {
                        return updates;
                    }
                }
                _ => {}
            }
        }
        updates
    }

    /// Records at most one author callback. The result joins the same cache used by an inline
    /// draw only after its commands were submitted. A superseded recording can be dropped first.
    #[doc(hidden)]
    #[cfg(not(target_arch = "wasm32"))]
    pub fn prepare_next_canvas(
        &mut self,
        updates: &mut CanvasUpdates,
        revision: u64,
        current: impl FnOnce() -> bool,
    ) -> Result<bool, wgpu::Error> {
        pollster::block_on(self.prepare_next_canvas_async(updates, revision, current))
    }

    pub async fn prepare_next_canvas_async(
        &mut self,
        updates: &mut CanvasUpdates,
        revision: u64,
        current: impl FnOnce() -> bool,
    ) -> Result<bool, wgpu::Error> {
        self.canvas_cache.prune();
        self.programs.prune_canvases();
        let scale = updates.scale;
        while let Some((mount, paint)) = updates.pending.pop_front() {
            let slot = mount.as_ptr().addr();
            let key = CanvasKey::new(&paint, scale);
            if mount.strong_count() == 0 || self.canvas_cache.get(slot, &key).is_some() {
                continue;
            }
            let mut changes = CanvasChanges::default();
            let target = CanvasTarget::Working(self.canvas_cache.working(slot, Some(&changes)));
            let Ok((view, commands)) = self.programs.encode_canvas(&self.device, &paint, scale, target, None).await else {
                return Ok(false);
            };
            if !current() {
                return Ok(false);
            }
            self.submit_commands_async([commands]).await?;
            changes.record(mount, view, key);
            self.canvas_cache.submit(changes, Some(revision));
            return Ok(true);
        }
        Ok(false)
    }
}
