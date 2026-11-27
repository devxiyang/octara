#![cfg(feature = "media")]

use octara::prelude::*;
use octara::{
    CancellationToken, Input, InteractionEvent, LoadedMedia, MediaFuture, MediaLoader, MediaProgress, MediaRequest, PaintItem, PointerId,
    PointerKind,
};
use octara_inspect::HeadlessDriver;
use octara_markdown::{LinkPillContent, MarkdownConfig, markdown, markdown_with};
use std::future::{pending, ready};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

struct ScopedResolver;

impl MediaResolver for ScopedResolver {
    fn resolve(&self, uri: &str) -> Option<ImageSource> {
        let path = uri.strip_prefix("cdn:")?;
        Some(ImageSource::Resolved {
            uri: format!("https://images.test/{path}").into(),
            headers: Arc::from([(Arc::from("Authorization"), Arc::from("Bearer example"))]),
            cache: CachePolicy::Standard,
        })
    }
}

struct PersistentCache;

impl MediaCacheBackend for PersistentCache {
    fn get(&self, _: &str) -> Option<HttpMediaEntry> {
        None
    }

    fn put(&self, _: &str, _: HttpMediaEntry) {}
    fn remove(&self, _: &str) {}
}

#[derive(Clone)]
struct Load {
    source: ImageSource,
    persistent: bool,
    cancel: CancellationToken,
    dropped: Arc<AtomicBool>,
}

struct DropNotice(Arc<AtomicBool>);

impl Drop for DropNotice {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}

struct PendingLoader(Arc<Mutex<Vec<Load>>>);

impl MediaLoader for PendingLoader {
    fn load(&self, request: MediaRequest, _: MediaProgress) -> MediaFuture {
        let dropped = Arc::new(AtomicBool::new(false));
        self.0.lock().unwrap().push(Load {
            source: request.source,
            persistent: request.disk_cache.is_some(),
            cancel: request.cancel,
            dropped: dropped.clone(),
        });
        let notice = DropNotice(dropped);
        Box::pin(async move {
            let result = pending().await;
            drop(notice);
            result
        })
    }
}

#[test]
fn scoped_block_and_inline_requests_bypass_disk_with_auth_and_cancel_after_scrolling() {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let links = Arc::new(Mutex::new(Vec::new()));
    let events = links.clone();
    let mut driver = HeadlessDriver::default();
    driver.app_mut().runtime_mut().set_media_loader(PendingLoader(requests.clone()));
    let definition = AppDefinition::new().with_media_cache_backend(Arc::new(PersistentCache)).window(WindowDefinition::new(
        WindowOptions::default(),
        move |build| {
            build.provide::<Arc<dyn MediaResolver>>(Arc::new(ScopedResolver));
            col().w(px(320.0)).h(px(360.0)).scroll_y().test_id("scroll").child(
                col()
                    .shrink(0.0)
                    .child(
                        markdown(
                            build,
                            "![Block](cdn:block)\n\nSee [![Map](cdn:inline)](https://example.test/map).\n\n![Public](https://images.test/public)",
                        )
                        .on_link(move |_, event| {
                            events.lock().unwrap().push(event.clone());
                            event.kind == LinkEventKind::Press
                        }),
                    )
                    .child(el().h(px(1800.0)).shrink(0.0)),
            )
        },
    ));
    let id = driver.launch(definition).unwrap()[0];
    let now = driver.app().runtime().now();
    driver.advance_to(now).unwrap();
    let request_log = requests.lock().unwrap().clone();
    let window = driver.app().runtime().window(id).unwrap();
    let tree = window.tree();
    let block_states: Vec<_> = tree.media_sources().map(|(node, slot)| (&slot.source, &slot.state, window.geometry(node))).collect();
    let inline_states: Vec<_> =
        tree.inline_media_sources().map(|(node, _, slot)| (&slot.source, &slot.state, window.geometry(node))).collect();
    assert_eq!(request_log.len(), 3, "media source states {block_states:?} {inline_states:?}");
    for part in ["block", "inline"] {
        let url = format!("https://images.test/{part}");
        let log =
            request_log.iter().find(|record| matches!(&record.source, ImageSource::Resolved { uri, .. } if uri.as_ref() == url)).unwrap();
        assert_eq!(log.source.cache_policy(), CachePolicy::MemoryOnly);
        assert!(!log.persistent);
        assert!(!log.cancel.is_cancelled());
    }
    let public = request_log.iter().find(|record| matches!(record.source, ImageSource::Uri(_))).unwrap();
    assert!(public.persistent);

    let (owner, slot_index, _) = tree.inline_media_sources().next().unwrap();
    let paragraph = tree.text(owner).unwrap();
    assert_eq!(&*paragraph.text, "See Map.");
    assert_eq!(paragraph.attachments[slot_index].range, 4..7);
    let link = paragraph.links[0];
    let rect = window.geometry(link).unwrap().rect;
    let position = std::iter::successors(tree.physical_parent(link), |&node| tree.physical_parent(node))
        .filter_map(|node| Some(window.geometry(node)?.rect))
        .fold(Point::new(rect.x + rect.w / 2.0, rect.y + rect.h / 2.0), |point, rect| Point::new(point.x + rect.x, point.y + rect.y));
    let hit = window.hit_test(position).unwrap();
    assert_eq!(hit.hit.node(), Some(link));
    let scroll = tree.test_ids("scroll")[0];
    let pointer = PointerId(1);
    let down = InteractionEvent::PointerDown {
        id: pointer,
        kind: PointerKind::Mouse,
        target: hit,
        pos: position,
        button: Button::Primary,
        mods: Modifiers::empty(),
    };
    let up = InteractionEvent::PointerUp { id: pointer, target: Some(hit), pos: position, mods: Modifiers::empty() };
    driver.dispatch(id, Input::Interaction(down)).unwrap();
    driver.dispatch(id, Input::Interaction(up)).unwrap();
    assert!(
        links
            .lock()
            .unwrap()
            .iter()
            .any(|event| { event.kind == LinkEventKind::Press && &*event.href == "https://example.test/map" && &*event.text == "Map" })
    );

    driver.app_mut().update_window(id, |write| write.scroll_to(scroll, Vec2::new(0.0, 1600.0))).unwrap();
    driver.advance_to(now).unwrap();
    for request in &request_log {
        assert!(request.cancel.is_cancelled());
        assert!(request.dropped.load(Ordering::Acquire));
    }
    let tree = driver.app().runtime().window(id).unwrap().tree();
    assert_eq!(tree.inline_media_source(owner, slot_index).unwrap().state, ImageState::Idle);
}

struct PixelLoader {
    pixels: Image,
    loads: Arc<AtomicUsize>,
}

impl MediaLoader for PixelLoader {
    fn load(&self, _: MediaRequest, _: MediaProgress) -> MediaFuture {
        self.loads.fetch_add(1, Ordering::Relaxed);
        Box::pin(ready(Ok(LoadedMedia { cacheable: false, ..LoadedMedia::pixels(self.pixels.clone()) })))
    }
}

#[test]
fn decoded_inline_image_paints_inside_its_attachment_and_retains_alt_text() {
    let pixels = Image::rgba(2, 1, [0, 0, 255, 255, 255, 0, 0, 255]);
    let loads = Arc::new(AtomicUsize::new(0));
    let mut driver = HeadlessDriver::default();
    driver.app_mut().runtime_mut().set_media_loader(PixelLoader { pixels: pixels.clone(), loads: loads.clone() });
    let metadata = driver.app_mut().build(|build| build.signal(None::<LinkPillContent>)).unwrap();
    let id = driver
        .app_mut()
        .open_window(WindowOptions::default(), move |build| {
            let config = MarkdownConfig::document().link_pill_content(move |read, _| read.get(metadata));
            markdown_with(build, "The [link](/page) and ![chart](https://images.test/chart) explain it.", config)
        })
        .unwrap();
    driver.advance_to(driver.app().runtime().now()).unwrap();
    {
        let tree = driver.app().runtime().window(id).unwrap().tree();
        let (_, index, slot) = tree.inline_media_sources().next().unwrap();
        assert_eq!(index, 0);
        assert_eq!(slot.state, ImageState::Ready { natural_size: (2, 1) });
    }
    driver
        .app_mut()
        .update_window(id, |write| {
            write.set(metadata, Some(LinkPillContent { icon: Some(octara_icons::LINK.into()), ..Default::default() }));
        })
        .unwrap();
    let window = driver.app().runtime().window(id).unwrap();
    let (owner, index, slot) = window.tree().inline_media_sources().next().unwrap();
    assert_eq!(index, 1);
    assert_eq!(loads.load(Ordering::Relaxed), 1);
    assert_eq!(slot.state, ImageState::Ready { natural_size: (2, 1) });
    assert_eq!(&*window.tree().text(owner).unwrap().text, "The link and chart explain it.");
    let draw = window.last_commit().unwrap().draw.as_ref().unwrap();
    assert!(draw.items().iter().any(|item| matches!(item, PaintItem::Image(paint)
        if paint.source == owner && paint.image.source() == &pixels && paint.bounds.w == 24.0 && paint.bounds.h == 20.0)));
}
