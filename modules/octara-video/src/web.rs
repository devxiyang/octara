use crate::{VideoCommand, VideoSource, snapshot_unavailable};
use octara::module::{CallTicket, HostSurface, ModuleDispatcher, ModuleError, NativePlacement, NativeSnapshotRequest, NativeViewInstance};
use std::any::Any;
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::{JsFuture, spawn_local};
use web_sys::{HtmlElement, HtmlVideoElement, Url};

pub(super) struct Video {
    element: HtmlVideoElement,
    visible: bool,
}

impl Video {
    pub(super) fn new(surface: HostSurface<'_>, source: VideoSource) -> Result<Self, ModuleError> {
        let parent = surface.downcast_ref::<HtmlElement>().ok_or(ModuleError::Unavailable)?;
        let document = parent.owner_document().ok_or(ModuleError::Unavailable)?;
        let element = document.create_element("video").map_err(dom_error)?;
        let element = element.dyn_into::<HtmlVideoElement>().map_err(|_| ModuleError::Unavailable)?;
        element
            .set_attribute("style", "position:absolute;inset:0;width:100%;height:100%;object-fit:contain;border-radius:inherit;")
            .map_err(dom_error)?;
        element.set_attribute("playsinline", "").map_err(dom_error)?;
        element.set_preload("metadata");
        parent.append_child(&element).map_err(dom_error)?;
        let video = Self { element, visible: false };
        video.update_source(&source);
        Ok(video)
    }

    fn update_source(&self, source: &VideoSource) {
        let base = self.element.base_uri().ok().flatten();
        let src = browser_uri(&source.uri, base.as_deref(), false);
        let poster = source.poster.as_deref().and_then(|poster| browser_uri(poster, base.as_deref(), true));
        self.element.set_controls(source.controls);
        update_uri(&self.element, "poster", poster.as_deref());
        if update_uri(&self.element, "src", src.as_deref()) && src.is_none() {
            self.element.load();
        }
    }
}

impl NativeViewInstance for Video {
    fn update(&mut self, props: Box<dyn Any>) {
        if let Ok(source) = props.downcast::<VideoSource>() {
            self.update_source(&source);
        }
    }

    fn place(&mut self, placement: &NativePlacement) {
        self.visible = placement.visible;
        if !self.visible {
            let _ = self.element.pause();
        }
    }

    fn call(&mut self, _: CallTicket, command: Box<dyn Any>, _: ModuleDispatcher) {
        let Ok(command) = command.downcast::<VideoCommand>() else { return };
        match *command {
            VideoCommand::Play if self.visible => {
                if let Ok(playing) = self.element.play() {
                    // Browser playback may require a user gesture or reject an unsupported source.
                    spawn_local(async move {
                        let _ = JsFuture::from(playing).await;
                    });
                }
            }
            VideoCommand::Pause => {
                let _ = self.element.pause();
            }
            VideoCommand::Play => {}
        }
    }

    fn snapshot(&mut self, request: NativeSnapshotRequest, reply: ModuleDispatcher) {
        snapshot_unavailable(request, reply);
    }
}

impl Drop for Video {
    fn drop(&mut self) {
        let _ = self.element.pause();
        let _ = self.element.remove_attribute("src");
        self.element.load();
        self.element.remove();
    }
}

fn browser_uri(uri: &str, base: Option<&str>, image: bool) -> Option<String> {
    if uri.trim().is_empty() {
        return None;
    }
    let url = match base {
        Some(base) => Url::new_with_base(uri, base),
        None => Url::new(uri),
    }
    .ok()?;
    let href = url.href();
    match url.protocol().as_str() {
        "http:" | "https:" | "blob:" => Some(href),
        "data:" if image && href.starts_with("data:image/") => Some(href),
        _ => None,
    }
}

fn update_uri(element: &HtmlVideoElement, attr: &str, uri: Option<&str>) -> bool {
    if element.get_attribute(attr).as_deref() == uri {
        return false;
    }
    match uri {
        Some(uri) => element.set_attribute(attr, uri),
        None => element.remove_attribute(attr),
    }
    .is_ok()
}

fn dom_error(error: JsValue) -> ModuleError {
    ModuleError::Failed(error.as_string().unwrap_or_else(|| "could not mount video element".into()))
}
