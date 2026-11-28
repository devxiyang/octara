//! Raster codecs backed by ImageIO and Core Graphics on Apple platforms.

use objc2_core_foundation::{
    CFData, CFDictionary, CFMutableData, CFNumber, CFRetained, CFString, CFType, CGAffineTransform, CGPoint, CGRect, CGSize, ConcreteType,
};
use objc2_core_graphics::{
    CGBitmapContextCreate, CGBitmapInfo, CGColorRenderingIntent, CGColorSpace, CGContext, CGDataProvider, CGImage, CGImageAlphaInfo,
    CGImageByteOrderInfo, CGInterpolationQuality, kCGColorSpaceSRGB,
};
use octara_model::image::{Image, ImageFrame};
use std::ffi::c_void;
use std::ptr::{NonNull, from_ref, null};
use std::sync::Arc;
use std::time::Duration;

const MAX_ENCODED_BYTES: usize = 64 * 1024 * 1024;
const MAX_DIMENSION: usize = 16_384;
const MAX_RGBA_BYTES: usize = 128 * 1024 * 1024;
const INVALID_IMAGE: &str = "Unsupported, oversized, or damaged image";
const INVALID_FRAME: &str = "Invalid decoded image";

#[link(name = "ImageIO", kind = "framework")]
unsafe extern "C-unwind" {
    fn CGImageSourceCreateWithData(data: &CFData, options: Option<&CFDictionary>) -> Option<NonNull<CFType>>;
    fn CGImageSourceGetCount(source: &CFType) -> usize;
    fn CGImageSourceGetPrimaryImageIndex(source: &CFType) -> usize;
    fn CGImageSourceGetStatusAtIndex(source: &CFType, index: usize) -> i32;
    fn CGImageSourceCopyPropertiesAtIndex(source: &CFType, index: usize, options: Option<&CFDictionary>) -> Option<NonNull<CFDictionary>>;
    fn CGImageSourceCreateImageAtIndex(source: &CFType, index: usize, options: Option<&CFDictionary>) -> Option<NonNull<CGImage>>;

    fn CGImageDestinationCreateWithData(
        data: &CFMutableData,
        kind: &CFString,
        count: usize,
        options: Option<&CFDictionary>,
    ) -> Option<NonNull<CFType>>;
    fn CGImageDestinationAddImage(destination: &CFType, image: &CGImage, properties: Option<&CFDictionary>);
    fn CGImageDestinationFinalize(destination: &CFType) -> bool;

    static kCGImagePropertyPixelWidth: &'static CFString;
    static kCGImagePropertyPixelHeight: &'static CFString;
    static kCGImagePropertyOrientation: &'static CFString;
    static kCGImagePropertyGIFDictionary: &'static CFString;
    static kCGImagePropertyGIFDelayTime: &'static CFString;
    static kCGImagePropertyGIFUnclampedDelayTime: &'static CFString;
}

pub(crate) fn decode(bytes: &[u8]) -> Result<Image, &'static str> {
    let source = AppleImageSource::new(bytes)?;
    // SAFETY: The source is a live CGImageSource. ImageIO returns zero for formats without a primary image.
    let index = unsafe { CGImageSourceGetPrimaryImageIndex(&source.0) };
    source.decode_frame(index).map(|(image, _)| image)
}

/// A bounded ImageIO source for decoding individual images or animation frames.
pub struct AppleImageSource(CFRetained<CFType>);

impl AppleImageSource {
    /// Opens encoded raster data through the system codec.
    pub fn new(bytes: &[u8]) -> Result<Self, &'static str> {
        if bytes.len() > MAX_ENCODED_BYTES {
            return Err("Encoded image exceeds the byte limit");
        }
        let data = CFData::from_bytes(bytes);
        // SAFETY: The Create function returns an owned source, which retains the input data.
        let source = unsafe { CGImageSourceCreateWithData(&data, None).map(|source| CFRetained::from_raw(source)) };
        source.map(Self).ok_or(INVALID_IMAGE)
    }

    /// Returns the number of frames reported by ImageIO.
    pub fn frame_count(&self) -> usize {
        // SAFETY: The source owns a live CGImageSource.
        unsafe { CGImageSourceGetCount(&self.0) }
    }

    /// Decodes one orientation-normalized frame with its GIF delay, when present.
    pub fn decode_frame(&self, index: usize) -> Result<(Image, Option<Duration>), &'static str> {
        // SAFETY: ImageIO checks the index. Copy returns an owned, immutable CFDictionary.
        let properties =
            unsafe { CGImageSourceCopyPropertiesAtIndex(&self.0, index, None).map(|properties| CFRetained::from_raw(properties)) }
                .ok_or(INVALID_IMAGE)?;
        // SAFETY: ImageIO exports these metadata keys for the lifetime of the process.
        let (width, height, orientation) =
            unsafe { (kCGImagePropertyPixelWidth, kCGImagePropertyPixelHeight, kCGImagePropertyOrientation) };
        let dimension = |key| property::<CFNumber>(&properties, key)?.as_isize()?.try_into().ok();
        let (width, height) = dimension(width).zip(dimension(height)).ok_or(INVALID_IMAGE)?;
        rgba_layout(width, height)?;
        let orientation = property::<CFNumber>(&properties, orientation).and_then(CFNumber::as_i32).unwrap_or(1);
        // SAFETY: The dimensions have been checked and ImageIO returns an owned image or null.
        let image = unsafe { CGImageSourceCreateImageAtIndex(&self.0, index, None).map(|image| CFRetained::from_raw(image)) }
            .ok_or(INVALID_IMAGE)?;
        // SAFETY: The image source and index are the ones used to create the image above. Zero means complete.
        if unsafe { CGImageSourceGetStatusAtIndex(&self.0, index) } != 0 {
            return Err(INVALID_IMAGE);
        }
        let (width, height) = image_size(&image);
        let decoded = rasterize(&image, width, height, orientation)?;
        Ok((decoded, gif_delay(&properties)))
    }
}

fn property<'a, T: ConcreteType>(properties: &'a CFDictionary, key: &CFString) -> Option<&'a T> {
    // SAFETY: ImageIO property dictionaries have CFString keys and immutable Core Foundation values.
    let value = unsafe { properties.value(from_ref(key).cast()).cast::<CFType>().as_ref()? };
    value.downcast_ref()
}

fn gif_delay(properties: &CFDictionary) -> Option<Duration> {
    // SAFETY: These dictionary keys are static ImageIO constants.
    let (gif, unclamped, clamped) =
        unsafe { (kCGImagePropertyGIFDictionary, kCGImagePropertyGIFUnclampedDelayTime, kCGImagePropertyGIFDelayTime) };
    let metadata = property::<CFDictionary>(properties, gif)?;
    [unclamped, clamped].into_iter().find_map(|key| Duration::try_from_secs_f64(property::<CFNumber>(metadata, key)?.as_f64()?).ok())
}

pub(crate) fn resize(frame: &ImageFrame, width: u32, height: u32) -> Result<Image, &'static str> {
    let image = frame_image(frame)?;
    rasterize(&image, width as usize, height as usize, 1)
}

pub(super) fn encode_png(frame: &ImageFrame) -> Result<Vec<u8>, &'static str> {
    let image = frame_image(frame)?;
    encode_cg_png(&image)
}

#[cfg(target_os = "ios")]
pub(super) fn encode_oriented_png(image: &CGImage, orientation: i32) -> Result<Vec<u8>, &'static str> {
    if orientation == 1 {
        return encode_cg_png(image);
    }
    let (width, height) = image_size(image);
    encode_png(&rasterize(image, width, height, orientation)?.frame())
}

fn frame_image(frame: &ImageFrame) -> Result<CFRetained<CGImage>, &'static str> {
    let (width, height) = (frame.width as usize, frame.height as usize);
    let stride = width.checked_mul(4).ok_or(INVALID_FRAME)?;
    if width == 0 || height == 0 || stride.checked_mul(height) != Some(frame.rgba.len()) {
        return Err(INVALID_FRAME);
    }
    let owner = Box::into_raw(Box::new(Arc::clone(&frame.rgba))).cast();
    // SAFETY: The owner pins the bytes until Core Graphics releases the provider.
    let provider = unsafe { CGDataProvider::with_data(owner, frame.rgba.as_ptr().cast(), frame.rgba.len(), Some(release_frame_pixels)) };
    let Some(provider) = provider else {
        // SAFETY: No provider was created, so the local code still owns this allocation.
        drop(unsafe { Box::from_raw(owner.cast::<Arc<[u8]>>()) });
        return Err(INVALID_FRAME);
    };
    let color_space = srgb()?;
    let bitmap = CGBitmapInfo(CGImageByteOrderInfo::Order32Big.0 | CGImageAlphaInfo::Last.0);
    // SAFETY: The provider has the validated, tightly packed RGBA bytes; null uses default component decoding.
    unsafe {
        CGImage::new(
            width,
            height,
            8,
            32,
            stride,
            Some(&color_space),
            bitmap,
            Some(&provider),
            null(),
            true,
            CGColorRenderingIntent::RenderingIntentDefault,
        )
    }
    .ok_or(INVALID_FRAME)
}

unsafe extern "C-unwind" fn release_frame_pixels(owner: *mut c_void, _bytes: NonNull<c_void>, _len: usize) {
    // SAFETY: Core Graphics returns the Box pointer passed to CGDataProvider::with_data exactly once.
    drop(unsafe { Box::from_raw(owner.cast::<Arc<[u8]>>()) });
}

fn encode_cg_png(image: &CGImage) -> Result<Vec<u8>, &'static str> {
    const ERROR: &str = "ImageIO could not encode PNG";
    let (width, height) = image_size(image);
    rgba_layout(width, height)?;
    let data = CFMutableData::new(None, 0).ok_or(ERROR)?;
    let kind = CFString::from_static_str("public.png");
    // SAFETY: The destination accepts mutable CFData and the PNG UTI. The Create result is owned.
    let destination =
        unsafe { CGImageDestinationCreateWithData(&data, &kind, 1, None).map(|dest| CFRetained::from_raw(dest)) }.ok_or(ERROR)?;
    // SAFETY: The destination is live and was configured for one valid CGImage and default properties.
    unsafe {
        CGImageDestinationAddImage(&destination, image, None);
        if !CGImageDestinationFinalize(&destination) {
            return Err(ERROR);
        }
    }
    drop(destination);
    Ok(data.to_vec())
}

fn image_size(image: &CGImage) -> (usize, usize) {
    (CGImage::width(Some(image)), CGImage::height(Some(image)))
}

fn srgb() -> Result<CFRetained<CGColorSpace>, &'static str> {
    // SAFETY: Core Graphics exports the sRGB color space name as a static CFString.
    CGColorSpace::with_name(Some(unsafe { kCGColorSpaceSRGB })).ok_or("Could not create sRGB color space")
}

fn rgba_layout(width: usize, height: usize) -> Result<(usize, usize), &'static str> {
    if width == 0 || height == 0 || width > MAX_DIMENSION || height > MAX_DIMENSION {
        return Err(INVALID_IMAGE);
    }
    let stride = width * 4;
    let bytes = stride * height;
    if bytes > MAX_RGBA_BYTES {
        return Err(INVALID_IMAGE);
    }
    Ok((stride, bytes))
}

fn rasterize(image: &CGImage, width: usize, height: usize, orientation: i32) -> Result<Image, &'static str> {
    let (output_width, output_height) = if matches!(orientation, 5..=8) { (height, width) } else { (width, height) };
    let (stride, len) = rgba_layout(output_width, output_height)?;
    let mut pixels = Vec::new();
    pixels.try_reserve_exact(len).map_err(|_| "Could not allocate image pixels")?;
    pixels.resize(len, 0);
    let color_space = srgb()?;
    let bitmap = CGImageByteOrderInfo::Order32Big.0 | CGImageAlphaInfo::PremultipliedLast.0;
    // SAFETY: The buffer has stride * output_height writable bytes and outlives the bitmap context.
    let context =
        unsafe { CGBitmapContextCreate(pixels.as_mut_ptr().cast(), output_width, output_height, 8, stride, Some(&color_space), bitmap) }
            .ok_or("Could not create image bitmap context")?;
    let (width, height) = (width as f64, height as f64);
    // EXIF describes rows from the top. Quartz draws in a bottom-left coordinate space.
    let (a, b, c, d, tx, ty) = match orientation {
        2 => (-1.0, 0.0, 0.0, 1.0, width, 0.0),
        3 => (-1.0, 0.0, 0.0, -1.0, width, height),
        4 => (1.0, 0.0, 0.0, -1.0, 0.0, height),
        5 => (0.0, -1.0, -1.0, 0.0, height, width),
        6 => (0.0, -1.0, 1.0, 0.0, 0.0, width),
        7 => (0.0, 1.0, 1.0, 0.0, 0.0, 0.0),
        8 => (0.0, 1.0, -1.0, 0.0, height, 0.0),
        _ => (1.0, 0.0, 0.0, 1.0, 0.0, 0.0),
    };
    CGContext::concat_ctm(Some(&context), CGAffineTransform { a, b, c, d, tx, ty });
    CGContext::set_interpolation_quality(Some(&context), CGInterpolationQuality::High);
    CGContext::draw_image(Some(&context), CGRect::new(CGPoint::ZERO, CGSize::new(width, height)), Some(image));
    drop(context);
    for pixel in pixels.as_chunks_mut::<4>().0 {
        let alpha = u32::from(pixel[3]);
        for color in &mut pixel[..3] {
            *color = ((u32::from(*color) * 255 + alpha / 2).checked_div(alpha).unwrap_or_default()).min(255) as u8;
        }
    }
    Ok(Image::rgba(output_width as u32, output_height as u32, pixels))
}

#[cfg(test)]
mod tests;
