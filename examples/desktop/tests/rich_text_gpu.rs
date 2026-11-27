//! Compare GPU output with the headless rich-text fixtures.

#[path = "../../tests/support/rich_text.rs"]
mod fixture;

use octara::model::paint::{DisplayList, Picture};
use octara_render::GpuRenderer;

#[test]
fn rich_span_pill_has_cpu_gpu_parity() {
    fixture::rich_span_pill(compare_gpu());
}

#[test]
fn inline_pill_has_cpu_gpu_parity() {
    fixture::inline_pill(compare_gpu());
}

fn compare_gpu() -> impl FnMut(&DisplayList, &Picture, f32) {
    let mut gpu = GpuRenderer::new();
    if let Some(gpu) = &gpu {
        eprintln!("Rich text CPU/GPU parity on {:?}", gpu.device().adapter_info().backend);
    } else {
        assert!(std::env::var_os("OCTARA_REQUIRE_GPU").is_none(), "a GPU adapter is required");
    }
    move |drawing, expected, scale| {
        if let Some(gpu) = &mut gpu {
            let actual = gpu.render(drawing, scale);
            assert_eq!((actual.width(), actual.height()), (expected.width(), expected.height()));
            let worst = expected.rgba().iter().zip(actual.rgba()).map(|(a, b)| a.abs_diff(*b)).max().unwrap();
            assert!(worst <= 2, "largest CPU/GPU channel difference at {scale}x is {worst}");
        }
    }
}
