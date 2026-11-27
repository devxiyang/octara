use objc2::Message;
use objc2::rc::Retained;
use objc2_core_foundation::{CFRunLoop, kCFRunLoopDefaultMode};
use objc2_foundation::{NSArray, NSString};
use objc2_uniform_type_identifiers::{UTType, UTTypeAudio, UTTypeImage, UTTypeMovie};

use super::{MediaKind, ModuleError, Result};

pub(super) fn later(f: impl Fn() + 'static) -> Result<()> {
    let run_loop = CFRunLoop::main().ok_or(ModuleError::Unavailable)?;
    let block = block2::RcBlock::new(f);
    // SAFETY: The main run loop copies the block and executes it on the UI thread.
    unsafe { run_loop.perform_block(kCFRunLoopDefaultMode.map(|mode| &**mode), Some(&block)) };
    run_loop.wake_up();
    Ok(())
}

pub(super) fn media_types(kind: MediaKind) -> Vec<Retained<UTType>> {
    // SAFETY: The imported constants are immutable system types.
    unsafe {
        match kind {
            MediaKind::Photos => vec![UTTypeImage.retain()],
            MediaKind::Videos => vec![UTTypeMovie.retain()],
            MediaKind::PhotosAndVideos => vec![UTTypeImage.retain(), UTTypeMovie.retain()],
        }
    }
}

pub(super) fn accept_types(accept: &str) -> Option<Retained<NSArray<UTType>>> {
    let types = accept
        .split(',')
        .map(str::trim)
        .filter(|hint| !hint.is_empty())
        .map(|hint| {
            // SAFETY: The imported constants are immutable system types.
            unsafe {
                match hint.to_ascii_lowercase().as_str() {
                    "image/*" => Some(UTTypeImage.retain()),
                    "video/*" => Some(UTTypeMovie.retain()),
                    "audio/*" => Some(UTTypeAudio.retain()),
                    hint if hint.starts_with('.') => UTType::typeWithFilenameExtension(&NSString::from_str(&hint[1..])),
                    hint => UTType::typeWithMIMEType(&NSString::from_str(hint)),
                }
            }
        })
        .collect::<Option<Vec<_>>>()?;
    (!types.is_empty()).then(|| NSArray::from_retained_slice(&types))
}
