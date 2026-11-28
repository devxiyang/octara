use super::{Atlases, Family, SampleBinding};
use objc2::rc::autoreleasepool;
use objc2_metal::{MTLCommandBuffer, MTLCommandQueue, MTLCreateSystemDefaultDevice, MTLDevice};
use octara_model::{
    geometry::Rect,
    identity::NodeId,
    paint::{Image, ImagePaint},
};
use octara_render::plan::{DrawSample, ResourcePlan};

#[test]
fn abandoned_uploads_advance_array_identity_and_committed_uploads_are_reused() {
    autoreleasepool(|_| {
        let Some(device) = MTLCreateSystemDefaultDevice() else {
            assert!(std::env::var_os("OCTARA_REQUIRE_GPU").is_none(), "a Metal device is required");
            return;
        };
        let queue = device.newCommandQueue().unwrap();
        let bounds = Rect::new(0.0, 0.0, 1.0, 1.0);
        let resources = ResourcePlan {
            samples: vec![DrawSample::Image(ImagePaint {
                source: NodeId::default(),
                image: Image::rgba(1, 1, [255, 0, 0, 255]).sample(),
                rect: bounds,
                bounds,
                radius: [0.0; 4],
                opacity: 1.0,
            })],
            ..ResourcePlan::default()
        };
        let mut atlases = Atlases::default();
        let abandoned = queue.commandBuffer().unwrap();
        let first = atlases.prepare(&device, &abandoned, &resources).unwrap();
        let [SampleBinding::Image(first_region)] = first.samples.as_slice() else { panic!("expected one image") };
        assert!(first.upload_bytes > 0);
        drop(abandoned);

        let submitted = queue.commandBuffer().unwrap();
        assert!(atlases.retire_invalid_writes(&submitted));
        let second = atlases.prepare(&device, &submitted, &resources).unwrap();
        let [SampleBinding::Image(second_region)] = second.samples.as_slice() else { panic!("expected one image") };
        assert!(second_region.array > first_region.array);
        assert_eq!(second.upload_bytes, first.upload_bytes);
        submitted.commit();

        let later = queue.commandBuffer().unwrap();
        assert!(!atlases.retire_invalid_writes(&later));
        let reused = atlases.prepare(&device, &later, &resources).unwrap();
        let [SampleBinding::Image(reused_region)] = reused.samples.as_slice() else { panic!("expected one image") };
        assert_eq!(reused_region.array, second_region.array);
        assert_eq!(reused.upload_bytes, 0);
        submitted.waitUntilCompleted();
    });
}

#[test]
fn overflow_checks_preserve_usable_atlas_state() {
    autoreleasepool(|_| {
        let Some(device) = MTLCreateSystemDefaultDevice() else {
            assert!(std::env::var_os("OCTARA_REQUIRE_GPU").is_none(), "a Metal device is required");
            return;
        };
        let mut atlases = Atlases::default();
        assert!(atlases.reserve(&device, Family::Image, [16_385, 1]).is_err());
        assert!(atlases.reserve(&device, Family::Image, [16_384, 16_384]).is_err());
        assert_eq!(atlases.next_array, 0);
        assert!(atlases.reserve(&device, Family::Image, [1, 1]).is_ok());
    });
}
