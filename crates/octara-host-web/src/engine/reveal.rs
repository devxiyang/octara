use super::DomEngine;
use js_sys::{Array, Uint8Array};
use octara::Tree;
use octara_model::motion::TextRevealTrack;
use rustc_hash::FxHashMap;
use std::time::Duration;
use unicode_segmentation::UnicodeSegmentation;
use wasm_bindgen::prelude::*;
use web_sys::HtmlElement;

#[wasm_bindgen(module = "/src/reveal.js")]
extern "C" {
    #[wasm_bindgen(catch)]
    fn octara_sample_reveals(surface: &HtmlElement, samples: &Array) -> Result<(), JsValue>;
    pub(super) fn octara_clear_reveals(surface: &HtmlElement);
}

impl DomEngine {
    pub(super) fn stage_reveals(&mut self, tree: &Tree, now: f64) -> Option<Duration> {
        let previous = self.pending.as_ref().and_then(|pending| pending.reveals.as_ref()).map_or(&self.reveals, |(tracks, _)| tracks);
        let samples = Array::new();
        let mut tracks = FxHashMap::default();
        let mut moving = false;
        for (node, reveal) in tree.text_reveals() {
            if reveal.finished || !reveal.enabled(self.reduced_motion) {
                continue;
            }
            let Some(text) = tree.text(node) else { continue };
            let Some(element) = self.projected_element(node) else { continue };
            let track = TextRevealTrack::update(previous.get(&node).copied(), *reveal, now);
            moving |= track.rest_at().is_some_and(|rest| rest > now);
            let mut coverage = Vec::new();
            for (start, grapheme) in text.text.grapheme_indices(true) {
                let opacity = (track.opacity(start + grapheme.len(), now) * 255.0).round() as u8;
                coverage.extend(std::iter::repeat_n(opacity, grapheme.encode_utf16().count()));
            }
            if coverage.iter().any(|&opacity| opacity != 255) {
                let sample = Array::new();
                sample.push(&element);
                sample.push(&Uint8Array::from(coverage.as_slice()));
                let tint = track.tint().map(|tint| crate::css::color(tint.resolve(tree.theme_at(node)).0));
                sample.push(&tint.map_or(JsValue::NULL, |tint| JsValue::from_str(&tint)));
                samples.push(&sample);
            }
            tracks.insert(node, track);
        }
        self.pending().reveals = Some((tracks, samples));
        moving.then(|| Duration::from_secs_f64(now + 1.0 / 60.0))
    }

    pub(super) fn publish_reveals(&mut self, projection: Option<(FxHashMap<octara::NodeId, TextRevealTrack>, Array)>) {
        if let Some((tracks, samples)) = projection {
            if let Err(error) = octara_sample_reveals(&self.surface, &samples) {
                octara_clear_reveals(&self.surface);
                web_sys::console::error_1(&error);
            }
            self.reveals = tracks;
        }
    }
}
