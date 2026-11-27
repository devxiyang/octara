//! AVIF decoding through the system ImageIO library.

use objc2_core_foundation::{CFData, CFRetained, CFType, CGPoint, CGRect, CGSize};
use objc2_core_graphics::{CGBitmapContextCreate, CGColorSpace, CGContext, CGImage, CGImageAlphaInfo, CGImageByteOrderInfo};
use octara_model::image::Image;
use std::ffi::c_void;
use std::ptr::{NonNull, null};

#[link(name = "ImageIO", kind = "framework")]
unsafe extern "C-unwind" {
    fn CGImageSourceCreateWithData(data: &CFData, options: *const c_void) -> *mut CFType;
    fn CGImageSourceCreateImageAtIndex(source: &CFType, index: usize, options: *const c_void) -> *mut CGImage;
}

pub(super) fn decode_avif(bytes: &[u8]) -> Option<Image> {
    let data = CFData::from_bytes(bytes);
    // SAFETY: Both ImageIO Create functions return a retained object or null. Null options use
    // default decoding. The byte data and source remain alive through the following draw.
    let source = unsafe { CFRetained::from_raw(NonNull::new(CGImageSourceCreateWithData(&data, null()))?) };
    let image = unsafe { CFRetained::from_raw(NonNull::new(CGImageSourceCreateImageAtIndex(&source, 0, null()))?) };
    let (width, height) = (CGImage::width(Some(&image)), CGImage::height(Some(&image)));
    if width == 0 || height == 0 || width > 16_384 || height > 16_384 {
        return None;
    }
    let stride = width.checked_mul(4)?;
    let len = stride.checked_mul(height)?;
    if len > 128 * 1024 * 1024 {
        return None;
    }
    let mut pixels = Vec::new();
    pixels.try_reserve_exact(len).ok()?;
    pixels.resize(len, 0);
    let color_space = CGColorSpace::new_device_rgb()?;
    let bitmap = CGImageByteOrderInfo::Order32Big.0 | CGImageAlphaInfo::PremultipliedLast.0;
    // SAFETY: This buffer provides height * stride initialized writable bytes and the bitmap
    // context is released before the buffer is accessed again.
    let context = unsafe { CGBitmapContextCreate(pixels.as_mut_ptr().cast(), width, height, 8, stride, Some(&color_space), bitmap) }?;
    CGContext::draw_image(Some(&context), CGRect::new(CGPoint::ZERO, CGSize::new(width as f64, height as f64)), Some(&image));
    drop(context);
    for pixel in pixels.as_chunks_mut::<4>().0 {
        let alpha = u32::from(pixel[3]);
        for color in &mut pixel[..3] {
            *color = ((u32::from(*color) * 255 + alpha / 2).checked_div(alpha).unwrap_or_default()).min(255) as u8;
        }
    }
    Some(Image::rgba(width as u32, height as u32, pixels))
}

#[cfg(test)]
mod tests {
    #[test]
    fn decodes_the_probed_avif_dimensions() {
        let bytes = include_bytes!("sample.avif");
        let size = octara::runtime::media::probe_image_size(bytes).expect("AVIF header dimensions");
        let image = super::decode_avif(bytes).expect("ImageIO AVIF decoder");
        assert_eq!(size, (32, 16));
        assert_eq!(image.size(), size);
    }
}
