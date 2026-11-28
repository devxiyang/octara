//! Browser-native URI images, abortable Fetch, and browser-side decoding.

use js_sys::{Array, Promise, Uint8Array};
use octara::runtime::media::{LoadedMedia, MediaFuture, MediaLoader, MediaProgress, MediaRequest, probe_image_size};
use octara_model::image::{CachePolicy, Image, ImageSource, MediaAsset};
use std::sync::Arc;
use std::time::Duration;
use wasm_bindgen::{Clamped, JsCast, JsValue, closure::Closure, prelude::wasm_bindgen};
use wasm_bindgen_futures::JsFuture;
use web_sys::{AbortController, AbortSignal, Blob, BlobPropertyBag, CanvasRenderingContext2d, HtmlCanvasElement, HtmlImageElement, Url};

#[wasm_bindgen(module = "/src/media.js")]
extern "C" {
    #[wasm_bindgen(js_name = fetchImage)]
    fn fetch_image(uri: &str, headers: &Array, cache: &str, signal: &AbortSignal, probe: &js_sys::Function) -> Promise;
}

pub(crate) struct BrowserMediaLoader;

impl MediaLoader for BrowserMediaLoader {
    fn load(&self, request: MediaRequest, progress: MediaProgress) -> MediaFuture {
        Box::pin(async move {
            let policy = request.cache_policy();
            let result = match request.source {
                ImageSource::Pixels(image) => return Ok(LoadedMedia::pixels(image)),
                ImageSource::External(handle) => {
                    return Ok(LoadedMedia {
                        natural_size: handle.size(),
                        asset: MediaAsset::External(handle),
                        cacheable: false,
                        cache_for: None,
                    });
                }
                ImageSource::Uri(uri) => direct(&uri).await,
                ImageSource::File(_) => return Err(Arc::from("Local image files require a native host")),
                ImageSource::Encoded { bytes, mime } => {
                    if bytes.len() > 64 * 1024 * 1024 {
                        return Err(Arc::from("Encoded image exceeds the byte limit"));
                    }
                    if let Some(size) = probe_image_size(&bytes) {
                        progress.probed(size);
                    }
                    let data = Array::of1(&Uint8Array::from(&*bytes));
                    let options = BlobPropertyBag::new();
                    if let Some(mime) = mime {
                        options.set_type(&mime);
                    }
                    let blob = Blob::new_with_u8_array_sequence_and_options(&data, &options);
                    match blob {
                        Ok(blob) => decoded(&blob).await,
                        Err(error) => Err(error),
                    }
                }
                ImageSource::Resolved { uri, headers, .. } => {
                    let Ok(uri) = browser_uri(&uri, false) else { return Err(Arc::from("Image URI is invalid")) };
                    let headers: Array =
                        headers.iter().map(|(key, value)| Array::of2(&JsValue::from_str(key), &JsValue::from_str(value))).collect();
                    let callback = Closure::<dyn Fn(Uint8Array) -> bool>::new(move |head: Uint8Array| {
                        let Some(size) = probe_image_size(&head.to_vec()) else { return false };
                        progress.probed(size);
                        true
                    });
                    let abort = AbortOnDrop(AbortController::new().map_err(|_| Arc::from("Image request could not start"))?);
                    let cache = if policy == CachePolicy::Standard { "default" } else { "no-store" };
                    let result = JsFuture::from(fetch_image(&uri, &headers, cache, &abort.0.signal(), callback.as_ref().unchecked_ref()))
                        .await
                        .and_then(JsCast::dyn_into::<Array>);
                    match result {
                        Ok(response) => {
                            let blob = response.get(0).dyn_into::<Blob>().map_err(|_| Arc::from("Invalid image response"))?;
                            decoded(&blob).await.map(|mut image| {
                                image.cacheable = response.get(1).as_bool().unwrap_or(false);
                                image.cache_for = response.get(2).as_f64().and_then(|age| Duration::try_from_secs_f64(age).ok());
                                image
                            })
                        }
                        Err(error) => Err(error),
                    }
                }
            };
            result.map_err(|_| Arc::from("Image could not be loaded or decoded"))
        })
    }
}

struct AbortOnDrop(AbortController);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

struct ImageRequest(HtmlImageElement);

impl Drop for ImageRequest {
    fn drop(&mut self) {
        self.0.remove_attribute("src").ok();
    }
}

struct BlobUrl(String);

impl Drop for BlobUrl {
    fn drop(&mut self) {
        Url::revoke_object_url(&self.0).ok();
    }
}

pub(crate) fn browser_uri(uri: &str, image: bool) -> Result<String, JsValue> {
    let base = web_sys::window().and_then(|win| win.document()).ok_or("No browser document")?.base_uri()?;
    let url = Url::new_with_base(uri, &base.unwrap_or_default())?;
    let protocol = url.protocol();
    if !matches!(protocol.as_str(), "http:" | "https:" | "blob:")
        && !(image && protocol == "data:" && url.href().starts_with("data:image/"))
    {
        return Err("Unsupported media URI scheme".into());
    }
    Ok(url.href())
}

async fn direct(uri: &str) -> Result<LoadedMedia, JsValue> {
    let uri = browser_uri(uri, true)?;
    let image = load_element(&uri).await?;
    let natural_size = (image.0.natural_width(), image.0.natural_height());
    Ok(LoadedMedia { asset: MediaAsset::BrowserUri(uri.into()), natural_size, cacheable: false, cache_for: None })
}

async fn load_element(src: &str) -> Result<ImageRequest, JsValue> {
    let image = ImageRequest(HtmlImageElement::new()?);
    image.0.set_attribute("decoding", "async")?;
    image.0.set_src(src);
    JsFuture::from(image.0.decode()).await?;
    Ok(image)
}

async fn decoded(blob: &Blob) -> Result<LoadedMedia, JsValue> {
    let url = BlobUrl(Url::create_object_url_with_blob(blob)?);
    let image = load_element(&url.0).await?;
    let document = web_sys::window().and_then(|win| win.document()).ok_or("No browser document")?;
    let canvas = document.create_element("canvas")?.dyn_into::<HtmlCanvasElement>()?;
    let (width, height) = (image.0.natural_width(), image.0.natural_height());
    if width == 0 || height == 0 || u64::from(width) * u64::from(height) > 32 * 1024 * 1024 {
        return Err("Unsupported image dimensions".into());
    }
    canvas.set_width(width);
    canvas.set_height(height);
    let context = canvas.get_context("2d")?.ok_or("No image canvas context")?.dyn_into::<CanvasRenderingContext2d>()?;
    context.draw_image_with_html_image_element(&image.0, 0.0, 0.0)?;
    let Clamped(bytes) = context.get_image_data(0.0, 0.0, width.into(), height.into())?.data();
    Ok(LoadedMedia::pixels(Image::rgba(width, height, bytes)))
}
