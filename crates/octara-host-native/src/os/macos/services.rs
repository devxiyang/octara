//! AppKit pasteboard services.

use objc2::{AllocAnyThread, ClassType, rc::Retained, runtime::ProtocolObject};
use objc2_app_kit::{
    NSBitmapFormat, NSBitmapImageFileType, NSBitmapImageRep, NSCalibratedRGBColorSpace, NSPasteboard, NSPasteboardItem,
    NSPasteboardTypePNG, NSPasteboardTypeString, NSPasteboardTypeTIFF, NSPasteboardURLReadingFileURLsOnlyKey, NSPasteboardWriting,
};
use objc2_foundation::{NSArray, NSData, NSDictionary, NSNumber, NSString, NSURL};
use octara_model::ask::{ClipboardItem, ClipboardKinds, ServiceError};
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

pub(super) fn write_clipboard(items: &[ClipboardItem]) -> Result<(), ServiceError> {
    write_items(&NSPasteboard::generalPasteboard(), items)
}

fn write_items(pasteboard: &NSPasteboard, items: &[ClipboardItem]) -> Result<(), ServiceError> {
    let mut entries = Vec::<Retained<ProtocolObject<dyn NSPasteboardWriting>>>::new();
    for item in items {
        match item {
            ClipboardItem::Text(text) => entries.push(ProtocolObject::from_retained(NSString::from_str(text))),
            ClipboardItem::Image { media_type, bytes } if media_type == "image/png" => {
                let entry = NSPasteboardItem::new();
                // SAFETY: AppKit exports a static type for the PNG representation.
                if !entry.setData_forType(&NSData::with_bytes(bytes), unsafe { NSPasteboardTypePNG }) {
                    return Err(ServiceError::Failed);
                }
                entries.push(ProtocolObject::from_retained(entry));
            }
            ClipboardItem::Files(paths) if !paths.is_empty() => {
                for path in paths {
                    let url = NSURL::from_file_path(path).ok_or(ServiceError::Failed)?;
                    entries.push(ProtocolObject::from_retained(url));
                }
            }
            _ => return Err(ServiceError::Unavailable),
        }
    }
    if entries.is_empty() {
        return Err(ServiceError::Unavailable);
    }
    let entries = NSArray::from_retained_slice(&entries);
    pasteboard.clearContents();
    pasteboard.writeObjects(&entries).then_some(()).ok_or(ServiceError::Failed)
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

#[cfg(test)]
mod tests {
    use super::*;
    use image::{ExtendedColorType, ImageEncoder, codecs::png::PngEncoder};

    #[test]
    fn clipboard_write_offers_text_and_png_in_one_update() {
        let pasteboard = NSPasteboard::pasteboardWithUniqueName();
        let mut bytes = Vec::new();
        PngEncoder::new(&mut bytes).write_image(&[45, 120, 190, 224], 1, 1, ExtendedColorType::Rgba8).unwrap();
        let items = [ClipboardItem::Text("Clipboard example".into()), ClipboardItem::Image { media_type: "image/png".into(), bytes }];
        write_items(&pasteboard, &items).unwrap();

        let previous = pasteboard.changeCount();
        let unsupported = ClipboardItem::Image { media_type: "image/gif".into(), bytes: Vec::new() };
        assert_eq!(write_items(&pasteboard, &[items[0].clone(), unsupported]), Err(ServiceError::Unavailable));
        assert_eq!(pasteboard.changeCount(), previous);

        let entries = pasteboard.pasteboardItems().unwrap();
        assert_eq!(entries.len(), items.len());
        // SAFETY: AppKit exports a static type for the string representation.
        let text = entries.objectAtIndex(0).stringForType(unsafe { NSPasteboardTypeString }).unwrap();
        let image = clipboard_image(|kind| entries.objectAtIndex(1).dataForType(kind)).unwrap();
        assert_eq!([ClipboardItem::Text(text.to_string()), image], items);
        pasteboard.clearContents();
    }
}
