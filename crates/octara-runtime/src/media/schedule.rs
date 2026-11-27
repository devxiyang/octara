//! Application-wide in-flight deduplication and bounded decoded image caches.
#![allow(clippy::mutable_key_type, reason = "Pixel sources use stable identity and bypass async cache keys")]

use super::{CancellationToken, LoadedMedia, MediaCacheBackend, MediaLoader, MediaProgress, MediaRequest, MediaVariant, PixelsOnly};
use crate::AppRuntime;
use crate::arena::ScopeId;
use crate::task::{TaskId, TaskScheduler};
use octara_model::geometry::{Point, Rect};
use octara_model::identity::WindowId;
use octara_model::image::{CachePolicy, ImageSource, ImageState, MediaAsset};
use octara_model::style::Display;
use octara_tree::{NodeFlags, NodeId, RuntimeOp, Tree, tables::MediaSourceSlot};
use rustc_hash::FxHashMap;
use std::future::poll_fn;
use std::sync::{Arc, Mutex};
use std::time::Duration;

const PARALLEL_LOADS: usize = 4;
const MIB: usize = 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct JobKey {
    source: ImageSource,
    variant: Option<MediaVariant>,
}

struct Job {
    task: TaskId,
    cancel: CancellationToken,
    probed: Option<(u32, u32)>,
    variant_started: Option<Duration>,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct Owner {
    window: WindowId,
    node: NodeId,
    inline: Option<usize>,
}

impl Owner {
    fn slot(self, tree: &Tree) -> Option<&MediaSourceSlot> {
        match self.inline {
            Some(index) => tree.inline_media_source(self.node, index),
            None => tree.media_source(self.node),
        }
    }

    fn update(self, state: ImageState, asset: Option<MediaAsset>) -> RuntimeOp {
        match self.inline {
            Some(index) => RuntimeOp::SetInlineMedia { node: self.node, index, state, asset },
            None => RuntimeOp::SetMedia { node: self.node, state, asset },
        }
    }
}

struct Desired {
    owner: Owner,
    source: ImageSource,
    variant: MediaVariant,
    priority: u8,
}

enum JobEvent {
    Probed(JobKey, (u32, u32)),
    Finished(JobKey, Result<LoadedMedia, Arc<str>>),
}

type Mailbox = Arc<Mutex<Vec<JobEvent>>>;
type ResultMap = FxHashMap<JobKey, Result<LoadedMedia, Arc<str>>>;

struct CacheEntry {
    asset: LoadedMedia,
    weight: usize,
    touch: u64,
    expires: Option<Duration>,
}

struct Cache {
    entries: FxHashMap<JobKey, CacheEntry>,
    bytes: usize,
    limit: usize,
    clock: u64,
}

impl Cache {
    fn new(limit: usize) -> Self {
        Self { entries: FxHashMap::default(), bytes: 0, limit, clock: 0 }
    }

    fn get(&mut self, key: &JobKey, now: Duration) -> Option<LoadedMedia> {
        let entry = self.entries.get_mut(key)?;
        if entry.expires.is_some_and(|deadline| deadline <= now) {
            self.remove(key);
            return None;
        }
        self.clock += 1;
        entry.touch = self.clock;
        // A derivative may not survive longer than the decoded source it came from.
        let cache_for = entry.expires.map(|deadline| deadline.saturating_sub(now));
        Some(LoadedMedia { cache_for, ..entry.asset.clone() })
    }

    fn remove(&mut self, key: &JobKey) {
        if let Some(entry) = self.entries.remove(key) {
            self.bytes -= entry.weight;
        }
    }

    fn put(&mut self, key: JobKey, loaded: LoadedMedia, now: Duration) {
        let bytes = match &loaded.asset {
            MediaAsset::Pixels(image) => image.frame().rgba.len(),
            MediaAsset::BrowserUri(uri) => uri.len(),
        };
        let encoded = match &key.source {
            ImageSource::Encoded { bytes, .. } => bytes.len(),
            _ => 0,
        };
        let weight = 256_usize.saturating_add(bytes).saturating_add(encoded);
        if weight > self.limit {
            return;
        }
        self.clock += 1;
        let expires = loaded.cache_for.map(|duration| now.saturating_add(duration));
        let entry = CacheEntry { asset: loaded, weight, touch: self.clock, expires };
        if let Some(old) = self.entries.insert(key, entry) {
            self.bytes -= old.weight;
        }
        self.bytes += weight;
        while self.bytes > self.limit {
            if let Some(oldest) = self.entries.iter().min_by_key(|(_, entry)| entry.touch).map(|(key, _)| key.clone()) {
                self.remove(&oldest);
            }
        }
    }
}

pub(crate) struct MediaScheduler {
    pub(crate) loader: Arc<dyn MediaLoader>,
    pub(crate) disk_cache: Option<Arc<dyn MediaCacheBackend>>,
    originals: Cache,
    variants: Cache,
    presented_targets: FxHashMap<Owner, MediaVariant>,
    jobs: FxHashMap<JobKey, Job>,
    events: Mailbox,
}

impl Default for MediaScheduler {
    fn default() -> Self {
        Self {
            loader: Arc::new(PixelsOnly),
            disk_cache: None,
            originals: Cache::new(20 * MIB),
            variants: Cache::new(30 * MIB),
            presented_targets: FxHashMap::default(),
            jobs: FxHashMap::default(),
            events: Mailbox::default(),
        }
    }
}

impl MediaScheduler {
    pub(crate) fn clear(&mut self, tasks: &mut TaskScheduler) {
        for (_, job) in self.jobs.drain() {
            job.cancel.cancel();
            tasks.cancel(job.task);
        }
        self.events.lock().expect("media events").clear();
        self.presented_targets.clear();
    }

    fn completions(&mut self, now: Duration) -> ResultMap {
        let events = std::mem::take(&mut *self.events.lock().expect("media events"));
        let mut finished = FxHashMap::default();
        for event in events {
            match event {
                JobEvent::Probed(key, size) => {
                    if let Some(job) = self.jobs.get_mut(&key) {
                        job.probed = Some(size);
                    }
                }
                JobEvent::Finished(key, result) => {
                    let Some(job) = self.jobs.remove(&key) else { continue };
                    let result = result.and_then(|mut loaded| {
                        if let (Some(started), Some(lifetime)) = (job.variant_started, loaded.cache_for) {
                            loaded.cache_for = Some(lifetime.saturating_sub(now.saturating_sub(started)));
                        }
                        let (width, height) = loaded.natural_size;
                        // Empty RGBA images are supported authoring values.
                        if (width == 0 || height == 0) && !matches!(key.source, ImageSource::Pixels(_)) {
                            return Err(Arc::from("Image has no natural size"));
                        }
                        Ok(loaded)
                    });
                    if let Ok(loaded) = &result
                        && loaded.cacheable
                        && key.source.cache_policy() != CachePolicy::NoCache
                    {
                        let cache = if key.variant.is_some() { &mut self.variants } else { &mut self.originals };
                        cache.put(key.clone(), loaded.clone(), now);
                    }
                    finished.insert(key, result);
                }
            }
        }
        finished
    }

    fn reconcile(&mut self, queued: Vec<(JobKey, Option<LoadedMedia>)>, scope: ScopeId, tasks: &mut TaskScheduler) {
        // The queue is in priority order and unique by key. Preempt speculative work
        // when visible jobs need its connection slots.
        let running = &queued[..queued.len().min(PARALLEL_LOADS)];
        let cancelled: Vec<_> = self.jobs.keys().filter(|key| !running.iter().any(|(wanted, _)| wanted == *key)).cloned().collect();
        for key in cancelled {
            let job = self.jobs.remove(&key).expect("active media load");
            job.cancel.cancel();
            tasks.cancel(job.task);
        }
        for (key, original) in queued.into_iter().take(PARALLEL_LOADS) {
            if self.jobs.contains_key(&key) {
                continue;
            }
            self.spawn(key, original, scope, tasks);
        }
    }

    fn spawn(&mut self, key: JobKey, original: Option<LoadedMedia>, scope: ScopeId, tasks: &TaskScheduler) {
        let cancel = CancellationToken::default();
        let request = MediaRequest {
            source: key.source.clone(),
            cancel: cancel.clone(),
            disk_cache: if key.source.cache_policy() == CachePolicy::Standard
                && matches!(key.source, ImageSource::Uri(_) | ImageSource::Resolved { .. })
            {
                self.disk_cache.clone()
            } else {
                None
            },
        };
        let loader = self.loader.clone();
        let events = self.events.clone();
        let job_key = key.clone();
        let spawner = tasks.spawner();
        let task = spawner.spawn(scope, None, "media", move |_| async move {
            let progress = MediaProgress::default();
            let mut future = match (job_key.variant, original) {
                (Some(variant), Some(loaded)) => loader.variant(loaded, variant, request.cancel),
                _ => loader.load(request, progress.clone()),
            };
            let mut last_probe = None;
            let result = poll_fn(|cx| {
                let result = future.as_mut().poll(cx);
                if let Some(size) = progress.read(cx.waker()).filter(|size| Some(*size) != last_probe) {
                    last_probe = Some(size);
                    events.lock().expect("media events").push(JobEvent::Probed(job_key.clone(), size));
                }
                result
            })
            .await;
            events.lock().expect("media events").push(JobEvent::Finished(job_key, result));
        });
        spawner.defer_until_next_turn(task);
        let variant_started = key.variant.is_some().then(|| tasks.now());
        self.jobs.insert(key, Job { task, cancel, probed: None, variant_started });
    }
}

impl Drop for MediaScheduler {
    fn drop(&mut self) {
        for job in self.jobs.values() {
            job.cancel.cancel();
        }
    }
}

impl AppRuntime {
    pub(crate) fn sync_media(&mut self) {
        let now = self.now();
        let finished = self.media.completions(now);
        let supports_variants = self.media.loader.has_variants();
        self.media.presented_targets.retain(|owner, _| {
            self.windows.get(&owner.window).and_then(|window| owner.slot(&window.tree)).is_some_and(|slot| slot.asset.is_some())
        });
        let mut desired = Vec::new();
        for (&window, runtime) in &self.windows {
            let viewport = runtime.options.viewport;
            let view = Rect::new(0.0, 0.0, viewport.size.width, viewport.size.height);
            let media = runtime
                .tree
                .media_sources()
                .map(|(node, slot)| (node, None, slot))
                .chain(runtime.tree.inline_media_sources().map(|(node, index, slot)| (node, Some(index), slot)));
            for (node, inline, slot) in media {
                let mut priority = 2;
                let mut size = (0, 0);
                let bounds = match inline {
                    Some(index) => runtime.engine.inline_attachment_bounds(node, index),
                    None => runtime.engine.interaction_bounds(node),
                };
                if let Some(bounds) = bounds {
                    let mut visible = view;
                    let mut prefetch = expand(view);
                    let mut hidden = false;
                    for ancestor in std::iter::successors(Some(node), |&node| runtime.tree.parent(node)) {
                        hidden |= runtime.tree.node(ancestor).is_some_and(|core| core.flags.contains(NodeFlags::HIDDEN))
                            || runtime.tree.layout_style(ancestor).display == Display::None;
                        if ancestor != node
                            && runtime.tree.scroll(ancestor).is_some()
                            && let Some(clip) = runtime.engine.interaction_bounds(ancestor)
                        {
                            visible = visible.intersect(&clip);
                            prefetch = prefetch.intersect(&expand(clip));
                        }
                    }
                    if !hidden {
                        if touches(bounds, visible) {
                            priority = 0;
                        } else if touches(bounds, prefetch) {
                            priority = 1;
                        }
                    }
                    size = ((bounds.w * viewport.scale).round() as u32, (bounds.h * viewport.scale).round() as u32);
                }
                if matches!(slot.source, ImageSource::Pixels(_)) {
                    priority = 0;
                }
                desired.push(Desired {
                    owner: Owner { window, node, inline },
                    source: slot.source.clone().normalize_cache_policy(),
                    variant: MediaVariant { width: size.0, height: size.1, fit: slot.fit },
                    priority,
                });
            }
        }
        desired.sort_by_key(|want| {
            let Owner { window, node, inline } = want.owner;
            (want.priority, window, octara_model::identity::NativeId::from(node), inline)
        });
        let mut queued = Vec::new();
        let mut keys = rustc_hash::FxHashSet::default();
        for want in desired {
            let owner = want.owner;
            let runtime = self.windows.get_mut(&owner.window).expect("media window");
            let slot = owner.slot(&runtime.tree).expect("live media source");
            if let ImageSource::Pixels(image) = want.source {
                let state = ImageState::Ready { natural_size: image.size() };
                runtime.apply(owner.update(state, Some(MediaAsset::Pixels(image))));
                continue;
            }
            if want.priority == 2 {
                self.media.presented_targets.remove(&owner);
                if !matches!(slot.state, ImageState::Idle) && (!matches!(slot.state, ImageState::Ready { .. }) || slot.asset.is_some()) {
                    let state = if matches!(slot.state, ImageState::Ready { .. }) { slot.state.clone() } else { ImageState::Idle };
                    runtime.apply(owner.update(state, None));
                }
                continue;
            }
            let showing_asset = slot.asset.is_some();
            if matches!(slot.state, ImageState::Failed(_))
                || (showing_asset && (!supports_variants || self.media.presented_targets.get(&owner) == Some(&want.variant)))
            {
                continue;
            }
            let original_key = JobKey { source: want.source.clone(), variant: None };
            let use_cache = want.source.cache_policy() != CachePolicy::NoCache;
            let original = finished
                .get(&original_key)
                .cloned()
                .or_else(|| if use_cache { self.media.originals.get(&original_key, now).map(Ok) } else { None });
            if showing_asset && !matches!(original, Some(Ok(_))) {
                // Resizing already displayed media does not justify a new download.
                self.media.presented_targets.insert(owner, want.variant);
                continue;
            }
            let mut probed = self.media.jobs.get(&original_key).and_then(|job| job.probed).or_else(|| slot.natural_size());
            let mut job = (original_key.clone(), None);
            let result = match original {
                Some(Ok(loaded))
                    if supports_variants
                        && loaded.cacheable
                        && use_cache
                        && want.variant.needs_resize(loaded.natural_size)
                        && self.media.originals.entries.contains_key(&original_key) =>
                {
                    probed = Some(loaded.natural_size);
                    let variant_key = JobKey { source: want.source, variant: Some(want.variant) };
                    let result = finished
                        .get(&variant_key)
                        .map(|variant| variant.clone().or_else(|_| Ok(loaded.clone())))
                        .or_else(|| self.media.variants.get(&variant_key, now).map(Ok));
                    job = (variant_key, Some(loaded));
                    result
                }
                other => other,
            };
            let (state, asset) = match result {
                Some(Ok(loaded)) => {
                    if supports_variants {
                        self.media.presented_targets.insert(owner, want.variant);
                    }
                    (ImageState::Ready { natural_size: loaded.natural_size }, Some(loaded.asset))
                }
                Some(Err(error)) => (ImageState::Failed(error), None),
                None => {
                    if keys.insert(job.0.clone()) {
                        queued.push(job);
                    }
                    if showing_asset {
                        continue;
                    }
                    (ImageState::Loading { probed_size: probed }, None)
                }
            };
            runtime.apply(owner.update(state, asset));
        }
        self.media.reconcile(queued, self.world.root_scope(), &mut self.tasks);
    }
}

impl MediaVariant {
    fn needs_resize(self, (width, height): (u32, u32)) -> bool {
        self.width != 0
            && self.height != 0
            && self.fit != octara_model::image::ObjectFit::None
            && width > self.width
            && height > self.height
    }
}

fn expand(rect: Rect) -> Rect {
    Rect::new(rect.x - rect.w, rect.y - rect.h, rect.w * 3.0, rect.h * 3.0)
}

fn touches(bounds: Rect, viewport: Rect) -> bool {
    if bounds.is_empty() {
        // An unloaded image can have no intrinsic size yet. Admit it at its laid-out origin.
        viewport.contains(Point::new(bounds.x, bounds.y))
    } else {
        !bounds.intersect(&viewport).is_empty()
    }
}

#[cfg(test)]
mod tests;
