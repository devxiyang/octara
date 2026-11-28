//! Accepted layer pixels and the native crossfade program.

use super::target::{Command, Texture};
use objc2::rc::Retained;
use objc2_metal::{MTLCommandBuffer, MTLCommandBufferStatus};
use octara_model::{
    capture::{PaintFallback, PaintOutcome, PaintSample},
    paint::{PaintProgram, TransitionPaint},
};
use octara_render::plan::{CanvasMode, CanvasProgram, FramePlan, PassBounds, bound_passes, transition_composite_bounds};
use std::{
    collections::VecDeque,
    sync::{Arc, Weak},
};

const HISTORY_BYTES: u64 = 32 * 1024 * 1024;
const HISTORY_SLOTS: usize = 64;
const PENDING_FRAMES: usize = 8;

struct CrossfadeProgram;

/// Blend the last accepted region and the current region in linear, premultiplied color.
pub fn transition_crossfade() -> PaintProgram {
    PaintProgram::new(CanvasProgram::new(CanvasMode::Transition, CrossfadeProgram))
}

/// Transition history from a successful recording. Accept it on the originating
/// renderer after committing the commands and accepting the frame for presentation.
#[must_use = "accept the frame to publish its transition history"]
pub struct MetalPresentation(Option<Arc<()>>);

impl MetalPresentation {
    /// Returns true when this frame has no transition history to publish.
    pub fn is_empty(&self) -> bool {
        self.0.is_none()
    }
}

pub(crate) struct TransitionPlan {
    pub(crate) mount: Weak<()>,
    pub(crate) run: Weak<()>,
    pub(crate) revision: u64,
    pub(crate) retain: bool,
    pub(crate) publish: bool,
    pub(crate) composite: Option<Crossfade>,
}

pub(crate) struct Crossfade {
    pub(crate) bounds: PassBounds,
    pub(crate) old: Region,
    pub(crate) progress: f32,
}

#[derive(Clone)]
pub(crate) struct Region {
    pub(crate) texture: Texture,
    pub(crate) bounds: PassBounds,
    pub(crate) scale: f32,
    pub(crate) sample: PaintSample,
}

pub(crate) struct Change {
    pub(crate) mount: Weak<()>,
    pub(crate) revision: u64,
    pub(crate) image: Option<Region>,
    pub(crate) pin: Option<(Weak<()>, Region)>,
}

struct Pending {
    token: Weak<()>,
    command: Retained<Command>,
    sequence: u64,
    changes: Vec<Change>,
}

struct Accepted {
    mount: Weak<()>,
    revision: u64,
    sequence: u64,
    image: Option<Region>,
    command: Option<Retained<Command>>,
}

enum Previous<'a> {
    Missing,
    Waiting,
    Pinned(&'a Region),
    Accepted(&'a Region),
}

struct Pin {
    mount: Weak<()>,
    run: Weak<()>,
    revision: u64,
    image: Region,
}

#[derive(Default)]
pub(crate) struct History {
    sequence: u64,
    pending: VecDeque<Pending>,
    regions: VecDeque<Accepted>,
    pins: VecDeque<Pin>,
}

impl History {
    pub(crate) fn prune(&mut self) {
        self.pending.retain(|pending| pending.token.strong_count() != 0);
        self.regions.retain_mut(|region| {
            if let Some(command) = &region.command {
                match command.status() {
                    MTLCommandBufferStatus::Completed => region.command = None,
                    MTLCommandBufferStatus::Committed | MTLCommandBufferStatus::Scheduled => {}
                    _ => {
                        region.command = None;
                        region.image = None;
                    }
                }
            }
            region.mount.strong_count() != 0
        });
        self.pins.retain(|pin| pin.mount.strong_count() != 0 && pin.run.strong_count() != 0);
    }

    pub(crate) fn plan(&self, frame: &mut FramePlan, extent: [u32; 2], scale: f32, publish: bool) -> Vec<Option<TransitionPlan>> {
        let mut transitions: Vec<Option<TransitionPlan>> = (0..frame.passes.len()).map(|_| None).collect();
        if !frame.passes.iter().any(|pass| pass.transition.is_some()) {
            return transitions;
        }
        let mut region_count = 0;
        let mut used_region_bytes = 0;
        let mut pin_count = self.pins.len();
        let mut pin_bytes = self.pins.iter().map(|pin| region_bytes(pin.image.bounds)).sum::<u64>();
        frame.bounds = bound_passes(&mut frame.passes, &frame.spatial, extent, |index, request, current, visible| {
            let paint = &request.paint;
            let supported = paint.program.downcast_ref::<CanvasProgram>().and_then(CanvasProgram::payload::<CrossfadeProgram>).is_some();
            let previous =
                if supported && request.receipt.is_some() && paint.opacity > 0.0 { self.previous(paint) } else { Previous::Missing };
            let composite = match previous {
                Previous::Pinned(old) | Previous::Accepted(old) => {
                    let bounds = transition_composite_bounds(current, visible, old.bounds, old.scale, scale);
                    let pins_new_region = publish && matches!(previous, Previous::Accepted(_));
                    let size = region_bytes(old.bounds);
                    if bounds.is_empty() || (pins_new_region && !fits(pin_count, pin_bytes, size)) {
                        None
                    } else {
                        pin_count += usize::from(pins_new_region);
                        pin_bytes += if pins_new_region { size } else { 0 };
                        Some(Crossfade { old: old.clone(), bounds, progress: paint.progress.clamp(0.0, 1.0) })
                    }
                }
                Previous::Missing | Previous::Waiting => None,
            };
            if let Some(receipt) = request.receipt {
                frame.requests.receipts[receipt].outcome = if let Some(composite) = &composite {
                    PaintOutcome::Transition { old: composite.old.sample }
                } else {
                    let reason = if supported { PaintFallback::HistoryUnavailable } else { PaintFallback::UnsupportedProgram };
                    PaintOutcome::Fallback { reason }
                };
            }
            let output = composite.as_ref().map_or(current, |composite| composite.bounds);
            // Wait for the accepted producer to finish without replacing its image
            // with the current revision's ordinary pixels on the intervening frame.
            let publish = publish && !matches!(previous, Previous::Waiting);
            let size = region_bytes(output);
            let retain = supported && publish && paint.opacity > 0.0 && !output.is_empty() && fits(region_count, used_region_bytes, size);
            if retain {
                region_count += 1;
                used_region_bytes += size;
            }
            transitions[index] = Some(TransitionPlan {
                mount: paint.mount.downgrade(),
                run: paint.run.downgrade(),
                revision: paint.revision,
                retain,
                publish,
                composite,
            });
            output
        });
        transitions
    }

    fn previous(&self, paint: &TransitionPaint) -> Previous<'_> {
        let run = paint.run.downgrade();
        let mount = paint.mount.downgrade();
        if let Some(pin) = self.pins.iter().find(|pin| pin.run.ptr_eq(&run) && pin.mount.ptr_eq(&mount) && pin.revision == paint.revision) {
            return Previous::Pinned(&pin.image);
        }
        let region = self.regions.iter().find(|region| region.mount.ptr_eq(&mount) && region.revision < paint.revision);
        let Some((region, image)) = region.and_then(|region| region.image.as_ref().map(|image| (region, image))) else {
            return Previous::Missing;
        };
        match region.command.as_ref().map(|command| command.status()) {
            None | Some(MTLCommandBufferStatus::Completed) => Previous::Accepted(image),
            Some(MTLCommandBufferStatus::Committed | MTLCommandBufferStatus::Scheduled) => Previous::Waiting,
            _ => Previous::Missing,
        }
    }

    pub(crate) fn pending(&mut self, command: &Retained<Command>, changes: Vec<Change>) -> MetalPresentation {
        if changes.is_empty() {
            return MetalPresentation(None);
        }
        let token = Arc::new(());
        self.sequence = self.sequence.checked_add(1).expect("Metal presentation sequence exhausted");
        if self.pending.len() >= PENDING_FRAMES {
            self.pending.pop_front();
        }
        self.pending.push_back(Pending { token: Arc::downgrade(&token), command: command.clone(), sequence: self.sequence, changes });
        MetalPresentation(Some(token))
    }

    pub(crate) fn accept(&mut self, presentation: MetalPresentation) {
        let Some(token) = presentation.0 else { return };
        let token_ref = Arc::downgrade(&token);
        let Some(index) = self.pending.iter().position(|pending| pending.token.ptr_eq(&token_ref)) else { return };
        let Pending { command, sequence, changes, .. } = self.pending.remove(index).expect("matched presentation");
        let status = command.status();
        if !matches!(status, MTLCommandBufferStatus::Committed | MTLCommandBufferStatus::Scheduled | MTLCommandBufferStatus::Completed) {
            return;
        }
        self.prune();
        let mut pin_bytes = self.pins.iter().map(|pin| region_bytes(pin.image.bounds)).sum::<u64>();
        for Change { mount, revision, image, pin } in changes {
            if mount.strong_count() == 0 {
                continue;
            }
            if let Some(index) = self.regions.iter().position(|entry| entry.mount.ptr_eq(&mount)) {
                let entry = &self.regions[index];
                if entry.sequence > sequence || entry.revision > revision {
                    continue;
                }
                self.regions.remove(index);
            }
            if let Some((run, image)) = pin
                && run.strong_count() != 0
                && self.pins.iter().all(|entry| !entry.run.ptr_eq(&run))
            {
                let size = region_bytes(image.bounds);
                if fits(self.pins.len(), pin_bytes, size) {
                    self.pins.push_back(Pin { mount: mount.clone(), run, revision, image });
                    pin_bytes += size;
                }
            }
            let producer = (image.is_some() && status != MTLCommandBufferStatus::Completed).then(|| command.clone());
            self.regions.push_back(Accepted { mount, revision, sequence, image, command: producer });
        }
        let size = |entry: &Accepted| entry.image.as_ref().map_or(0, |image| region_bytes(image.bounds));
        let mut region_bytes: u64 = self.regions.iter().map(size).sum();
        while self.regions.len() > HISTORY_SLOTS || region_bytes > HISTORY_BYTES {
            region_bytes -= size(&self.regions.pop_front().expect("a transition region over budget"));
        }
    }
}

fn region_bytes(bounds: PassBounds) -> u64 {
    u64::from(bounds.size[0]) * u64::from(bounds.size[1]) * 8
}

fn fits(slots: usize, bytes: u64, incoming: u64) -> bool {
    slots < HISTORY_SLOTS && bytes + incoming <= HISTORY_BYTES
}
