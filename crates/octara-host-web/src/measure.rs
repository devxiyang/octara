//! Numeric CSSOM geometry measured in one browser call.

use js_sys::{Array, Float32Array};
use octara_model::geometry::{Edges, LayoutBox, Rect, Size, Vec2};
use wasm_bindgen::prelude::*;
use web_sys::HtmlElement;

const STRIDE: usize = 20;

#[wasm_bindgen(module = "/src/measure.js")]
extern "C" {
    #[wasm_bindgen(catch)]
    fn octara_measure_batch(surface: &HtmlElement, elements: &Array, out: &Float32Array) -> Result<(), JsValue>;
    #[wasm_bindgen(catch)]
    fn octara_text_anchor(surface: &HtmlElement, element: &HtmlElement, caret: bool, points: &Array) -> Result<JsValue, JsValue>;

}

pub(crate) fn text_anchor(surface: &HtmlElement, element: &HtmlElement, caret: bool, points: &Array) -> Option<Rect> {
    let value = octara_text_anchor(surface, element, caret, points).ok()?;
    if value.is_null() {
        return None;
    }
    let array = Array::from(&value);
    let coordinate = |index| array.get(index).as_f64().map(|value| value as f32);
    Some(Rect::new(coordinate(0)?, coordinate(1)?, coordinate(2)?, coordinate(3)?))
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MeasureMode {
    /// Measure changed nodes, registered geometry demands, and observer notifications.
    #[default]
    DemandDriven,
    /// Verify each mounted box against the current CSSOM.
    VerifyFullCssom,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Geometry {
    pub bounds: Rect,
    pub client: Size,
    pub content: Size,
    pub scroll: Vec2,
    pub padding: Edges,
    pub border: Edges,
    pub box_size: Size,
}

impl Geometry {
    pub fn layout(self, parent: Option<Self>) -> LayoutBox {
        let rect = if let Some(parent) = parent.filter(|_| self.bounds != Rect::default()) {
            Rect {
                x: self.bounds.x - parent.bounds.x + parent.scroll.x,
                y: self.bounds.y - parent.bounds.y + parent.scroll.y,
                ..self.bounds
            }
        } else {
            self.bounds
        };
        LayoutBox { rect, padding: self.padding, border: self.border }
    }

    pub fn scroll_max(self) -> Vec2 {
        Vec2::new((self.content.width - self.client.width).max(0.0), (self.content.height - self.client.height).max(0.0))
    }
}

#[derive(Default)]
pub(crate) struct MeasureBuffer(Vec<f32>);

impl MeasureBuffer {
    pub fn read(&mut self, surface: &HtmlElement, elements: &Array) -> Result<impl Iterator<Item = Geometry> + '_, JsValue> {
        self.0.resize(elements.length() as usize * STRIDE, 0.0);
        if !self.0.is_empty() {
            // The synchronous shim only writes to this view. It calls no Rust code and does not retain it.
            let out = unsafe { Float32Array::view_mut_raw(self.0.as_mut_ptr(), self.0.len()) };
            octara_measure_batch(surface, elements, &out)?;
        }
        Ok(self.0.as_chunks::<STRIDE>().0.iter().map(|f| Geometry {
            bounds: Rect::new(f[0], f[1], f[2], f[3]),
            client: Size::new(f[4], f[5]),
            content: Size::new(f[6], f[7]),
            scroll: Vec2::new(f[8], f[9]),
            padding: [f[10], f[11], f[12], f[13]].into(),
            border: [f[14], f[15], f[16], f[17]].into(),
            box_size: Size::new(f[18], f[19]),
        }))
    }
}
