use super::services::load_png;
use clipboard_rs::{Clipboard as _, ClipboardContent, ClipboardContext};
use octara_model::ask::{ClipboardItem, ServiceError};
use std::sync::Mutex;
use wl_clipboard_rs::copy::{Error as WaylandError, MimeSource, MimeType, Options, Source};
use x11rb::connection::RequestConnection;

const PNG_MIME: &str = "image/png";

pub(super) enum MixedClipboard {
    Wayland,
    X11,
}

impl MixedClipboard {
    pub(super) fn new() -> Self {
        // Match arboard's Wayland data-control probe so reads and writes address the same selection.
        if std::env::var_os("WAYLAND_DISPLAY").is_some() && wl_clipboard_rs::utils::is_primary_selection_supported().is_ok() {
            Self::Wayland
        } else {
            Self::X11
        }
    }

    pub(super) fn write(&self, items: &[ClipboardItem]) -> Result<(), ServiceError> {
        let (text, png) = mixed_items(items)?;
        match self {
            Self::Wayland => Options::new()
                .copy_multi(vec![
                    MimeSource { source: Source::Bytes(text.as_bytes().into()), mime_type: MimeType::Text },
                    MimeSource { source: Source::Bytes(png.into()), mime_type: MimeType::Specific(PNG_MIME.into()) },
                ])
                .map_err(|error| match error {
                    WaylandError::NoSeats | WaylandError::MissingProtocol { .. } => ServiceError::Unavailable,
                    _ => ServiceError::Failed,
                }),
            Self::X11 => write_x11(text, png),
        }
    }
}

fn mixed_items(items: &[ClipboardItem]) -> Result<(&str, &[u8]), ServiceError> {
    match items {
        [ClipboardItem::Text(text), ClipboardItem::Image { media_type, bytes }]
        | [ClipboardItem::Image { media_type, bytes }, ClipboardItem::Text(text)]
            if media_type == PNG_MIME =>
        {
            load_png(bytes)?;
            Ok((text, bytes))
        }
        _ => Err(ServiceError::Unavailable),
    }
}

struct X11Clipboard {
    owner: ClipboardContext,
    max_payload_bytes: usize,
}

fn write_x11(text: &str, png: &[u8]) -> Result<(), ServiceError> {
    // Keep one selection owner alive for the process. clipboard-rs starts its own serving thread.
    static CLIPBOARD: Mutex<Option<X11Clipboard>> = Mutex::new(None);
    let mut clipboard = CLIPBOARD.lock().map_err(|_| ServiceError::Failed)?;
    let clipboard = match clipboard.as_mut() {
        Some(clipboard) => clipboard,
        None => {
            let (connection, _) = x11rb::connect(None).map_err(|_| ServiceError::Unavailable)?;
            // clipboard-rs serves each target in a single ChangeProperty request. Reserve room for the request header.
            let max_payload_bytes = connection.maximum_request_bytes().saturating_sub(28);
            let owner = ClipboardContext::new().map_err(|_| ServiceError::Failed)?;
            clipboard.insert(X11Clipboard { owner, max_payload_bytes })
        }
    };
    if text.len().max(png.len()) > clipboard.max_payload_bytes {
        return Err(ServiceError::Unavailable);
    }
    clipboard
        .owner
        .set(vec![ClipboardContent::Text(text.into()), ClipboardContent::Other(PNG_MIME.into(), png.into())])
        .map_err(|_| ServiceError::Failed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{ExtendedColorType, ImageEncoder, codecs::png::PngEncoder};

    #[test]
    fn mixed_clipboard_accepts_either_order_and_preserves_png_bytes() {
        let mut png = Vec::new();
        PngEncoder::new(&mut png).write_image(&[45, 120, 190, 224], 1, 1, ExtendedColorType::Rgba8).unwrap();
        let text = ClipboardItem::Text("Clipboard ☃".into());
        let image = ClipboardItem::Image { media_type: PNG_MIME.into(), bytes: png.clone() };
        assert_eq!(mixed_items(&[text.clone(), image.clone()]), Ok(("Clipboard ☃", png.as_slice())));
        assert_eq!(mixed_items(&[image, text]), Ok(("Clipboard ☃", png.as_slice())));
    }

    #[test]
    fn invalid_and_unsupported_pairs_fail_before_contacting_the_clipboard() {
        let text = ClipboardItem::Text("Clipboard text".into());
        let invalid_png = ClipboardItem::Image { media_type: PNG_MIME.into(), bytes: vec![] };
        let jpeg = ClipboardItem::Image { media_type: "image/jpeg".into(), bytes: vec![] };
        let backend = MixedClipboard::X11;
        assert_eq!(backend.write(&[text.clone(), invalid_png]), Err(ServiceError::Failed));
        assert_eq!(backend.write(&[text.clone(), jpeg]), Err(ServiceError::Unavailable));
        assert_eq!(backend.write(&[text.clone(), text]), Err(ServiceError::Unavailable));
    }
}
