//! AppKit pasteboard services.

use crate::os::apple_media;
use objc2::{ClassType, rc::Retained, runtime::ProtocolObject};
use objc2_app_kit::{
    NSPasteboard, NSPasteboardItem, NSPasteboardTypePNG, NSPasteboardTypeString, NSPasteboardTypeTIFF,
    NSPasteboardURLReadingFileURLsOnlyKey, NSPasteboardWriting,
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
    // SAFETY: The pasteboard returns data that is not mutated during the decode.
    let image = apple_media::decode(unsafe { data.as_bytes_unchecked() }).ok()?;
    let bytes = apple_media::encode_png(&image.frame()).ok()?;
    Some(ClipboardItem::Image { media_type: "image/png".into(), bytes })
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
    let Ok(png) = apple_media::encode_png(&image.frame()) else { return };
    let png = NSData::with_bytes(&png);
    let pasteboard = NSPasteboard::generalPasteboard();
    pasteboard.clearContents();
    // SAFETY: PNG is the representation encoded above.
    pasteboard.setData_forType(Some(&png), unsafe { NSPasteboardTypePNG });
}

#[cfg(test)]
mod tests {
    use super::*;
    use objc2::AllocAnyThread;
    use objc2_app_kit::NSBitmapImageRep;

    #[test]
    fn clipboard_write_offers_text_and_png_in_one_update() {
        let pasteboard = NSPasteboard::pasteboardWithUniqueName();
        let bytes = apple_media::encode_png(&Image::rgba(1, 1, [45, 120, 190, 224]).frame()).unwrap();
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

    #[test]
    fn clipboard_transcodes_tiff_to_png() {
        let png = NSData::with_bytes(include_bytes!("../apple_media/fixtures/sample.png"));
        let bitmap = NSBitmapImageRep::initWithData(NSBitmapImageRep::alloc(), &png).unwrap();
        let tiff = bitmap.TIFFRepresentation().unwrap();
        let entry = NSPasteboardItem::new();
        // SAFETY: AppKit exports a static type for the TIFF representation.
        assert!(entry.setData_forType(&tiff, unsafe { NSPasteboardTypeTIFF }));
        let ClipboardItem::Image { media_type, bytes } = clipboard_image(|kind| entry.dataForType(kind)).unwrap() else {
            panic!("Expected a clipboard image");
        };
        assert_eq!(media_type, "image/png");
        assert!(bytes.starts_with(b"\x89PNG\r\n\x1a\n"));
        assert_eq!(apple_media::decode(&bytes).unwrap().size(), (24, 16));
    }
}
