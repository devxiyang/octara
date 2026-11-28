//! Upload image changes and prepare the canvas frames for publication.

use super::{CanvasFrame, DomEngine};
use crate::image::{self, CanvasSize};
use crate::{css, dom, external_canvas, rich};
#[cfg(feature = "webgpu")]
use octara::runtime::engine::HostEngine;
#[cfg(feature = "webgpu")]
use octara::tree::TreeDelta;
use octara::tree::{DirtyBitmap, NodeId, Tree};
use octara_model::geometry::{Size, WindowViewport};
use octara_model::image::{ExternalTextureHandle, MediaAsset, ObjectFit};
use octara_model::paint::{CanvasOp, Sample};
#[cfg(feature = "webgpu")]
use rustc_hash::FxHashSet;
use std::sync::Arc;
use wasm_bindgen::{JsCast, JsValue};
use web_sys::{HtmlCanvasElement, HtmlElement};

#[derive(Clone, PartialEq)]
enum MediaContent {
    Pixels(Sample),
    Uri(Arc<str>),
    External(ExternalTextureHandle),
}

#[derive(Clone)]
pub(super) struct MediaFrame {
    content: MediaContent,
    fit: ObjectFit,
    element: HtmlElement,
    wrapper: HtmlElement,
}

impl MediaFrame {
    pub(super) fn is_pixels(&self) -> bool {
        matches!(self.content, MediaContent::Pixels(_))
    }

    pub(super) fn mounted_external(&self) -> Option<&HtmlElement> {
        (matches!(self.content, MediaContent::External(_)) && self.element.parent_node().as_ref() == Some(self.wrapper.as_ref()))
            .then_some(&self.element)
    }

    fn replaced_external(&self, asset: Option<&MediaAsset>) -> Option<HtmlElement> {
        if let (MediaContent::External(current), Some(MediaAsset::External(next))) = (&self.content, asset)
            && current == next
        {
            return None;
        }
        self.mounted_external().cloned()
    }

    fn style(content: &MediaContent, fit: ObjectFit) -> String {
        let clip = if matches!(content, MediaContent::External(_)) { "overflow:hidden;" } else { "" };
        format!(
            "position:absolute;inset:0;width:100%;height:100%;pointer-events:none;border-radius:inherit;object-fit:{};{clip}",
            css::object_fit(fit)
        )
    }
}

impl DomEngine {
    pub(super) fn detach_external_canvases(&mut self, node: NodeId) -> Result<(), JsValue> {
        let canvases: Vec<_> = self.entries.get(&node).into_iter().flat_map(super::EntryState::mounted_external_media).cloned().collect();
        for canvas in canvases {
            self.pending().undo.place(&canvas, None, None)?;
        }
        Ok(())
    }

    pub(super) fn release_replaced_canvases(&mut self, tree: &Tree, nodes: impl IntoIterator<Item = NodeId>) -> Result<(), JsValue> {
        // Release every changing owner before placing any replacement so retained slots can swap canvases.
        for node in nodes.into_iter().filter(|&node| tree.is_live(node)) {
            let Some(entry) = self.entries.get(&node) else { continue };
            let media = entry
                .media
                .as_ref()
                .and_then(|frame| frame.replaced_external(tree.media_source(node).and_then(|slot| slot.asset.as_ref())));
            let mut releasing: Vec<_> = media.into_iter().collect();
            if let Some(inline) = entry.inline_media.as_deref() {
                releasing.extend(
                    tree.inline_media_on(node)
                        .enumerate()
                        .filter_map(|(occurrence, (_, slot))| inline.get(&occurrence)?.replaced_external(slot.asset.as_ref())),
                );
            }
            for element in releasing {
                self.pending().undo.place(&element, None, None)?;
            }
        }
        Ok(())
    }

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
        let mut content = asset.map(|asset| match asset {
            MediaAsset::Pixels(image) => MediaContent::Pixels(image.sample()),
            MediaAsset::BrowserUri(uri) => MediaContent::Uri(uri.clone()),
            MediaAsset::External(handle) => MediaContent::External(handle.clone()),
        });
        let external = if let Some(MediaContent::External(handle)) = &content {
            let element = external_canvas::resolve(handle, &self.document).filter(|element| {
                element.parent_node().is_none() || frame.as_ref().and_then(MediaFrame::mounted_external) == Some(element)
            });
            if element.is_none() {
                content = None;
            }
            element
        } else {
            None
        };
        if let Some(previous) = frame.as_mut()
            && content.as_ref() == Some(&previous.content)
        {
            let undo = &mut self.pending().undo;
            let fit_changed = previous.fit != fit;
            if fit_changed {
                undo.attr(&previous.element, "style", Some(&MediaFrame::style(&previous.content, fit)))?;
                previous.fit = fit;
            }
            // Rich text can rebuild its wrapper while keeping the same image asset.
            let moved = undo.place(&previous.element, Some(wrapper), None)?;
            previous.wrapper = wrapper.clone();
            return Ok(fit_changed || moved);
        }
        if content.is_none() && frame.is_none() {
            return Ok(false);
        }
        if let Some(previous) = frame.as_ref()
            && (!matches!(previous.content, MediaContent::External(_)) || previous.mounted_external().is_some())
        {
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
                MediaContent::External(_) => external.expect("registered external canvas"),
            };
            let undo = &mut self.pending().undo;
            undo.attr(&element, "aria-hidden", Some("true"))?;
            undo.attr(&element, "style", Some(&MediaFrame::style(&content, fit)))?;
            undo.place(&element, Some(wrapper), None)?;
            Some(MediaFrame { content, fit, element, wrapper: wrapper.clone() })
        } else {
            None
        };
        Ok(true)
    }

    pub(super) fn update_canvases(
        &mut self,
        tree: &Tree,
        viewport: WindowViewport,
        measured: &[NodeId],
        dirty: &DirtyBitmap,
    ) -> Result<(), JsValue> {
        let mut affected = self.canvas_images.clone();
        affected.extend(measured.iter().copied().chain(dirty.iter()).filter(|&node| tree.canvas(node).is_some()));
        for node in affected {
            let Some(core) = tree.node(node).filter(|_| tree.is_live(node)) else { continue };
            let Some(operations) = tree.canvas(node) else { continue };
            let Some(layout) = self.sample(node) else { continue };
            let Some(element) = self.dom.element(node) else { continue };
            let size = CanvasSize {
                area: Size::new(
                    (layout.box_size.width - layout.border.left - layout.border.right - layout.padding.left - layout.padding.right)
                        .max(0.0),
                    (layout.box_size.height - layout.border.top - layout.border.bottom - layout.padding.top - layout.padding.bottom)
                        .max(0.0),
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
        affected.extend(created.iter().copied());
        affected.extend(measured.iter().copied().filter(|&node| tree.paint(node).is_some_and(|paint| paint.custom.is_some())));
        for node in affected.iter() {
            if !tree.is_live(node) {
                continue;
            }
            let plan = (|| {
                let paint = tree.paint(node)?.custom.as_ref()?;
                let mut layout = self.geometry(node)?;
                let size = self.sample(node)?.box_size;
                layout.rect.w = size.width;
                layout.rect.h = size.height;
                let element = self.dom.element(node)?;
                Plan::new(node, paint, &element, layout, tree.visual(node).radius, viewport.scale, now)
            })();
            self.pending().gpu.insert(node, plan);
        }
    }
}
