//! Shared filter parsing and bounded reads for desktop file chooser services.

use octara_model::ask::{PickedFile, ServiceError};
use std::fs::File;
use std::io::{self, ErrorKind, Read};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

fn file_error(error: io::Error) -> ServiceError {
    match error.kind() {
        ErrorKind::PermissionDenied => ServiceError::Denied,
        _ => ServiceError::Failed,
    }
}

pub(super) fn read_file(path: &Path, max_bytes: usize, live: &AtomicBool) -> Result<PickedFile, ServiceError> {
    if !live.load(Ordering::SeqCst) {
        return Err(ServiceError::Cancelled);
    }
    let name = path.file_name().ok_or(ServiceError::Failed)?.to_string_lossy().into_owned();
    if !path.metadata().map_err(file_error)?.is_file() {
        return Err(ServiceError::Failed);
    }
    let file = open_file(path).map_err(file_error)?;
    let metadata = file.metadata().map_err(file_error)?;
    if !metadata.is_file() {
        return Err(ServiceError::Failed);
    }
    if metadata.len() > max_bytes as u64 {
        return Err(ServiceError::TooLarge { bytes: metadata.len(), max_bytes });
    }
    let bytes = read_limited(file, max_bytes, live)?;
    let media_type = path.extension().and_then(|extension| extension.to_str()).map_or("", media_type).into();
    Ok(PickedFile { name, media_type, bytes })
}

fn open_file(path: &Path) -> io::Result<File> {
    #[cfg(unix)]
    {
        use rustix::fs::{Mode, OFlags};
        // Handle a regular file being replaced by a pipe between metadata and open.
        Ok(File::from(rustix::fs::open(path, OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NONBLOCK, Mode::empty())?))
    }
    #[cfg(not(unix))]
    {
        File::open(path)
    }
}

fn read_limited(mut source: impl Read, max_bytes: usize, live: &AtomicBool) -> Result<Vec<u8>, ServiceError> {
    let mut bytes = Vec::new();
    let mut buffer = [0; 8192];
    loop {
        if !live.load(Ordering::SeqCst) {
            return Err(ServiceError::Cancelled);
        }
        let remaining = max_bytes - bytes.len();
        let limit = remaining.clamp(1, buffer.len());
        match source.read(&mut buffer[..limit]) {
            Ok(0) => return Ok(bytes),
            Ok(n) if n > remaining => return Err(ServiceError::TooLarge { bytes: (max_bytes as u64).saturating_add(1), max_bytes }),
            Ok(n) => {
                let len = bytes.len() + n;
                if len > bytes.capacity() {
                    let capacity = len.max(bytes.capacity().saturating_mul(2)).min(max_bytes);
                    bytes.try_reserve_exact(capacity - bytes.len()).map_err(|_| ServiceError::Failed)?;
                }
                bytes.extend_from_slice(&buffer[..n]);
            }
            Err(error) if error.kind() == ErrorKind::Interrupted => continue,
            Err(error) => return Err(file_error(error)),
        }
    }
}
pub(super) fn accept_extensions(accept: &str) -> Vec<String> {
    let mut extensions = Vec::new();
    for entry in accept.split(',').map(str::trim).map(str::to_ascii_lowercase) {
        if let Some(extension) = entry.strip_prefix('.') {
            if !extension.is_empty()
                && extension.split('.').all(|part| !part.is_empty())
                && extension.bytes().all(|byte| byte.is_ascii_alphanumeric() || b"._-+".contains(&byte))
            {
                extensions.push(extension.to_owned());
            }
        } else if let Some((top, subtype)) = entry.split_once('/')
            && !top.is_empty()
            && !subtype.is_empty()
            && entry.bytes().all(|byte| byte.is_ascii_alphanumeric() || b"./-+*".contains(&byte))
        {
            let matched = MEDIA_TYPES.iter().filter(|(mime, _)| {
                let (group, _) = mime.split_once('/').expect("media type");
                *mime == entry || subtype == "*" && group == top
            });
            let before = extensions.len();
            extensions.extend(matched.flat_map(|(_, extensions)| extensions.iter().map(ToString::to_string)));
            // Extension-only chooser filters cannot express an unmapped MIME hint.
            if extensions.len() == before {
                return Vec::new();
            }
        }
    }
    extensions.sort_unstable();
    extensions.dedup();
    extensions
}

fn media_type(extension: &str) -> &str {
    MEDIA_TYPES
        .iter()
        .find_map(|(mime, extensions)| extensions.iter().any(|ext| extension.eq_ignore_ascii_case(ext)).then_some(*mime))
        .unwrap_or("")
}

const MEDIA_TYPES: &[(&str, &[&str])] = &[
    ("application/gzip", &["gz"]),
    ("application/json", &["json"]),
    ("application/pdf", &["pdf"]),
    ("application/rtf", &["rtf"]),
    ("application/xml", &["xml"]),
    ("application/zip", &["zip"]),
    ("audio/flac", &["flac"]),
    ("audio/mpeg", &["mp3"]),
    ("audio/mp4", &["m4a"]),
    ("audio/ogg", &["oga", "ogg", "opus"]),
    ("audio/wav", &["wav"]),
    ("image/avif", &["avif"]),
    ("image/bmp", &["bmp"]),
    ("image/gif", &["gif"]),
    ("image/heic", &["heic"]),
    ("image/jpeg", &["jpeg", "jpg", "jpe"]),
    ("image/png", &["png"]),
    ("image/svg+xml", &["svg", "svgz"]),
    ("image/tiff", &["tif", "tiff"]),
    ("image/webp", &["webp"]),
    ("text/csv", &["csv"]),
    ("text/html", &["htm", "html"]),
    ("text/markdown", &["markdown", "md"]),
    ("text/plain", &["txt"]),
    ("video/mp4", &["m4v", "mp4"]),
    ("video/quicktime", &["mov"]),
    ("video/webm", &["webm"]),
];
