//! AppKit pasteboard services.

use objc2::{AllocAnyThread, ClassType, rc::Retained};
use objc2_app_kit::{
    NSBitmapFormat, NSBitmapImageFileType, NSBitmapImageRep, NSCalibratedRGBColorSpace, NSPasteboard, NSPasteboardTypePNG,
    NSPasteboardTypeString, NSPasteboardTypeTIFF, NSPasteboardURLReadingFileURLsOnlyKey,
};
use objc2_foundation::{NSArray, NSData, NSDictionary, NSNumber, NSString, NSURL};
use octara_model::ask::{ClipboardItem, ClipboardKinds};
use octara_model::paint::Image;

pub(super) fn read_clipboard(accept: ClipboardKinds) -> Vec<ClipboardItem> {
    let pasteboard = NSPasteboard::generalPasteboard();
    let mut items = Vec::new();
    if accept.contains(ClipboardKinds::FILES) {
        // SAFETY: AppKit expects the NSURL class and an NSNumber for the file-URL option.
        let urls = unsafe {
            let yes = NSNumber::new_bool(true);
            let options = NSDictionary::from_slices(&[NSPasteboardURLReadingFileURLsOnlyKey], &[yes.as_ref()]);
            pasteboard.readObjectsForClasses_options(&NSArray::from_slice(&[NSURL::class()]), Some(&options))
        };
        if let Some(urls) = urls {
            let paths: Vec<_> = urls.iter().filter_map(|value| value.downcast_ref::<NSURL>()?.to_file_path()).collect();
            if !paths.is_empty() {
                items.push(ClipboardItem::Files(paths));
            }
        }
    }
    if !accept.intersects(ClipboardKinds::TEXT | ClipboardKinds::IMAGE) {
        return items;
    }
    if let Some(entries) = pasteboard.pasteboardItems() {
        for entry in &entries {
            if accept.contains(ClipboardKinds::IMAGE)
                && let Some(image) = clipboard_image(|kind| entry.dataForType(kind))
            {
                items.push(image);
            }
            // SAFETY: AppKit exports a static type for the string representation.
            if accept.contains(ClipboardKinds::TEXT)
                && let Some(text) = entry.stringForType(unsafe { NSPasteboardTypeString })
            {
                items.push(ClipboardItem::Text(text.to_string()));
            }
        }
    }
    items
}

fn clipboard_image(data: impl Fn(&NSString) -> Option<Retained<NSData>>) -> Option<ClipboardItem> {
    // SAFETY: AppKit exports static types for PNG and TIFF representations.
    let (png, tiff) = unsafe { (NSPasteboardTypePNG, NSPasteboardTypeTIFF) };
    let jpeg = NSString::from_str("public.jpeg");
    for (kind, media_type) in [(png, "image/png"), (&*jpeg, "image/jpeg")] {
        if let Some(bytes) = data(kind) {
            return Some(ClipboardItem::Image { media_type: media_type.into(), bytes: bytes.to_vec() });
        }
    }
    let data = data(tiff)?;
    let rep = NSBitmapImageRep::initWithData(NSBitmapImageRep::alloc(), &data)?;
    // SAFETY: The PNG encoder accepts an empty bitmap property dictionary.
    let png = unsafe { rep.representationUsingType_properties(NSBitmapImageFileType::PNG, &NSDictionary::new()) }?;
    Some(ClipboardItem::Image { media_type: "image/png".into(), bytes: png.to_vec() })
}

pub(super) fn write_text(text: &str) -> bool {
    let pasteboard = NSPasteboard::generalPasteboard();
    pasteboard.clearContents();
    // SAFETY: The AppKit string type is a static pasteboard representation.
    pasteboard.setString_forType(&NSString::from_str(text), unsafe { NSPasteboardTypeString })
}

pub(super) fn write_image(image: &Image) {
    let frame = image.frame();
    if frame.width == 0 || frame.height == 0 {
        return;
    }
    let stride = frame.width as isize * 4;
    // SAFETY: AppKit owns the storage it allocates for these dimensions and row width.
    let Some(rep) = (unsafe {
        NSBitmapImageRep::initWithBitmapDataPlanes_pixelsWide_pixelsHigh_bitsPerSample_samplesPerPixel_hasAlpha_isPlanar_colorSpaceName_bitmapFormat_bytesPerRow_bitsPerPixel(
            NSBitmapImageRep::alloc(),
            std::ptr::null_mut(),
            frame.width as isize,
            frame.height as isize,
            8,
            4,
            true,
            false,
            NSCalibratedRGBColorSpace,
            NSBitmapFormat::AlphaNonpremultiplied,
            stride,
            32,
        )
    }) else {
        return;
    };
    let data = rep.bitmapData();
    if data.is_null() || rep.bytesPerRow() != stride {
        return;
    }
    // SAFETY: The bitmap dimensions and stride match the length of the image's RGBA bytes.
    unsafe { std::ptr::copy_nonoverlapping(frame.rgba.as_ptr(), data, frame.rgba.len()) };
    // SAFETY: PNG representation accepts an empty bitmap property dictionary.
    let Some(png) = (unsafe { rep.representationUsingType_properties(NSBitmapImageFileType::PNG, &NSDictionary::new()) }) else { return };
    let pasteboard = NSPasteboard::generalPasteboard();
    pasteboard.clearContents();
    // SAFETY: PNG is the representation encoded above.
    pasteboard.setData_forType(Some(&png), unsafe { NSPasteboardTypePNG });
}
