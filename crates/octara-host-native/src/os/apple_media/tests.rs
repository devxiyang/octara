use super::*;

const PNG: &[u8] = include_bytes!("fixtures/sample.png");

fn pixel(frame: &ImageFrame, x: usize, y: usize) -> [u8; 4] {
    let start = (y * frame.width as usize + x) * 4;
    frame.rgba[start..start + 4].try_into().unwrap()
}

fn assert_color(frame: &ImageFrame, x: usize, y: usize, channel: usize) {
    let rgba = pixel(frame, x, y);
    assert_eq!(rgba[3], 255);
    assert!(rgba[channel] > 160, "{rgba:?}");
    assert!(rgba[..3].iter().enumerate().all(|(index, &value)| index == channel || value < 100), "{rgba:?}");
}

#[test]
fn decodes_png_webp_heic_and_avif() {
    for bytes in [PNG, include_bytes!("fixtures/sample.webp"), include_bytes!("fixtures/sample.heic")] {
        let image = decode(bytes).unwrap();
        assert_eq!(image.size(), (24, 16));
        let frame = image.frame();
        assert_color(&frame, 2, 2, 0);
        assert_color(&frame, 21, 13, 2);
    }

    let bytes = include_bytes!("../../media/sample.avif");
    let size = octara::runtime::media::probe_image_size(bytes).expect("AVIF header dimensions");
    let image = decode(bytes).expect("ImageIO AVIF decoder");
    assert_eq!(size, (32, 16));
    assert_eq!(image.size(), size);
    assert!(image.frame().rgba.as_chunks::<4>().0.iter().any(|pixel| pixel[3] == 255 && pixel[..3] != [0; 3]));
}

#[test]
fn reads_gif_frames_and_unclamped_delays() {
    let bytes = include_bytes!("fixtures/animated.gif");
    let source = AppleImageSource::new(bytes).unwrap();
    assert_eq!(source.frame_count(), 2);
    let (first, first_delay) = source.decode_frame(0).unwrap();
    let (second, second_delay) = source.decode_frame(1).unwrap();
    assert_eq!(first_delay.unwrap().as_millis(), 20);
    assert_eq!(second_delay.unwrap().as_millis(), 170);
    assert_eq!(decode(bytes).unwrap().frame(), first.frame());
    assert_eq!(second.size(), (12, 8));
    assert_color(&first.frame(), 10, 2, 0);
    assert_color(&second.frame(), 2, 2, 0);
    assert_color(&second.frame(), 10, 2, 1);
    assert!(source.decode_frame(2).is_err());
}

#[test]
fn jpeg_exif_orientation_rotates_pixels_and_dimensions() {
    let image = decode(include_bytes!("fixtures/rotated.jpg")).unwrap();
    assert_eq!(image.size(), (16, 24));
    assert_color(&image.frame(), 8, 2, 0);
    assert_color(&image.frame(), 8, 21, 2);
}

#[test]
fn rotates_and_mirrors_all_exif_orientations() {
    let colors = [[255, 0, 0, 255], [0, 255, 0, 255], [0, 0, 255, 255], [255, 255, 0, 255], [255, 0, 255, 255], [0, 255, 255, 255]];
    let frame = Image::rgba(3, 2, colors.concat()).frame();
    let image = frame_image(&frame).unwrap();
    for (orientation, indices) in [
        (1, [0, 1, 2, 3, 4, 5]),
        (2, [2, 1, 0, 5, 4, 3]),
        (3, [5, 4, 3, 2, 1, 0]),
        (4, [3, 4, 5, 0, 1, 2]),
        (5, [0, 3, 1, 4, 2, 5]),
        (6, [3, 0, 4, 1, 5, 2]),
        (7, [5, 2, 4, 1, 3, 0]),
        (8, [2, 5, 1, 4, 0, 3]),
    ] {
        let output = rasterize(&image, 3, 2, orientation).unwrap();
        assert_eq!(output.size(), if orientation <= 4 { (3, 2) } else { (2, 3) });
        assert_eq!(output.frame().rgba.as_ref(), indices.map(|index| colors[index]).as_flattened(), "orientation {orientation}");
        #[cfg(target_os = "ios")]
        {
            let png = encode_oriented_png(&image, orientation).unwrap();
            assert_eq!(decode(&png).unwrap().frame(), output.frame());
        }
    }
}

#[test]
fn resizes_pixels_and_round_trips_png_alpha() {
    let source = decode(PNG).unwrap();
    let smaller = resize(&source.frame(), 6, 4).unwrap();
    assert_eq!(smaller.size(), (6, 4));
    assert_color(&smaller.frame(), 0, 0, 0);
    assert_color(&smaller.frame(), 5, 3, 2);

    let image = Image::rgba(1, 1, [45, 120, 190, 224]);
    let png = encode_png(&image.frame()).unwrap();
    assert!(png.starts_with(b"\x89PNG\r\n\x1a\n"));
    let decoded = decode(&png).unwrap();
    assert_eq!(decoded.size(), (1, 1));
    let [r, g, b, a] = pixel(&decoded.frame(), 0, 0);
    assert_eq!(a, 224);
    assert!(r.abs_diff(45) <= 1 && g.abs_diff(120) <= 1 && b.abs_diff(190) <= 1);
}

#[test]
fn rejects_damaged_and_oversized_images() {
    assert!(decode(&PNG[..40]).is_err());
    assert!(decode(&[]).is_err());
    assert_eq!(decode(&vec![0; MAX_ENCODED_BYTES + 1]), Err("Encoded image exceeds the byte limit"));
    assert!(rgba_layout(MAX_DIMENSION + 1, 1).is_err());
    assert!(rgba_layout(MAX_DIMENSION, MAX_DIMENSION).is_err());
    let frame = Image::rgba(0, 0, []).frame();
    assert!(encode_png(&frame).is_err());
}
