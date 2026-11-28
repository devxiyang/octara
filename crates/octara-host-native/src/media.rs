//! Native media transfer, disk revalidation, and pixel variants.

mod disk;
mod transport;
mod worker;

pub use disk::DiskMediaCache;
pub use transport::{MediaCancelRegistration, MediaFetchRequest, MediaFetchResponse, MediaTransport};

use octara::runtime::media::{
    CancellationToken, HttpMediaEntry, LoadedMedia, MediaFuture, MediaLoader, MediaProgress, MediaRequest, MediaVariant, probe_image_size,
};
use octara_model::geometry::{Rect, Size};
use octara_model::image::{CachePolicy, Image, ImageFrame, ImageSource, MediaAsset};
use std::collections::HashSet;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use transport::MediaCancellation;
use worker::Pool;

type Failure = Arc<str>;
const MAX_TRANSFER: usize = 64 * 1024 * 1024;

pub(crate) struct NativeMediaLoader {
    setup: fn(),
    pool: OnceLock<Result<Arc<Pool>, Failure>>,
    transport: Option<Arc<dyn MediaTransport>>,
}

impl NativeMediaLoader {
    pub(crate) fn new(setup: fn(), transport: Option<Arc<dyn MediaTransport>>) -> Self {
        #[cfg(any(target_os = "macos", target_os = "ios"))]
        let transport = transport.or_else(|| Some(Arc::new(crate::os::apple_transport::AppleMediaTransport)));
        Self { setup, pool: OnceLock::new(), transport }
    }

    fn run(&self, work: impl FnOnce(&Pool, &MediaCancellation) -> Result<LoadedMedia, Failure> + Send + 'static) -> MediaFuture {
        let pool = self.pool.get_or_init(|| Pool::new(self.setup).map(Arc::new)).clone();
        Box::pin(async move {
            let pool = pool?;
            let cancel = MediaCancellation::default();
            let abort = CancelOnDrop(cancel.clone());
            let active_pool = pool.clone();
            let mut receive = pool.submit(move || {
                check_cancel(&cancel)?;
                work(&active_pool, &cancel)
            })?;
            let output = receive.recv().await.ok_or_else(|| Arc::from("Image worker stopped"))?;
            drop(abort);
            output
        })
    }
}

impl MediaLoader for NativeMediaLoader {
    fn load(&self, request: MediaRequest, progress: MediaProgress) -> MediaFuture {
        match request.source {
            ImageSource::Pixels(image) => return Box::pin(std::future::ready(Ok(LoadedMedia::pixels(image)))),
            ImageSource::External(texture) => return Box::pin(std::future::ready(Ok(LoadedMedia::external(texture)))),
            _ => {}
        }
        let transport = self.transport.clone();
        self.run(move |pool, cancel| {
            check_request_cancel(&request.cancel, cancel)?;
            let (bytes, cacheable, cache_for) = match &request.source {
                ImageSource::Encoded { bytes, .. } => (bytes.clone(), true, None),
                ImageSource::File(path) => (read_local(path, cancel)?, true, None),
                ImageSource::Uri(uri) => transfer(uri, &[], &request, &progress, transport.as_ref(), pool, cancel)?,
                ImageSource::Resolved { uri, headers, .. } => {
                    transfer(uri, headers, &request, &progress, transport.as_ref(), pool, cancel)?
                }
                ImageSource::Pixels(_) | ImageSource::External(_) => unreachable!(),
            };
            check_request_cancel(&request.cancel, cancel)?;
            if bytes.len() > MAX_TRANSFER {
                return Err(Arc::from("Encoded image exceeds the byte limit"));
            }
            if let Some(size) = probe_image_size(&bytes) {
                progress.probed(size);
            }
            let image = decode(&bytes)?;
            check_request_cancel(&request.cancel, cancel)?;
            Ok(LoadedMedia { cacheable, cache_for, ..LoadedMedia::pixels(image) })
        })
    }

    fn has_variants(&self) -> bool {
        true
    }

    fn variant(&self, mut original: LoadedMedia, target: MediaVariant, external: CancellationToken) -> MediaFuture {
        let MediaAsset::Pixels(image) = &original.asset else {
            return Box::pin(std::future::ready(Ok(original)));
        };
        let image = image.clone();
        self.run(move |_, cancel| {
            check_request_cancel(&external, cancel)?;
            let frame = image.frame();
            let natural = Size::new(frame.width as f32, frame.height as f32);
            let placed = target.fit.place(natural, Rect::new(0.0, 0.0, target.width as f32, target.height as f32));
            let scale = (placed.w / natural.width).max(placed.h / natural.height).min(1.0);
            if scale > 0.0 && scale < 1.0 {
                let width = (natural.width * scale).round().max(1.0) as u32;
                let height = (natural.height * scale).round().max(1.0) as u32;
                let resized = resize(&frame, width, height)?;
                check_request_cancel(&external, cancel)?;
                original.asset = MediaAsset::Pixels(resized);
            }
            Ok(original)
        })
    }
}

struct CancelOnDrop(MediaCancellation);

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.cancel();
    }
}

fn check_cancel(cancel: &MediaCancellation) -> Result<(), Failure> {
    if cancel.flag().load(Ordering::Acquire) { Err(Arc::from("Image request was cancelled")) } else { Ok(()) }
}

fn check_request_cancel(external: &CancellationToken, cancel: &MediaCancellation) -> Result<(), Failure> {
    if external.is_cancelled() {
        cancel.cancel();
    }
    check_cancel(cancel)
}

#[cfg(any(target_os = "macos", target_os = "ios"))]
fn decode(bytes: &[u8]) -> Result<Image, Failure> {
    crate::os::apple_media::decode(bytes).map_err(Arc::from)
}

#[cfg(any(target_os = "macos", target_os = "ios"))]
fn resize(frame: &ImageFrame, width: u32, height: u32) -> Result<Image, Failure> {
    crate::os::apple_media::resize(frame, width, height).map_err(Arc::from)
}

#[cfg(not(any(target_os = "macos", target_os = "ios")))]
fn decode(bytes: &[u8]) -> Result<Image, Failure> {
    if image::guess_format(bytes).ok() == Some(image::ImageFormat::Avif) {
        return Err(Arc::from("The built-in decoder on this platform does not support AVIF"));
    }
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(16_384);
    limits.max_image_height = Some(16_384);
    limits.max_alloc = Some(128 * 1024 * 1024);
    let mut reader =
        image::ImageReader::new(std::io::Cursor::new(bytes)).with_guessed_format().map_err(|_| Arc::from("Invalid image header"))?;
    reader.limits(limits);
    let pixels = reader.decode().map_err(|_| Arc::from("Unsupported, oversized, or damaged image"))?.into_rgba8();
    let (width, height) = pixels.dimensions();
    Ok(Image::rgba(width, height, pixels.into_raw()))
}

#[cfg(not(any(target_os = "macos", target_os = "ios")))]
fn resize(frame: &ImageFrame, width: u32, height: u32) -> Result<Image, Failure> {
    let source = image::ImageBuffer::<image::Rgba<u8>, _>::from_raw(frame.width, frame.height, &*frame.rgba)
        .ok_or_else(|| Arc::from("Invalid decoded image"))?;
    let pixels = image::imageops::resize(&source, width, height, image::imageops::FilterType::Triangle);
    Ok(Image::rgba(width, height, pixels.into_raw()))
}

fn read_local(path: &std::path::Path, cancel: &MediaCancellation) -> Result<Arc<[u8]>, Failure> {
    use std::io::Read;

    #[cfg(unix)]
    let opened = {
        use rustix::fs::{Mode, OFlags, open};
        open(path, OFlags::RDONLY | OFlags::NONBLOCK | OFlags::CLOEXEC, Mode::empty()).map(std::fs::File::from)
    };
    #[cfg(not(unix))]
    let opened = std::fs::File::open(path);
    let mut file = opened.map_err(|_| Arc::from("Image file is unavailable"))?;
    let metadata = file.metadata().map_err(|_| Arc::from("Image file is unavailable"))?;
    if !metadata.is_file() {
        return Err(Arc::from("Image path must refer to a regular file"));
    }
    if metadata.len() > MAX_TRANSFER as u64 {
        return Err(Arc::from("Encoded image exceeds the byte limit"));
    }
    let mut bytes = Vec::new();
    let mut chunk = [0; 16 * 1024];
    loop {
        check_cancel(cancel)?;
        let read = file.read(&mut chunk).map_err(|_| Arc::from("Image file could not be read"))?;
        if read == 0 {
            return Ok(bytes.into());
        }
        if bytes.len() + read > MAX_TRANSFER {
            return Err(Arc::from("Encoded image exceeds the byte limit"));
        }
        bytes.extend_from_slice(&chunk[..read]);
    }
}

type Transfer = (Arc<[u8]>, bool, Option<Duration>);

fn transfer(
    uri: &Arc<str>,
    headers: &[(Arc<str>, Arc<str>)],
    request: &MediaRequest,
    progress: &MediaProgress,
    transport: Option<&Arc<dyn MediaTransport>>,
    pool: &Pool,
    cancel: &MediaCancellation,
) -> Result<Transfer, Failure> {
    if !is_http_image_uri(uri) {
        return Err(Arc::from("Native image URLs must use HTTP or HTTPS without URL credentials"));
    }

    let cache = request.disk_cache.as_ref().filter(|_| headers.is_empty() && request.cache_policy() == CachePolicy::Standard);
    let cached = cache.and_then(|cache| cache.get(uri));
    check_request_cancel(&request.cancel, cancel)?;
    let now = unix_seconds();
    if let Some(entry) = &cached {
        if now < entry.fresh_until {
            return Ok((entry.bytes.clone(), true, Some(Duration::from_secs(entry.fresh_until - now))));
        }
        if now < entry.stale_until {
            if let (Some(cache), Some(transport)) = (cache, transport) {
                revalidate(pool, uri.clone(), entry.clone(), cache.clone(), transport.clone());
            }
            return Ok((entry.bytes.clone(), false, None));
        }
    }
    let transport = transport.ok_or_else(|| Arc::from("HTTP image loading requires a native media transport"))?;
    let request = fetch_request(uri.clone(), headers, request.cache_policy(), cached.as_ref(), progress.clone(), cancel.clone());
    let (entry, policy) = fetch(transport.as_ref(), request, cached.as_ref())?;
    check_cancel(cancel)?;
    if let Some(cache) = cache {
        update_cache(cache.as_ref(), uri, cached.as_ref(), &entry, &policy);
    }
    let cache_for = policy.max_age.map(|_| Duration::from_secs(policy.fresh.saturating_sub(unix_seconds())));
    Ok((entry.bytes, policy.ram, cache_for))
}

fn is_http_image_uri(uri: &str) -> bool {
    let Some((scheme, rest)) = uri.split_once("://") else { return false };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    (scheme.eq_ignore_ascii_case("http") || scheme.eq_ignore_ascii_case("https"))
        && !authority.is_empty()
        && !authority.contains(['@', '\\'])
        && !authority.chars().any(|ch| ch.is_whitespace() || ch.is_control())
}

fn fetch_request(
    uri: Arc<str>,
    authored: &[(Arc<str>, Arc<str>)],
    cache: CachePolicy,
    previous: Option<&HttpMediaEntry>,
    progress: MediaProgress,
    cancellation: MediaCancellation,
) -> MediaFetchRequest {
    let mut request = MediaFetchRequest::new(uri, MAX_TRANSFER);
    request.follow_redirects = authored.is_empty();
    request.progress = progress;
    request.cancellation = cancellation;
    if cache != CachePolicy::Standard {
        let value = if cache == CachePolicy::NoCache { "no-store, no-cache" } else { "no-store" };
        request.headers.push((Arc::from("Cache-Control"), Arc::from(value)));
    }
    if let Some(etag) = previous.and_then(|entry| entry.etag.as_ref()) {
        request.headers.push((Arc::from("If-None-Match"), etag.clone()));
    }
    request.headers.extend_from_slice(authored);
    request
}

fn unix_seconds() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs()
}

#[derive(Default)]
struct HttpPolicy {
    persist: bool,
    ram: bool,
    max_age: Option<u64>,
    stale_window: u64,
    fresh: u64,
    stale: u64,
}

type Headers = [(Arc<str>, Arc<str>)];

fn header<'a>(headers: &'a Headers, key: &str) -> Option<&'a Arc<str>> {
    headers.iter().find(|(name, _)| name.eq_ignore_ascii_case(key)).map(|(_, value)| value)
}

fn http_policy(headers: &Headers, now: u64, previous: Option<&HttpMediaEntry>) -> HttpPolicy {
    let mut policy = HttpPolicy {
        max_age: previous.and_then(|entry| entry.max_age),
        stale_window: previous.map_or(0, |entry| entry.stale_while_revalidate),
        ..HttpPolicy::default()
    };
    let mut no_disk = header(headers, "Vary").is_some() || header(headers, "Set-Cookie").is_some();
    let mut must_validate = false;
    let mut must_revalidate = false;
    if header(headers, "Cache-Control").is_some() {
        policy.max_age = None;
        policy.stale_window = 0;
    }
    for (_, line) in headers.iter().filter(|(name, _)| name.eq_ignore_ascii_case("Cache-Control")) {
        for directive in line.split(',').map(str::trim) {
            let (name, value) = directive.split_once('=').unwrap_or((directive, ""));
            let seconds = value.trim().trim_matches('"').parse::<u64>().ok();
            match name.trim().to_ascii_lowercase().as_str() {
                "private" => no_disk = true,
                "no-store" => return HttpPolicy::default(),
                "no-cache" => must_validate = true,
                "must-revalidate" => must_revalidate = true,
                "max-age" => policy.max_age = seconds,
                "stale-while-revalidate" => policy.stale_window = seconds.unwrap_or_default(),
                _ => {}
            }
        }
    }
    if must_validate {
        policy.max_age = Some(0);
    }
    if must_validate || must_revalidate {
        policy.stale_window = 0;
    }
    let age = header(headers, "Age").and_then(|age| age.parse::<u64>().ok()).unwrap_or_default();
    let fresh = policy.max_age.unwrap_or_default();
    policy.fresh = now.saturating_add(fresh.saturating_sub(age));
    policy.stale = now.saturating_add(fresh.saturating_add(policy.stale_window).saturating_sub(age));
    let has_validator = header(headers, "ETag").is_some() || previous.is_some_and(|entry| entry.etag.is_some());
    policy.persist = !no_disk && (has_validator || policy.stale > now);
    policy.ram = !must_validate && (policy.max_age.is_some() || !policy.persist);
    policy
}

fn fetch(
    transport: &dyn MediaTransport,
    request: MediaFetchRequest,
    previous: Option<&HttpMediaEntry>,
) -> Result<(HttpMediaEntry, HttpPolicy), Failure> {
    let cancel = request.cancellation.clone();
    check_cancel(&cancel)?;
    let response = transport.fetch(request, cancel.flag())?;
    check_cancel(&cancel)?;
    let now = unix_seconds();
    if response.status == 304
        && let Some(previous) = previous.filter(|entry| entry.etag.is_some())
    {
        let policy = http_policy(&response.headers, now, Some(previous));
        let mut entry = previous.clone();
        entry.fresh_until = policy.fresh;
        entry.stale_until = policy.stale;
        entry.max_age = policy.max_age;
        entry.stale_while_revalidate = policy.stale_window;
        if let Some(etag) = header(&response.headers, "ETag") {
            entry.etag = Some(etag.clone());
        }
        return Ok((entry, policy));
    }
    if !(200..300).contains(&response.status) {
        return Err(Arc::from(format!("Image HTTP request returned {}", response.status)));
    }
    if response.bytes.len() > MAX_TRANSFER {
        return Err(Arc::from("Image response exceeds the byte limit"));
    }
    let policy = http_policy(&response.headers, now, None);
    let entry = HttpMediaEntry {
        bytes: response.bytes,
        mime: header(&response.headers, "Content-Type").cloned(),
        etag: header(&response.headers, "ETag").cloned(),
        fresh_until: policy.fresh,
        stale_until: policy.stale,
        max_age: policy.max_age,
        stale_while_revalidate: policy.stale_window,
    };
    Ok((entry, policy))
}

fn update_cache(
    cache: &dyn octara::MediaCacheBackend,
    uri: &str,
    expected: Option<&HttpMediaEntry>,
    entry: &HttpMediaEntry,
    policy: &HttpPolicy,
) {
    use std::hash::{BuildHasher, BuildHasherDefault, DefaultHasher};

    static WRITES: [Mutex<()>; 16] = [const { Mutex::new(()) }; 16];
    let slot = BuildHasherDefault::<DefaultHasher>::new().hash_one(uri) as usize % WRITES.len();
    let _write = WRITES[slot].lock().expect("media cache write");
    // Stale validation can finish after a newer load has already replaced this entry.
    if cache.get(uri).as_ref() != expected {
        return;
    }
    if policy.persist {
        cache.put(uri, entry.clone());
    } else {
        cache.remove(uri);
    }
}

type RevalidationKey = (usize, Arc<str>);
static REVALIDATING: Mutex<Option<HashSet<RevalidationKey>>> = Mutex::new(None);

struct Revalidation(RevalidationKey);

impl Revalidation {
    fn acquire(cache: &Arc<dyn octara::MediaCacheBackend>, uri: Arc<str>) -> Option<Self> {
        let key = (Arc::as_ptr(cache) as *const () as usize, uri);
        let mut active = REVALIDATING.lock().expect("media revalidation");
        let active = active.get_or_insert_with(HashSet::new);
        (active.len() < 4 && active.insert(key.clone())).then(|| Self(key))
    }
}

impl Drop for Revalidation {
    fn drop(&mut self) {
        if let Some(active) = REVALIDATING.lock().expect("media revalidation").as_mut() {
            active.remove(&self.0);
        }
    }
}

fn revalidate(
    pool: &Pool,
    uri: Arc<str>,
    entry: HttpMediaEntry,
    cache: Arc<dyn octara::MediaCacheBackend>,
    transport: Arc<dyn MediaTransport>,
) {
    let Some(lease) = Revalidation::acquire(&cache, uri.clone()) else { return };
    pool.maintain(move || {
        let _lease = lease;
        let request =
            fetch_request(uri.clone(), &[], CachePolicy::Standard, Some(&entry), MediaProgress::default(), MediaCancellation::default());
        if let Ok((response, policy)) = fetch(transport.as_ref(), request, Some(&entry)) {
            update_cache(cache.as_ref(), &uri, Some(&entry), &response, &policy);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use octara::MediaCacheBackend;
    use std::sync::atomic::{AtomicBool, AtomicUsize};

    const PNG: &[u8] = include_bytes!("os/apple_media/fixtures/sample.png");

    struct TestDirectory(std::path::PathBuf);

    impl TestDirectory {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "octara-media-test-{}-{}",
                std::process::id(),
                SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos(),
            ));
            std::fs::create_dir(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn private_and_no_store_cannot_retain_a_previous_validator() {
        let previous = HttpMediaEntry {
            bytes: [42].into(),
            mime: None,
            etag: Some(r#""original""#.into()),
            fresh_until: 100,
            stale_until: 200,
            max_age: Some(50),
            stale_while_revalidate: 100,
        };
        let renewed = http_policy(&[], 101, Some(&previous));
        assert!(renewed.persist && renewed.ram);
        assert_eq!((renewed.fresh, renewed.stale), (151, 251));
        let policy = |line: &'static str| http_policy(&[("Cache-Control".into(), line.into())], 101, Some(&previous));
        let private = policy("private, max-age=60");
        assert!(!private.persist && private.ram);
        let no_store = policy("no-store, max-age=60");
        assert!(!no_store.persist && !no_store.ram);
        let no_cache = policy("no-cache, max-age=60, stale-while-revalidate=100");
        assert!(no_cache.persist && !no_cache.ram);
        assert_eq!((no_cache.fresh, no_cache.stale), (101, 101));
    }

    #[test]
    fn injected_transport_revalidates_cached_pixels_and_preserves_disk_freshness() {
        #[derive(Default)]
        struct Conditional(AtomicUsize);

        impl MediaTransport for Conditional {
            fn fetch(&self, request: MediaFetchRequest, cancel: &AtomicBool) -> Result<MediaFetchResponse, Failure> {
                assert!(!cancel.load(Ordering::Acquire));
                assert_eq!(request.max_bytes, MAX_TRANSFER);
                assert!(request.follow_redirects);
                assert!(std::thread::current().name().unwrap().starts_with("octara-media-"));
                let validator = header(&request.headers, "If-None-Match").map(AsRef::as_ref);
                match self.0.fetch_add(1, Ordering::Relaxed) {
                    0 => {
                        assert_eq!(validator, None);
                        Ok(MediaFetchResponse {
                            status: 200,
                            headers: vec![("ETag".into(), "\"sample\"".into()), ("cache-control".into(), "max-age=0".into())],
                            bytes: PNG.into(),
                        })
                    }
                    1 => {
                        assert_eq!(validator, Some("\"sample\""));
                        Ok(MediaFetchResponse {
                            status: 304,
                            headers: vec![("Cache-Control".into(), "max-age=60".into())],
                            bytes: Arc::from([]),
                        })
                    }
                    _ => panic!("fresh disk entry should be used without another transfer"),
                }
            }
        }

        let folder = TestDirectory::new();
        let cache = Arc::new(DiskMediaCache::new(&folder.0));
        let transport = Arc::new(Conditional::default());
        let loader = NativeMediaLoader::new(|| {}, Some(transport.clone()));
        let uri: Arc<str> = "https://example.test/pixels.png".into();
        let load = || {
            let request = MediaRequest {
                source: ImageSource::Uri(uri.clone()),
                cancel: CancellationToken::default(),
                disk_cache: Some(cache.clone()),
            };
            octara_render::block_on(loader.load(request, MediaProgress::default())).unwrap()
        };
        let first = load();
        assert_eq!(first.cache_for, Some(Duration::ZERO));
        let superseded = cache.get(&uri).unwrap();
        let renewed = load();
        assert!(renewed.cache_for.unwrap() >= Duration::from_secs(55));
        assert_eq!(first.natural_size, renewed.natural_size);
        let fresh = load();
        assert_eq!(fresh.natural_size, renewed.natural_size);
        assert_eq!(transport.0.load(Ordering::Relaxed), 2);
        assert_eq!(&*cache.get(&uri).unwrap().bytes, PNG);
        for persist in [true, false] {
            let policy = HttpPolicy { persist, ..Default::default() };
            update_cache(cache.as_ref(), &uri, Some(&superseded), &superseded, &policy);
            assert_eq!(cache.get(&uri).unwrap().max_age, Some(60));
        }
    }

    #[test]
    fn only_application_file_sources_can_read_local_images() {
        let folder = TestDirectory::new();
        let path = folder.0.join("image.png");
        std::fs::write(&path, PNG).unwrap();
        let loader = NativeMediaLoader::new(|| {}, None);
        let load = |source| {
            let request = MediaRequest { source, cancel: CancellationToken::default(), disk_cache: None };
            octara_render::block_on(loader.load(request, MediaProgress::default()))
        };
        let image = load(ImageSource::File(path.clone().into())).unwrap();
        assert_eq!(image.natural_size, probe_image_size(PNG).unwrap());
        for resource in [path.display().to_string(), format!("file://{}", path.display())] {
            assert!(load(ImageSource::Uri(resource.into())).is_err());
        }
    }

    #[test]
    fn dropping_a_pending_load_interrupts_the_transport() {
        use std::future::Future;
        use std::sync::mpsc;
        use std::task::{Context, Poll, Waker};

        struct StalledTransport {
            started: mpsc::Sender<()>,
            interrupted: Arc<AtomicBool>,
        }

        impl MediaTransport for StalledTransport {
            fn fetch(&self, request: MediaFetchRequest, cancel: &AtomicBool) -> Result<MediaFetchResponse, Failure> {
                let (signal, receive) = mpsc::channel();
                let interrupted = self.interrupted.clone();
                let _registration = request.on_cancel(move || {
                    interrupted.store(true, Ordering::Release);
                    let _ = signal.send(());
                });
                self.started.send(()).unwrap();
                receive.recv_timeout(Duration::from_secs(5)).unwrap();
                assert!(cancel.load(Ordering::Acquire));
                Err(Arc::from("Image request was cancelled"))
            }
        }

        let (started, receive) = mpsc::channel();
        let interrupted = Arc::new(AtomicBool::new(false));
        let loader = NativeMediaLoader::new(|| {}, Some(Arc::new(StalledTransport { started, interrupted: interrupted.clone() })));
        let request = MediaRequest {
            source: ImageSource::Uri("https://example.test/stalled.png".into()),
            cancel: CancellationToken::default(),
            disk_cache: None,
        };
        let mut load = loader.load(request, MediaProgress::default());
        assert!(matches!(Future::poll(load.as_mut(), &mut Context::from_waker(Waker::noop())), Poll::Pending));
        receive.recv_timeout(Duration::from_secs(5)).unwrap();
        drop(load);
        assert!(interrupted.load(Ordering::Acquire));
    }
}
