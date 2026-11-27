//! A platform map behind the frame (MKMapView in the underlay on macOS; a placeholder box
//! headless): the frame is cut out where it is, and the zoom buttons drawn over it stay
//! Octara's, for the eye and for the pointer.

use super::section;
use octara::prelude::*;
use octara_kit::{ButtonVariant, Size as KitSize, button};
use serde_json::json;

#[ui]
pub fn view(cx: &mut BuildCx<'_>) -> El {
    let span = cx.signal(0.08_f64);
    let center = cx.signal(String::new());
    let zoom = move |w: &mut WriteCx<'_>, k: f64| w.update(span, |value| *value = (*value * k).clamp(0.002, 60.0));
    section("Map")
        .child(
            el().position(Position::Relative)
                .w(360)
                .h(200)
                .child(
                    native_view("map")
                        .native_slot(NativeSlot::Underlay)
                        .test_id("map")
                        .label("Map of Zürich")
                        .native_props(move |r: &mut ReadCx<'_>| {
                            let span = r.get(span);
                            json!({ "center": [47.3769, 8.5417], "span": [span, span] })
                        })
                        .on_native(move |w, e: &NativeEvent| {
                            if e.name == "region" {
                                let c = &e.data["center"];
                                w.set(center, format!("{:.4}, {:.4}", c[0].as_f64().unwrap_or(0.0), c[1].as_f64().unwrap_or(0.0)));
                            }
                        })
                        .size(rel(1.0))
                        .rounded(RadiusToken::Xl),
                )
                .child(
                    col()
                        .position(Position::Absolute)
                        .top(10.0)
                        .right(10.0)
                        .gap(sp(1.0))
                        .child(
                            button("+", KitSize::Md, ButtonVariant::Secondary)
                                .test_id("zoom_in")
                                .label("Zoom in")
                                .on_press(move |w| zoom(w, 0.5)),
                        )
                        .child(
                            button("−", KitSize::Md, ButtonVariant::Secondary)
                                .test_id("zoom_out")
                                .label("Zoom out")
                                .on_press(move |w| zoom(w, 2.0)),
                        ),
                ),
        )
        .child(
            text(move |r: &mut ReadCx<'_>| {
                let center = r.get(center);
                if center.is_empty() { "Drag or pinch the map".to_owned() } else { format!("Centered at {center}") }
            })
            .text_size(FontSizeToken::Sm)
            .test_id("map_status"),
        )
}
