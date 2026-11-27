use super::*;
use crate::{CommitOutput, HostEngine, ProjectError, ProjectFeedback, WindowOptions};
use octara_model::geometry::{LayoutBox, Point, WindowViewport};
use octara_model::image::{Image, ObjectFit};
use octara_model::input::ContactTarget;
use octara_tree::transaction::{NodeContent, NodeSpec};
use octara_tree::{Tree, TreeDelta};
use std::cell::Cell;
use std::collections::VecDeque;
use std::task::{Poll, Waker};

type TestBounds = Rc<Cell<Rect>>;

struct Bounds(TestBounds);

impl HostEngine for Bounds {
    fn project(&mut self, _: &Tree, _: &TreeDelta, _: WindowViewport, _: f64) -> Result<ProjectFeedback, ProjectError> {
        Ok(ProjectFeedback::default())
    }

    fn commit(&mut self) -> CommitOutput {
        CommitOutput::default()
    }
    fn rollback(&mut self) {}
    fn hit_test(&self, _: &Tree, _: Point) -> Option<ContactTarget> {
        None
    }
    fn geometry(&self, _: NodeId) -> Option<LayoutBox> {
        None
    }
    fn interaction_bounds(&self, _: NodeId) -> Option<Rect> {
        Some(self.0.get())
    }
}

#[derive(Default)]
struct Loader {
    requests: RefCell<Vec<Rc<Pending>>>,
    resize: Cell<bool>,
    variants: RefCell<Vec<(MediaVariant, Rc<Pending>)>>,
}

#[derive(Default)]
struct Pending {
    cancel: CancellationToken,
    progress: MediaProgress,
    disk: bool,
    dropped: Cell<bool>,
    image: RefCell<Option<LoadedMedia>>,
    waker: RefCell<Option<Waker>>,
}

impl Pending {
    fn fill(&self) {
        self.complete(LoadedMedia::pixels(Image::rgba(1, 1, [0; 4])));
    }

    fn complete(&self, image: LoadedMedia) {
        self.image.replace(Some(image));
        if let Some(waker) = self.waker.take() {
            waker.wake();
        }
    }
}

struct DropNotice(Rc<Pending>);

impl Drop for DropNotice {
    fn drop(&mut self) {
        self.0.dropped.set(true);
    }
}

impl MediaLoader for Rc<Loader> {
    fn load(&self, request: MediaRequest, progress: MediaProgress) -> super::super::MediaFuture {
        let pending = Rc::new(Pending { cancel: request.cancel, progress, disk: request.disk_cache.is_some(), ..Pending::default() });
        self.requests.borrow_mut().push(pending.clone());
        wait_for_image(pending)
    }

    fn has_variants(&self) -> bool {
        self.resize.get()
    }

    fn variant(&self, _: LoadedMedia, target: MediaVariant, cancel: CancellationToken) -> super::super::MediaFuture {
        let pending = Rc::new(Pending { cancel, ..Pending::default() });
        self.variants.borrow_mut().push((target, pending.clone()));
        wait_for_image(pending)
    }
}

fn wait_for_image(pending: Rc<Pending>) -> super::super::MediaFuture {
    let notify = DropNotice(pending.clone());
    Box::pin(async move {
        let result = poll_fn(|cx| {
            if let Some(image) = pending.image.take() {
                Poll::Ready(Ok(image))
            } else {
                pending.waker.replace(Some(cx.waker().clone()));
                Poll::Pending
            }
        })
        .await;
        drop(notify);
        result
    })
}

#[derive(Default)]
struct NoDisk;

impl MediaCacheBackend for NoDisk {
    fn get(&self, _: &str) -> Option<super::super::HttpMediaEntry> {
        unreachable!()
    }
    fn put(&self, _: &str, _: super::super::HttpMediaEntry) {
        unreachable!()
    }
    fn remove(&self, _: &str) {
        unreachable!()
    }
}

struct Fixture {
    app: AppRuntime,
    loader: Rc<Loader>,
    bounds: Rc<RefCell<VecDeque<TestBounds>>>,
}

impl Fixture {
    fn new() -> Self {
        let bounds: Rc<RefCell<VecDeque<TestBounds>>> = Rc::default();
        let engine = bounds.clone();
        let mut app = AppRuntime::new(move |_, _| Box::new(Bounds(engine.borrow_mut().pop_front().expect("window bounds"))));
        let loader = Rc::new(Loader::default());
        app.set_media_loader(loader.clone());
        app.set_media_cache_backend(Arc::new(NoDisk));
        Self { app, loader, bounds }
    }

    fn open(&mut self, source: ImageSource, y: f32) -> (WindowId, TestBounds) {
        self.open_at(source, Rect::new(0.0, y, 60.0, 40.0))
    }

    fn open_at(&mut self, source: ImageSource, bounds: Rect) -> (WindowId, TestBounds) {
        let bounds = Rc::new(Cell::new(bounds));
        self.bounds.borrow_mut().push_back(bounds.clone());
        let id = self
            .app
            .open_window(WindowOptions::default(), move |mount| {
                let node = mount.create(NodeSpec::new(NodeContent::ImageSource { source, fit: ObjectFit::Contain }));
                mount.place(mount.root(), 0, node);
            })
            .unwrap();
        (id, bounds)
    }

    fn tick(&mut self) {
        self.app.advance_to(self.app.now()).unwrap();
    }

    fn state(&self, id: WindowId) -> ImageState {
        self.app.window(id).unwrap().tree().media_sources().next().unwrap().1.state.clone()
    }

    fn natural_size(&self, id: WindowId) -> Option<(u32, u32)> {
        self.app.window(id).unwrap().tree().media_sources().next().unwrap().1.natural_size()
    }

    fn pixels(&self, id: WindowId) -> Option<Image> {
        let slot = self.app.window(id).unwrap().tree().media_sources().next().unwrap().1;
        match &slot.asset {
            Some(MediaAsset::Pixels(image)) => Some(image.clone()),
            _ => None,
        }
    }

    fn request(&self, index: usize) -> Rc<Pending> {
        self.loader.requests.borrow()[index].clone()
    }

    fn started(&self) -> usize {
        self.loader.requests.borrow().len()
    }

    fn complete_variant(&self, index: usize, (width, height): (u32, u32), natural_size: (u32, u32)) -> Image {
        let (target, request) = &self.loader.variants.borrow()[index];
        assert_eq!((target.width, target.height), (width, height));
        let image = Image::rgba(width, height, vec![0; (width * height * 4) as usize]);
        request.complete(LoadedMedia { natural_size, ..LoadedMedia::pixels(image.clone()) });
        image
    }
}

#[test]
fn announces_probe_and_cancels_when_the_last_view_leaves_the_prefetch_band() {
    let mut fixture = Fixture::new();
    let source = ImageSource::Uri("https://example.test/shared".into());
    let (first, first_position) = fixture.open(source.clone(), 0.0);
    let (second, second_position) = fixture.open(source, 800.0);
    fixture.tick();
    assert_eq!(fixture.started(), 1);
    assert!(fixture.request(0).disk);
    assert_eq!(fixture.state(first), ImageState::Loading { probed_size: None });

    let first_request = fixture.request(0);
    first_request.progress.probed((30, 15));
    fixture.tick();
    assert_eq!(fixture.state(first), ImageState::Loading { probed_size: Some((30, 15)) });
    assert_eq!(fixture.state(first), fixture.state(second));

    first_position.set(Rect { y: 2000.0, ..first_position.get() });
    fixture.tick();
    assert_eq!(fixture.state(first), ImageState::Idle);
    assert_eq!(fixture.natural_size(first), Some((30, 15)));
    assert!(!first_request.cancel.is_cancelled());

    second_position.set(Rect { y: 2000.0, ..second_position.get() });
    fixture.tick();
    assert!(first_request.cancel.is_cancelled());
    assert!(first_request.dropped.get());
    assert_eq!(fixture.state(second), ImageState::Idle);
    assert_eq!(fixture.natural_size(second), Some((30, 15)));

    first_request.fill();
    first_position.set(Rect { y: 0.0, ..first_position.get() });
    fixture.tick();
    fixture.tick();
    assert_eq!(fixture.started(), 2);
    assert_eq!(fixture.state(first), ImageState::Loading { probed_size: Some((30, 15)) });
    fixture.request(1).fill();
    fixture.tick();
    assert_eq!(fixture.state(first), ImageState::Ready { natural_size: (1, 1) });

    let (third, _) = fixture.open(ImageSource::Uri("https://example.test/shared".into()), 0.0);
    fixture.tick();
    assert_eq!(fixture.started(), 2);
    assert_eq!(fixture.state(third), ImageState::Ready { natural_size: (1, 1) });
}

#[test]
fn zero_sized_images_load_at_visible_and_prefetch_origins() {
    let mut fixture = Fixture::new();
    let (visible, _) = fixture.open_at(ImageSource::Uri("https://example.test/visible".into()), Rect::new(20.0, 30.0, 0.0, 0.0));
    let (prefetch, _) = fixture.open_at(ImageSource::Uri("https://example.test/prefetch".into()), Rect::new(20.0, 800.0, 0.0, 0.0));
    let (outside, _) = fixture.open_at(ImageSource::Uri("https://example.test/outside".into()), Rect::new(20.0, 2000.0, 0.0, 0.0));
    fixture.tick();
    assert_eq!(fixture.started(), 2);
    assert_eq!(fixture.state(visible), ImageState::Loading { probed_size: None });
    assert_eq!(fixture.state(prefetch), ImageState::Loading { probed_size: None });
    assert_eq!(fixture.state(outside), ImageState::Idle);
}

#[test]
fn resizing_a_ready_image_keeps_pixels_until_the_new_variant_is_ready() {
    let mut fixture = Fixture::new();
    fixture.loader.resize.set(true);
    let (id, bounds) = fixture.open(ImageSource::Uri("https://example.test/resize".into()), 0.0);
    fixture.tick();
    let natural_size = (120, 80);
    fixture.request(0).complete(LoadedMedia::pixels(Image::rgba(120, 80, vec![0; 120 * 80 * 4])));
    fixture.tick();
    fixture.tick();
    let small = fixture.complete_variant(0, (60, 40), natural_size);
    fixture.tick();
    assert_eq!(fixture.pixels(id), Some(small.clone()));

    bounds.set(Rect::new(0.0, 0.0, 90.0, 60.0));
    fixture.tick();
    fixture.tick();
    assert_eq!(fixture.pixels(id), Some(small.clone()));
    assert_eq!(fixture.state(id), ImageState::Ready { natural_size });
    let large = fixture.complete_variant(1, (90, 60), natural_size);
    fixture.tick();
    assert_eq!(fixture.pixels(id), Some(large));

    bounds.set(Rect::new(0.0, 0.0, 60.0, 40.0));
    fixture.tick();
    fixture.tick();
    assert_eq!(fixture.pixels(id), Some(small));
    assert_eq!(fixture.started(), 1);
    assert_eq!(fixture.loader.variants.borrow().len(), 2);
}

fn private(header: &str, cache: CachePolicy) -> ImageSource {
    ImageSource::Resolved { uri: "https://example.test/private".into(), headers: [("Authorization".into(), header.into())].into(), cache }
}

#[test]
fn private_headers_bypass_disk_and_distinguish_in_flight_requests() {
    let mut fixture = Fixture::new();
    let first = private("one", CachePolicy::Standard);
    let second = private("two", CachePolicy::Standard);
    let (first_id, _) = fixture.open(first.clone(), 0.0);
    fixture.open(second, 0.0);
    fixture.tick();
    assert_eq!(fixture.started(), 2);
    for i in 0..2 {
        assert!(!fixture.request(i).disk);
        fixture.request(i).fill();
    }
    fixture.tick();
    fixture.app.close_window(first_id).unwrap();
    let (third, _) = fixture.open(first, 0.0);
    assert_eq!(fixture.state(third), ImageState::Ready { natural_size: (1, 1) });
}

#[test]
fn no_cache_reloads_after_retirement_and_visible_loads_preempt_prefetches() {
    let mut fixture = Fixture::new();
    let source = private("uncached", CachePolicy::NoCache);
    let (first, _) = fixture.open(source.clone(), 0.0);
    fixture.tick();
    fixture.request(0).fill();
    fixture.tick();
    fixture.app.close_window(first).unwrap();
    let (reloaded, _) = fixture.open(source, 0.0);
    fixture.tick();
    assert_eq!(fixture.started(), 2);
    assert!(!fixture.request(1).disk);
    fixture.app.close_window(reloaded).unwrap();

    for index in 0..PARALLEL_LOADS {
        fixture.open(ImageSource::Uri(format!("https://example.test/prefetch-{index}").into()), 800.0);
    }
    fixture.tick();
    let speculative = fixture.started();
    assert_eq!(speculative, PARALLEL_LOADS + 2);

    fixture.open(ImageSource::Uri("https://example.test/visible".into()), 0.0);
    let retired = (2..speculative).filter(|&i| fixture.request(i).cancel.is_cancelled()).count();
    assert_eq!(retired, 1);
    fixture.tick();
    assert_eq!(fixture.started(), speculative + 1);
    assert!(!fixture.request(speculative).cancel.is_cancelled());
}
