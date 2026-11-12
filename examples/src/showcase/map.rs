//! A platform map behind the frame (MKMapView in the underlay on macOS; a placeholder box
//! headless): the frame is cut out where it is, and the zoom buttons drawn over it stay
//! Octara's, for the eye and for the pointer.

use super::section;
use octara::prelude::*;
use octara_kit::button;

pub fn card() -> El {
    let span = state("map_span", || 0.08f64);
    let center = state("map_center", String::new);
    let zoom = move |k: f64| span.set((span.get() * k).clamp(0.002, 60.0));
    section("Map")
        .child(
            el().relative()
                .w(360)
                .h(200)
                .child(
                    native_view("map")
                        .slot(Slot::Underlay)
                        .id("map")
                        .label("Map of Zürich")
                        .props(move || json!({ "center": [47.3769, 8.5417], "span": [span.get(), span.get()] }))
                        .on_native(move |e: &NativeEvent| {
                            if e.name == "region" {
                                let c = &e.data["center"];
                                center.set(format!("{:.4}, {:.4}", c[0].as_f64().unwrap_or(0.0), c[1].as_f64().unwrap_or(0.0)));
                            }
                        })
                        .size_full()
                        .rounded_xl(),
                )
                .child(
                    col()
                        .absolute()
                        .top(10.0)
                        .right(10.0)
                        .gap_1()
                        .child(button("+").id("zoom_in").label("Zoom in").on_press(move || zoom(0.5)))
                        .child(button("−").id("zoom_out").label("Zoom out").on_press(move || zoom(2.0))),
                ),
        )
        .child(
            text(move || if center.get().is_empty() { "Drag or pinch the map".to_string() } else { format!("Centered at {}", center.get()) })
                .text_sm()
                .id("map_status"),
        )
}
