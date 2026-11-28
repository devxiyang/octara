//! Pasteboard writers and previews for AppKit drag-out.

use objc2::AllocAnyThread;
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_app_kit::{NSDraggingItem, NSImage, NSPasteboardWriting, NSWorkspace};
use objc2_foundation::{NSArray, NSData, NSPoint, NSRect, NSSize, NSString, NSURL};
use octara_model::input::DragItem;
use std::ffi::CString;
use std::os::unix::ffi::OsStrExt;
use std::ptr::NonNull;

pub(super) fn dragging_items(items: &[DragItem], point: NSPoint) -> Option<Retained<NSArray<NSDraggingItem>>> {
    let items = items.iter().map(|item| dragging_item(item, point)).collect::<Option<Vec<_>>>()?;
    Some(NSArray::from_retained_slice(&items))
}

fn dragging_item(item: &DragItem, point: NSPoint) -> Option<Retained<NSDraggingItem>> {
    let (writer, preview) = match item {
        DragItem::Text(text) => (ProtocolObject::<dyn NSPasteboardWriting>::from_retained(NSString::from_str(text)), document_icon()?),
        DragItem::File(path) => {
            if path.as_os_str().is_empty() {
                return None;
            }
            let filename = CString::new(path.as_os_str().as_bytes()).ok()?;
            // SAFETY: The NUL-terminated filesystem representation stays live for this call.
            let url = unsafe {
                NSURL::fileURLWithFileSystemRepresentation_isDirectory_relativeToURL(
                    NonNull::from(filename.as_c_str()).cast(),
                    path.is_dir(),
                    None,
                )
            };
            let preview = url.path().map(|path| NSWorkspace::sharedWorkspace().iconForFile(&path)).or_else(document_icon)?;
            (ProtocolObject::from_retained(url), preview)
        }
        DragItem::Image { bytes, .. } => {
            let image = NSImage::initWithData(NSImage::alloc(), &NSData::with_bytes(bytes)).filter(|image| image.isValid())?;
            (ProtocolObject::from_retained(image.clone()), image)
        }
    };
    let size = preview.size();
    if size.width <= 0.0 || size.height <= 0.0 || !size.width.is_finite() || !size.height.is_finite() {
        return None;
    }
    let scale = 48.0 / size.width.max(size.height);
    let size = NSSize::new(size.width * scale, size.height * scale);
    let frame = NSRect::new(NSPoint::new(point.x - size.width / 2.0, point.y - size.height / 2.0), size);
    let item = NSDraggingItem::initWithPasteboardWriter(NSDraggingItem::alloc(), &writer);
    // SAFETY: AppKit accepts NSImage contents for the dragging preview.
    unsafe { item.setDraggingFrame_contents(frame, Some(&preview)) };
    Some(item)
}

fn document_icon() -> Option<Retained<NSImage>> {
    NSImage::imageWithSystemSymbolName_accessibilityDescription(&NSString::from_str("doc.text"), None)
}
