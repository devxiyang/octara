//! AppKit pasteboard and scoped file picking.

use super::asks::later;
use crate::os::ServiceCleanup;
use objc2::{AllocAnyThread, ClassType, MainThreadMarker, Message, rc::Retained};
use objc2_app_kit::{
    NSBitmapFormat, NSBitmapImageFileType, NSBitmapImageRep, NSCalibratedRGBColorSpace, NSModalResponseCancel, NSModalResponseOK,
    NSOpenPanel, NSPasteboard, NSPasteboardTypePNG, NSPasteboardTypeString, NSPasteboardTypeTIFF, NSPasteboardURLReadingFileURLsOnlyKey,
    NSView,
};
use objc2_foundation::{NSArray, NSData, NSDictionary, NSNumber, NSString, NSURL};
use octara_model::ask::{ClipboardItem, ClipboardKinds, ServiceError, ServiceOutcome};
use octara_model::paint::Image;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

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

type Panel = RefCell<Option<Retained<NSOpenPanel>>>;

struct Pick {
    panel: Rc<Panel>,
    live: Arc<AtomicBool>,
}

impl Pick {
    fn cancel(&self) {
        self.live.store(false, Ordering::Relaxed);
        let panel = self.panel.borrow_mut().take();
        if let Some(panel) = panel {
            // SAFETY: The AppKit cancel action accepts a nil sender.
            unsafe { panel.cancel(None) };
        }
    }
}

#[derive(Default)]
pub(super) struct Picker(Option<Rc<Pick>>);

impl Picker {
    pub(super) fn start(
        &mut self,
        view: &NSView,
        max_bytes: usize,
        accept: &str,
        send: impl Fn(ServiceOutcome) + Send + Sync + 'static,
    ) -> Option<ServiceCleanup> {
        if self.0.as_ref().is_some_and(|request| request.live.load(Ordering::Relaxed)) {
            send(ServiceOutcome::Error(ServiceError::Busy));
            return Some(Box::new(|| {}));
        }
        let request = Rc::new(Pick { panel: Rc::default(), live: Arc::new(AtomicBool::new(true)) });
        self.0 = Some(request.clone());
        let view = view.retain();
        let extensions = crate::os::files::accept_extensions(accept);
        let send = Arc::new(send);
        let scheduled = Rc::downgrade(&request);
        later(move || {
            let Some(request) = scheduled.upgrade().filter(|request| request.live.load(Ordering::Relaxed)) else { return };
            let sender = send.clone();
            let Some((mtm, parent)) = MainThreadMarker::new().zip(view.window()) else {
                finish(&request.live, &*sender, ServiceOutcome::Error(ServiceError::Unavailable));
                return;
            };
            if parent.attachedSheet().is_some() {
                finish(&request.live, &*sender, ServiceOutcome::Error(ServiceError::Busy));
                return;
            }
            let panel = NSOpenPanel::openPanel(mtm);
            panel.setCanChooseFiles(true);
            panel.setCanChooseDirectories(false);
            panel.setAllowsMultipleSelection(false);
            let filters = (!extensions.is_empty()).then(|| {
                let extensions = extensions.iter().map(|extension| NSString::from_str(extension)).collect::<Vec<_>>();
                NSArray::from_retained_slice(&extensions)
            });
            // The extension-only AppKit API also supports unregistered extensions.
            #[allow(deprecated)]
            panel.setAllowedFileTypes(filters.as_deref());
            let pending = Rc::downgrade(&request.panel);
            let live = request.live.clone();
            let done = block2::RcBlock::new(move |response| {
                let Some(panel) = pending.upgrade().and_then(|slot| slot.borrow_mut().take()) else { return };
                panel.orderOut(None);
                if response == NSModalResponseCancel {
                    finish(&live, &*sender, ServiceOutcome::FilePicked(None));
                } else if response == NSModalResponseOK
                    && let Some(path) = panel.URL().and_then(|url| url.to_file_path())
                {
                    let active = live.clone();
                    let answer = sender.clone();
                    let read = std::thread::Builder::new().name("octara-file-read".into()).spawn(move || {
                        let result = crate::os::files::read_file(&path, max_bytes, &active)
                            .map_or_else(ServiceOutcome::Error, |file| ServiceOutcome::FilePicked(Some(file)));
                        finish(&active, &*answer, result);
                    });
                    if read.is_err() {
                        finish(&live, &*sender, ServiceOutcome::Error(ServiceError::Failed));
                    }
                } else {
                    finish(&live, &*sender, ServiceOutcome::Error(ServiceError::Failed));
                }
            });
            request.panel.replace(Some(panel.clone()));
            panel.beginSheetModalForWindow_completionHandler(&parent, &done);
        });
        Some(Box::new(move || request.cancel()))
    }
}

impl Drop for Picker {
    fn drop(&mut self) {
        if let Some(request) = &self.0 {
            request.cancel();
        }
    }
}

fn finish(live: &AtomicBool, send: &impl Fn(ServiceOutcome), result: ServiceOutcome) {
    if live.swap(false, Ordering::AcqRel) {
        send(result);
    }
}
