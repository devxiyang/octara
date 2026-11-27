//! Native transfer, decoding and pixel variants.

#[cfg(any(target_os = "macos", target_os = "ios"))]
mod apple;
mod disk;
pub use disk::DiskMediaCache;

use octara::runtime::media::{
    CancellationToken, HttpMediaEntry, LoadedMedia, MediaFuture, MediaLoader, MediaProgress, MediaRequest, MediaVariant, probe_image_size,
};
use octara_model::geometry::{Rect, Size};
use octara_model::image::{CachePolicy, Image, ImageSource, MediaAsset};
use reqwest::{Client, StatusCode, Url, header, redirect};
use std::io::Cursor;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::runtime::Runtime;
use tokio::task::JoinHandle;

const MAX_TRANSFER: usize = 64 * 1024 * 1024;

pub(crate) struct NativeMediaLoader;

impl MediaLoader for NativeMediaLoader {
    fn load(&self, request: MediaRequest, progress: MediaProgress) -> MediaFuture {
        Box::pin(async move {
            if let ImageSource::Pixels(image) = request.source {
                return Ok(LoadedMedia::pixels(image));
            }
            run(async move {
                let (bytes, cacheable, cache_for) = match &request.source {
                    ImageSource::Encoded { bytes, .. } => (bytes.clone(), true, None),
                    ImageSource::Uri(uri) => transfer(uri, &[], &request, &progress).await?,
                    ImageSource::Resolved { uri, headers, .. } => transfer(uri, headers, &request, &progress).await?,
                    ImageSource::Pixels(_) => unreachable!(),
                };
                if let Some(size) = probe_image_size(&bytes) {
                    progress.probed(size);
                }
                let cancel = request.cancel;
                let mut decode = AbortOnDrop(tokio::task::spawn_blocking(move || decode(&bytes, &cancel)));
                let image = (&mut decode.0).await.map_err(|_| Arc::from("Image decoder stopped"))??;
                Ok(LoadedMedia { cacheable, cache_for, ..LoadedMedia::pixels(image) })
            })
            .await
        })
    }

    fn has_variants(&self) -> bool {
        true
    }

    fn variant(&self, mut original: LoadedMedia, target: MediaVariant, cancel: CancellationToken) -> MediaFuture {
        Box::pin(async move {
            let MediaAsset::Pixels(image) = &original.asset else { return Ok(original) };
            let image = image.clone();
            original.asset = run(async move {
                let mut resize = AbortOnDrop(tokio::task::spawn_blocking(move || {
                    check_cancel(&cancel)?;
                    let frame = image.frame();
                    let natural = Size::new(frame.width as f32, frame.height as f32);
                    let placed = target.fit.place(natural, Rect::new(0.0, 0.0, target.width as f32, target.height as f32));
                    let scale = (placed.w / natural.width).max(placed.h / natural.height).min(1.0);
                    if scale >= 1.0 || scale <= 0.0 {
                        return Ok(MediaAsset::Pixels(image));
                    }
                    let (width, height) =
                        ((natural.width * scale).round().max(1.0) as u32, (natural.height * scale).round().max(1.0) as u32);
                    let source = image::ImageBuffer::<image::Rgba<u8>, _>::from_raw(frame.width, frame.height, &*frame.rgba)
                        .ok_or_else(|| Arc::from("Invalid decoded image"))?;
                    let resized = image::imageops::resize(&source, width, height, image::imageops::FilterType::Triangle);
                    check_cancel(&cancel)?;
                    Ok(MediaAsset::Pixels(Image::rgba(width, height, resized.into_raw())))
                }));
                (&mut resize.0).await.map_err(|_| Arc::from("Image resize worker stopped"))?
            })
            .await?;
            Ok(original)
        })
    }
}

struct AbortOnDrop<T>(JoinHandle<T>);

impl<T> Drop for AbortOnDrop<T> {
    fn drop(&mut self) {
        self.0.abort();
    }
}

async fn run<T: Send + 'static>(work: impl std::future::Future<Output = Result<T, Arc<str>>> + Send + 'static) -> Result<T, Arc<str>> {
    static RUNTIME: OnceLock<Result<Runtime, Arc<str>>> = OnceLock::new();
    let runtime = RUNTIME
        .get_or_init(|| {
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .thread_name("octara-media")
                .build()
                .map_err(|_| Arc::from("Image worker could not start"))
        })
        .as_ref()
        .map_err(Arc::clone)?;
    let mut job = AbortOnDrop(runtime.spawn(work));
    (&mut job.0).await.map_err(|_| Arc::from("Image worker stopped"))?
}

fn check_cancel(cancel: &CancellationToken) -> Result<(), Arc<str>> {
    if cancel.is_cancelled() { Err(Arc::from("Image request was cancelled")) } else { Ok(()) }
}

fn decode(bytes: &[u8], cancel: &CancellationToken) -> Result<Image, Arc<str>> {
    check_cancel(cancel)?;
    if bytes.len() > MAX_TRANSFER {
        return Err(Arc::from("Encoded image exceeds the byte limit"));
    }
    if image::guess_format(bytes).ok() == Some(image::ImageFormat::Avif) {
        #[cfg(any(target_os = "macos", target_os = "ios"))]
        {
            let decoded = apple::decode_avif(bytes).ok_or_else(|| Arc::from("Unsupported, oversized, or damaged AVIF image"))?;
            check_cancel(cancel)?;
            return Ok(decoded);
        }
        #[cfg(not(any(target_os = "macos", target_os = "ios")))]
        return Err(Arc::from("The built-in decoder on this platform does not support AVIF"));
    }
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(16_384);
    limits.max_image_height = Some(16_384);
    limits.max_alloc = Some(128 * 1024 * 1024);
    let mut reader = image::ImageReader::new(Cursor::new(bytes)).with_guessed_format().map_err(|_| Arc::from("Invalid image header"))?;
    reader.limits(limits);
    let pixels = reader.decode().map_err(|_| Arc::from("Unsupported, oversized, or damaged image"))?.into_rgba8();
    check_cancel(cancel)?;
    let (width, height) = pixels.dimensions();
    Ok(Image::rgba(width, height, pixels.into_raw()))
}

fn client(private: bool) -> Result<&'static Client, Arc<str>> {
    static PUBLIC: OnceLock<Result<Client, Arc<str>>> = OnceLock::new();
    static PRIVATE: OnceLock<Result<Client, Arc<str>>> = OnceLock::new();
    let cell = if private { &PRIVATE } else { &PUBLIC };
    cell.get_or_init(|| {
        Client::builder()
            .redirect(if private { redirect::Policy::none() } else { redirect::Policy::limited(8) })
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(|_| Arc::from("Image HTTP client could not start"))
    })
    .as_ref()
    .map_err(Arc::clone)
}

type Transfer = (Arc<[u8]>, bool, Option<Duration>);

async fn transfer(
    uri: &str,
    headers: &[(Arc<str>, Arc<str>)],
    request: &MediaRequest,
    progress: &MediaProgress,
) -> Result<Transfer, Arc<str>> {
    let url = Url::parse(uri).map_err(|_| Arc::from("Image URI is invalid"))?;
    if !matches!(url.scheme(), "http" | "https") || !url.username().is_empty() || url.password().is_some() {
        return Err(Arc::from("Native image URLs must use HTTP or HTTPS without URL credentials"));
    }
    check_cancel(&request.cancel)?;
    let cache = request.disk_cache.as_ref();
    let cached = if let Some(cache) = cache {
        let cache = cache.clone();
        let uri = uri.to_owned();
        tokio::task::spawn_blocking(move || cache.get(&uri)).await.ok().flatten()
    } else {
        None
    };
    let now = unix_seconds();
    if let Some(entry) = &cached {
        if now < entry.fresh_until {
            return Ok((entry.bytes.clone(), true, Some(Duration::from_secs(entry.fresh_until - now))));
        }
        if now < entry.stale_until {
            // Revalidation is independent of this view because the stale result is already usable.
            // The source is public and a revalidation lease keeps bursts to one request per URI.
            revalidate(url, uri.to_owned(), entry.clone(), request.disk_cache.clone());
            return Ok((entry.bytes.clone(), false, None));
        }
    }
    let (entry, policy) = fetch(url, headers, cached, request.cache_policy(), &request.cancel, progress).await?;
    if let Some(cache) = cache {
        let cache = cache.clone();
        let uri = uri.to_owned();
        let persist = policy.persist && request.cache_policy() == CachePolicy::Standard;
        let entry = entry.clone();
        let _ = tokio::task::spawn_blocking(move || if persist { cache.put(&uri, entry) } else { cache.remove(&uri) }).await;
    }
    let cache_for = policy.max_age.map(|_| Duration::from_secs(policy.fresh.saturating_sub(unix_seconds())));
    Ok((entry.bytes, policy.ram, cache_for))
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

fn http_policy(headers: &header::HeaderMap, now: u64, previous: Option<&HttpMediaEntry>) -> HttpPolicy {
    let mut policy = HttpPolicy {
        max_age: previous.and_then(|entry| entry.max_age),
        stale_window: previous.map_or(0, |entry| entry.stale_while_revalidate),
        ..HttpPolicy::default()
    };
    let mut no_disk = headers.contains_key(header::VARY) || headers.contains_key(header::SET_COOKIE);
    let mut must_validate = false;
    let mut must_revalidate = false;
    if headers.contains_key(header::CACHE_CONTROL) {
        policy.max_age = None;
        policy.stale_window = 0;
    }
    for line in headers.get_all(header::CACHE_CONTROL) {
        let Ok(line) = line.to_str() else { continue };
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
    let age = headers.get(header::AGE).and_then(|age| age.to_str().ok()?.parse::<u64>().ok()).unwrap_or_default();
    let fresh = policy.max_age.unwrap_or_default();
    policy.fresh = now.saturating_add(fresh.saturating_sub(age));
    policy.stale = now.saturating_add(fresh.saturating_add(policy.stale_window).saturating_sub(age));
    let has_validator = headers.contains_key(header::ETAG) || previous.is_some_and(|entry| entry.etag.is_some());
    policy.persist = !no_disk && (has_validator || policy.stale > now);
    policy.ram = !must_validate && (policy.max_age.is_some() || !policy.persist);
    policy
}

async fn fetch(
    url: Url,
    headers: &[(Arc<str>, Arc<str>)],
    previous: Option<HttpMediaEntry>,
    cache: CachePolicy,
    cancel: &CancellationToken,
    progress: &MediaProgress,
) -> Result<(HttpMediaEntry, HttpPolicy), Arc<str>> {
    let mut request = client(!headers.is_empty())?.get(url);
    if cache != CachePolicy::Standard {
        request = request.header(header::CACHE_CONTROL, if cache == CachePolicy::NoCache { "no-store, no-cache" } else { "no-store" });
    }
    if let Some(etag) = previous.as_ref().and_then(|entry| entry.etag.as_deref()) {
        request = request.header(header::IF_NONE_MATCH, etag);
    }
    for (name, value) in headers {
        request = request.header(name.as_ref(), value.as_ref());
    }
    let mut response = request.send().await.map_err(|_| Arc::from("Image HTTP request failed"))?;
    let now = unix_seconds();
    if response.status() == StatusCode::NOT_MODIFIED
        && let Some(mut entry) = previous
    {
        let policy = http_policy(response.headers(), now, Some(&entry));
        entry.fresh_until = policy.fresh;
        entry.stale_until = policy.stale;
        entry.max_age = policy.max_age;
        entry.stale_while_revalidate = policy.stale_window;
        if let Some(etag) = response.headers().get(header::ETAG).and_then(|value| value.to_str().ok()) {
            entry.etag = Some(Arc::from(etag));
        }
        return Ok((entry, policy));
    }
    if !response.status().is_success() {
        return Err(Arc::from(format!("Image HTTP request returned {}", response.status().as_u16())));
    }
    if response.content_length().is_some_and(|len| len > MAX_TRANSFER as u64) {
        return Err(Arc::from("Image response exceeds the byte limit"));
    }
    let policy = http_policy(response.headers(), now, None);
    let metadata = |name| response.headers().get(name).and_then(|text| text.to_str().ok()).map(Arc::from);
    let mime = metadata(header::CONTENT_TYPE);
    let etag = metadata(header::ETAG);
    let mut bytes = Vec::new();
    let mut did_probe = false;
    while let Some(chunk) = response.chunk().await.map_err(|_| Arc::from("Image transfer ended early"))? {
        check_cancel(cancel)?;
        if bytes.len().saturating_add(chunk.len()) > MAX_TRANSFER {
            return Err(Arc::from("Image response exceeds the byte limit"));
        }
        let probe = !did_probe && bytes.len() < 16 * 1024;
        bytes.extend_from_slice(&chunk);
        if probe && let Some(size) = probe_image_size(&bytes) {
            progress.probed(size);
            did_probe = true;
        }
    }
    let entry = HttpMediaEntry {
        bytes: bytes.into(),
        mime,
        etag,
        fresh_until: policy.fresh,
        stale_until: policy.stale,
        max_age: policy.max_age,
        stale_while_revalidate: policy.stale_window,
    };
    Ok((entry, policy))
}

fn revalidate(url: Url, uri: String, entry: HttpMediaEntry, backend: Option<Arc<dyn octara::MediaCacheBackend>>) {
    use std::collections::HashSet;
    use std::sync::Mutex;
    static REVALIDATING: OnceLock<Mutex<HashSet<(usize, String)>>> = OnceLock::new();
    let Some(backend) = backend else { return };
    let active = REVALIDATING.get_or_init(Mutex::default);
    let id = (Arc::as_ptr(&backend) as *const () as usize, uri.clone());
    {
        let mut active = active.lock().expect("media revalidation");
        if active.len() >= 4 || !active.insert(id.clone()) {
            return;
        }
    }
    tokio::spawn(async move {
        let result = fetch(url, &[], Some(entry), CachePolicy::Standard, &CancellationToken::default(), &MediaProgress::default()).await;
        if let Ok((entry, policy)) = result {
            let key = uri.clone();
            let _ = tokio::task::spawn_blocking(move || if policy.persist { backend.put(&key, entry) } else { backend.remove(&key) }).await;
        }
        active.lock().expect("media revalidation").remove(&id);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let mut headers = header::HeaderMap::new();
        let renewed = http_policy(&headers, 101, Some(&previous));
        assert!(renewed.persist && renewed.ram);
        assert_eq!((renewed.fresh, renewed.stale), (151, 251));
        headers.insert(header::CACHE_CONTROL, "private, max-age=60".parse().unwrap());
        let private = http_policy(&headers, 101, Some(&previous));
        assert!(!private.persist && private.ram);
        headers.insert(header::CACHE_CONTROL, "no-store, max-age=60".parse().unwrap());
        let no_store = http_policy(&headers, 101, Some(&previous));
        assert!(!no_store.persist && !no_store.ram);
        headers.insert(header::CACHE_CONTROL, "no-cache, max-age=60, stale-while-revalidate=100".parse().unwrap());
        let no_cache = http_policy(&headers, 101, Some(&previous));
        assert!(no_cache.persist && !no_cache.ram);
        assert_eq!((no_cache.fresh, no_cache.stale), (101, 101));
    }
}
