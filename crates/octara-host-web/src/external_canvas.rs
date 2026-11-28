//! Registration of producer-owned canvases for direct browser composition.

use crate::dom;
use octara_model::image::{ExternalTextureHandle, ExternalTextureKind};
use rustc_hash::FxHashMap;
use std::cell::RefCell;
use std::sync::atomic::{AtomicU64, Ordering};
use wasm_bindgen::JsValue;
use web_sys::{Document, HtmlCanvasElement, HtmlElement};

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct CanvasId(u64);

static NEXT_CANVAS: AtomicU64 = AtomicU64::new(1);

thread_local! {
    static CANVASES: RefCell<FxHashMap<CanvasId, HtmlElement>> = RefCell::default();
}

/// Registers an existing canvas for use as an external image source on the web host.
///
/// Keep the registration alive while using its handle. A canvas has a single
/// DOM position and can appear in one media slot at a time in its own document.
/// The producer retains control of the drawing context and backing dimensions.
pub struct WebCanvas {
    id: CanvasId,
    handle: ExternalTextureHandle,
    container: HtmlElement,
    canvas: HtmlCanvasElement,
    original_style: Option<String>,
}

impl WebCanvas {
    /// Registers a detached canvas, using its current backing dimensions as the
    /// image's natural size. Octara owns its CSS and placement until this guard drops.
    pub fn register(canvas: HtmlCanvasElement) -> Result<Self, JsValue> {
        if canvas.parent_node().is_some() {
            return Err("An external canvas must be detached before registration".into());
        }
        let document = canvas.owner_document().ok_or("An external canvas needs an owning document")?;
        let container = dom::html(&document, "div")?;
        let id = CanvasId(
            NEXT_CANVAS.try_update(Ordering::Relaxed, Ordering::Relaxed, |id| id.checked_add(1)).expect("canvas identities exhausted"),
        );
        let handle = ExternalTextureHandle::new(ExternalTextureKind::WebCanvas, (canvas.width(), canvas.height()), id);
        let registration = Self { id, handle, container, original_style: canvas.get_attribute("style"), canvas };
        registration.canvas.set_attribute("style", "display:block;width:100%;height:100%;object-fit:inherit;pointer-events:none;")?;
        registration.container.append_child(&registration.canvas)?;
        CANVASES.with_borrow_mut(|canvases| canvases.insert(id, registration.container.clone()));
        Ok(registration)
    }

    /// Returns the handle accepted by `ImageSource::External`.
    pub fn handle(&self) -> ExternalTextureHandle {
        self.handle.clone()
    }
}

impl Drop for WebCanvas {
    fn drop(&mut self) {
        CANVASES.with_borrow_mut(|canvases| canvases.remove(&self.id));
        self.container.remove();
        if self.canvas.parent_node().as_ref() == Some(self.container.as_ref()) {
            self.canvas.remove();
        }
        let _ = dom::attribute(&self.canvas, "style", self.original_style.as_deref());
    }
}

pub(crate) fn resolve(handle: &ExternalTextureHandle, document: &Document) -> Option<HtmlElement> {
    if handle.kind() != ExternalTextureKind::WebCanvas {
        return None;
    }
    let id = handle.downcast_ref::<CanvasId>()?;
    CANVASES.with_borrow(|canvases| canvases.get(id).filter(|container| container.owner_document().as_ref() == Some(document)).cloned())
}
