//! Upload image changes and prepare the canvas frames for publication.

use super::{CanvasFrame, DomEngine};
use crate::image::{self, CanvasSize};
use crate::{css, dom, media, rich};
use octara::runtime::engine::HostEngine;
#[cfg(feature = "webgpu")]
use octara::tree::TreeDelta;
use octara::tree::{NodeId, Tree};
use octara_model::geometry::{Size, WindowViewport};
use octara_model::image::{MediaAsset, ObjectFit, VideoSource};
use octara_model::paint::{CanvasOp, Sample};
use rustc_hash::FxHashSet;
use std::sync::Arc;
use wasm_bindgen::{JsCast, JsValue};
use web_sys::{HtmlCanvasElement, HtmlElement};

#[derive(Clone, PartialEq)]
enum MediaContent {
    Pixels(Sample),
    Uri(Arc<str>),
}

#[derive(Clone)]
pub(super) struct MediaFrame {
    content: MediaContent,
    fit: ObjectFit,
    element: HtmlElement,
}

impl MediaFrame {
    pub(super) fn is_pixels(&self) -> bool {
        matches!(self.content, MediaContent::Pixels(_))
    }
}

impl DomEngine {
    pub(super) fn update_image(&mut self, node: NodeId, sample: Sample) -> Result<(), JsValue> {
        let Some(element) = self.dom.element(node) else { return Ok(()) };
        let canvas = element.dyn_into::<HtmlCanvasElement>()?;
        let previous = self.entries.get(&node).and_then(|entry| entry.image.clone());
        self.pending().undo.save_image(&canvas, previous);
        image::upload(&canvas, Some(&sample))?;
        self.remember(node);
        self.entries.get_mut(&node).expect("retained image").image = Some(sample);
        self.register_indexes(node);
        Ok(())
    }

    pub(super) fn update_media(&mut self, node: NodeId, asset: Option<&MediaAsset>, fit: ObjectFit) -> Result<bool, JsValue> {
        let Some(wrapper) = self.dom.element(node) else { return Ok(false) };
        let mut frame = self.entries.get(&node).and_then(|entry| entry.media.clone());
        if !self.replace_media_frame(&wrapper, &mut frame, asset, fit)? {
            return Ok(false);
        }
        self.remember(node);
        self.entries.get_mut(&node).expect("retained media").media = frame;
        self.register_indexes(node);
        Ok(true)
    }

    pub(super) fn update_inline_media(&mut self, tree: &Tree, node: NodeId) -> Result<(), JsValue> {
        let Some(owner) = self.dom.element(node) else { return Ok(()) };
        for (occurrence, (index, slot)) in tree.inline_media_on(node).enumerate() {
            let mut frame = self.entries.get(&node).and_then(|entry| entry.inline_media.as_deref()?.get(&occurrence)).cloned();
            if slot.asset.is_none() && frame.is_none() {
                continue;
            }
            let Some(wrapper) = rich::media_element(&owner, index) else { continue };
            if self.replace_media_frame(&wrapper, &mut frame, slot.asset.as_ref(), slot.fit)? {
                self.remember(node);
                let inline = &mut self.entries.get_mut(&node).expect("retained text").inline_media;
                if let Some(frame) = frame {
                    inline.get_or_insert_with(Default::default).insert(occurrence, frame);
                } else if let Some(map) = inline.as_deref_mut() {
                    map.remove(&occurrence);
                    if map.is_empty() {
                        *inline = None;
                    }
                }
                self.register_indexes(node);
            }
        }
        Ok(())
    }

    fn replace_media_frame(
        &mut self,
        wrapper: &HtmlElement,
        frame: &mut Option<MediaFrame>,
        asset: Option<&MediaAsset>,
        fit: ObjectFit,
    ) -> Result<bool, JsValue> {
        let content = asset.map(|asset| match asset {
            MediaAsset::Pixels(image) => MediaContent::Pixels(image.sample()),
            MediaAsset::BrowserUri(uri) => MediaContent::Uri(uri.clone()),
        });
        if frame.as_ref().map(|frame| (&frame.content, frame.fit)) == content.as_ref().map(|content| (content, fit)) {
            // Rich text can rebuild its wrapper while keeping the exact same image asset.
            if let Some(frame) = frame.as_ref() {
                self.pending().undo.place(&frame.element, Some(wrapper), None)?;
            }
            return Ok(false);
        }
        if let Some(previous) = frame.as_ref() {
            self.pending().undo.place(&previous.element, None, None)?;
        }
        *frame = if let Some(content) = content {
            let element = match &content {
                MediaContent::Pixels(sample) => {
                    let canvas = self.document.create_element("canvas")?.dyn_into::<HtmlCanvasElement>()?;
                    image::upload(&canvas, Some(sample))?;
                    canvas.unchecked_into::<HtmlElement>()
                }
                MediaContent::Uri(uri) => {
                    let element = dom::html(&self.document, "img")?;
                    element.set_attribute("alt", "")?;
                    element.set_attribute("decoding", "async")?;
                    element.set_attribute("loading", "lazy")?;
                    element.set_attribute("src", uri)?;
                    element
                }
            };
            element.set_attribute("aria-hidden", "true")?;
            element.set_attribute(
                "style",
                &format!(
                    "position:absolute;inset:0;width:100%;height:100%;pointer-events:none;border-radius:inherit;object-fit:{};",
                    css::object_fit(fit)
                ),
            )?;
            self.pending().undo.place(&element, Some(wrapper), None)?;
            Some(MediaFrame { content, fit, element })
        } else {
            None
        };
        Ok(true)
    }

    pub(super) fn update_video(&mut self, node: NodeId, properties: &serde_json::Value) -> Result<(), JsValue> {
        let Some(video) = self.dom.element(node) else { return Ok(()) };
        let source = serde_json::from_value::<VideoSource>(properties.clone()).ok();
        let src = source.as_ref().and_then(|source| media::browser_uri(&source.uri, false).ok());
        let poster = source.as_ref().and_then(|source| source.poster.as_deref()).and_then(|poster| media::browser_uri(poster, true).ok());
        let undo = &mut self.pending().undo;
        undo.attr(&video, "preload", Some("metadata"))?;
        undo.attr(&video, "playsinline", Some(""))?;
        undo.attr(&video, "controls", source.as_ref().filter(|source| source.controls).map(|_| ""))?;
        undo.attr(&video, "poster", poster.as_deref())?;
        undo.attr(&video, "src", src.as_deref())?;
        Ok(())
    }

    pub(super) fn update_canvases(
        &mut self,
        tree: &Tree,
        viewport: WindowViewport,
        measured: &[NodeId],
        dirty: &FxHashSet<NodeId>,
    ) -> Result<(), JsValue> {
        let mut affected = self.canvas_images.clone();
        affected.extend(measured.iter().chain(dirty).copied().filter(|&node| tree.canvas(node).is_some()));
        for node in affected {
            let Some(core) = tree.node(node).filter(|_| tree.is_live(node)) else { continue };
            let Some(operations) = tree.canvas(node) else { continue };
            let Some(layout) = self.geometry(node) else { continue };
            let Some(element) = self.dom.element(node) else { continue };
            let size = CanvasSize {
                area: Size::new(
                    (layout.rect.w - layout.border.left - layout.border.right - layout.padding.left - layout.padding.right).max(0.0),
                    (layout.rect.h - layout.border.top - layout.border.bottom - layout.padding.top - layout.padding.bottom).max(0.0),
                ),
                padding: layout.padding,
                scale: viewport.scale,
            };
            let previous = self.entries.get(&node).and_then(|entry| entry.canvas.as_ref());
            let samples: Vec<_> = operations
                .iter()
                .filter_map(|op| match op {
                    CanvasOp::Image { image, .. } => Some(image.sample()),
                    _ => None,
                })
                .collect();
            let images: Vec<_> = samples.iter().map(|sample| (sample.source().id(), sample.frame().version)).collect();
            if previous.is_some_and(|frame| frame.size == size && frame.paint_rev == core.paint_rev && frame.images == images) {
                continue;
            }
            let old = previous.map(|frame| frame.canvas.clone());
            let next = if !operations.is_empty() && size.area.width > 0.0 && size.area.height > 0.0 {
                let canvas = image::draw(&self.document, operations, samples, size, tree.visual(node), tree.theme_at(node))?;
                Some(CanvasFrame { canvas, size, paint_rev: core.paint_rev, images })
            } else {
                None
            };
            if let Some(old) = old {
                self.pending().undo.place(&old, None, None)?;
            }
            if let Some(frame) = &next {
                self.pending().undo.place(&frame.canvas, Some(&element), None)?;
            }
            self.remember(node);
            self.entries.get_mut(&node).expect("retained canvas").canvas = next;
            self.register_indexes(node);
        }
        Ok(())
    }

    #[cfg(feature = "webgpu")]
    pub(super) fn stage_gpu(
        &mut self,
        tree: &Tree,
        delta: &TreeDelta,
        viewport: WindowViewport,
        now: f64,
        created: &FxHashSet<NodeId>,
        measured: &[NodeId],
    ) {
        use crate::gpu::Plan;
        let now = std::time::Duration::try_from_secs_f64(now).unwrap_or_default();
        let mut affected = delta.paint_dirty.clone();
        affected.extend(created);
        affected.extend(measured.iter().copied().filter(|&node| tree.paint(node).is_some_and(|paint| paint.custom.is_some())));
        for node in affected {
            if !tree.is_live(node) {
                continue;
            }
            let plan = (|| {
                let paint = tree.paint(node)?.custom.as_ref()?;
                let layout = self.geometry(node)?;
                let element = self.dom.element(node)?;
                Plan::new(node, paint, &element, layout, tree.visual(node).radius, viewport.scale, now)
            })();
            self.pending().gpu.insert(node, plan);
        }
    }
}
