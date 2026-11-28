use octara_model::{
    ask::{ClipboardItem, ServiceError},
    input::DragItem,
};
use std::path::{Path, PathBuf};

const URIS: &str = "text/uri-list";
const TEXT: &str = "text/plain";
const UTF8: &str = "text/plain;charset=utf-8";

type Formats = Vec<(&'static str, Vec<u8>)>;

pub(super) fn files_from_uri_list(bytes: &[u8]) -> Vec<PathBuf> {
    std::str::from_utf8(bytes)
        .unwrap_or_default()
        .lines()
        .filter_map(|line| url::Url::parse(line.trim()).ok()?.to_file_path().ok())
        .collect()
}

pub(super) fn clipboard_formats(items: &[ClipboardItem]) -> Result<Formats, ServiceError> {
    let mut formats = Offers::default();
    for item in items {
        match item {
            ClipboardItem::Text(value) => formats.push_text(value),
            ClipboardItem::Files(paths) => {
                for path in paths {
                    formats.push_file(path)?;
                }
            }
            ClipboardItem::Image { media_type, bytes } if media_type == "image/png" => {
                super::services::load_png(bytes)?;
                formats.push_image("image/png", bytes)?;
            }
            ClipboardItem::Image { .. } => return Err(ServiceError::Unavailable),
        }
    }
    formats.finish()
}

pub(super) fn drag_formats(items: &[DragItem]) -> Result<Formats, ServiceError> {
    let mut formats = Offers::default();
    for item in items {
        match item {
            DragItem::Text(value) => formats.push_text(value),
            DragItem::File(path) => formats.push_file(path)?,
            DragItem::Image { media_type, bytes } => {
                let mime = match media_type.as_str() {
                    "image/png" => "image/png",
                    "image/jpeg" => "image/jpeg",
                    "image/gif" => "image/gif",
                    "image/webp" => "image/webp",
                    _ => return Err(ServiceError::Unavailable),
                };
                formats.push_image(mime, bytes)?;
            }
        }
    }
    formats.finish()
}

#[derive(Default)]
struct Offers {
    text: Option<String>,
    files: String,
    images: Formats,
}

impl Offers {
    fn push_text(&mut self, value: &str) {
        match &mut self.text {
            Some(text) => {
                text.push('\n');
                text.push_str(value);
            }
            None => self.text = Some(value.to_owned()),
        }
    }

    fn push_file(&mut self, path: &Path) -> Result<(), ServiceError> {
        let url = url::Url::from_file_path(path).map_err(|_| ServiceError::Unavailable)?;
        self.files.push_str(url.as_str());
        self.files.push_str("\r\n");
        Ok(())
    }

    fn push_image(&mut self, mime: &'static str, bytes: &[u8]) -> Result<(), ServiceError> {
        if bytes.is_empty() || self.images.iter().any(|(offered, _)| *offered == mime) {
            return Err(ServiceError::Unavailable);
        }
        self.images.push((mime, bytes.to_vec()));
        Ok(())
    }

    fn finish(mut self) -> Result<Formats, ServiceError> {
        if let Some(text) = self.text {
            let bytes = text.into_bytes();
            self.images.push((UTF8, bytes.clone()));
            self.images.push((TEXT, bytes));
        }
        if !self.files.is_empty() {
            self.images.push((URIS, self.files.into_bytes()));
        }
        if self.images.is_empty() { Err(ServiceError::Unavailable) } else { Ok(self.images) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offers_utf8_text_and_percent_encoded_paths() {
        let formats = drag_formats(&[DragItem::Text("drag text".into()), DragItem::File("/tmp/example #1".into())]).unwrap();
        assert!(formats.iter().any(|(mime, bytes)| *mime == UTF8 && bytes == b"drag text"));
        let (_, paths) = formats.iter().find(|(mime, _)| *mime == URIS).unwrap();
        assert_eq!(paths, b"file:///tmp/example%20%231\r\n");
        assert_eq!(files_from_uri_list(paths), [PathBuf::from("/tmp/example #1")]);
    }
}
