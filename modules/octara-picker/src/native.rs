use std::fs::File;
use std::io::{self, ErrorKind, Read};
use std::path::{Path, PathBuf};

#[cfg(any(target_os = "macos", target_os = "ios"))]
use super::ModuleCall;
use super::{Job, ModuleError, PickedFile, Result, too_large};

#[cfg(any(target_os = "macos", target_os = "ios"))]
pub(super) fn read_selected<C: ModuleCall<Output = Vec<PickedFile>>>(paths: Vec<PathBuf>, max_bytes: Option<usize>, job: Job) {
    if max_bytes.is_none() {
        job.finish::<C>(selected(paths, None, &job));
        return;
    }
    let worker = job.clone();
    if std::thread::Builder::new()
        .name("octara-picker-read".into())
        .spawn(move || {
            worker.finish::<C>(selected(paths, max_bytes, &worker));
        })
        .is_err()
    {
        job.finish::<C>(Err(ModuleError::Failed("could not start file reader".into())));
    }
}

pub(super) fn selected(paths: Vec<PathBuf>, max_bytes: Option<usize>, job: &Job) -> Result<Vec<PickedFile>> {
    if paths.is_empty() {
        return Err(ModuleError::Cancelled);
    }
    paths
        .into_iter()
        .map(|path| {
            job.check_live()?;
            let name =
                path.file_name().ok_or_else(|| ModuleError::Failed("selected file has no filename".into()))?.to_string_lossy().into_owned();
            let media_type = mime_type(&path).into();
            let bytes = max_bytes.map(|limit| read_file(&path, Some(limit), job)).transpose()?;
            Ok(PickedFile { path: Some(path), name, media_type, bytes })
        })
        .collect()
}

fn io_error(error: io::Error) -> ModuleError {
    match error.kind() {
        ErrorKind::PermissionDenied => ModuleError::PermissionDenied,
        _ => ModuleError::Failed(format!("could not read the selected file ({})", error.kind())),
    }
}

pub(super) fn read_file(path: &Path, max_bytes: Option<usize>, job: &Job) -> Result<Vec<u8>> {
    job.check_live()?;
    if !path.metadata().map_err(io_error)?.is_file() {
        return Err(ModuleError::Failed("selection is not a regular file".into()));
    }
    let file = open_file(path).map_err(io_error)?;
    let metadata = file.metadata().map_err(io_error)?;
    if !metadata.is_file() {
        return Err(ModuleError::Failed("selection is not a regular file".into()));
    }
    let max_bytes = max_bytes.unwrap_or(usize::MAX);
    if metadata.len() > max_bytes as u64 {
        return Err(too_large(max_bytes));
    }
    read_limited(file, max_bytes, job)
}

fn open_file(path: &Path) -> io::Result<File> {
    #[cfg(unix)]
    {
        use rustix::fs::{Mode, OFlags};
        // A regular file may have been replaced with a pipe since the metadata check.
        Ok(File::from(rustix::fs::open(path, OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NONBLOCK, Mode::empty())?))
    }
    #[cfg(not(unix))]
    {
        File::open(path)
    }
}

fn read_limited(mut source: impl Read, max_bytes: usize, job: &Job) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    let mut buffer = [0; 8192];
    loop {
        job.check_live()?;
        let remaining = max_bytes - bytes.len();
        let limit = remaining.clamp(1, buffer.len());
        match source.read(&mut buffer[..limit]) {
            Ok(0) => return Ok(bytes),
            Ok(n) if n > remaining => return Err(too_large(max_bytes)),
            Ok(n) => {
                bytes.try_reserve(n).map_err(|_| ModuleError::Failed("could not allocate space for the selected file".into()))?;
                bytes.extend_from_slice(&buffer[..n]);
            }
            Err(error) if error.kind() == ErrorKind::Interrupted => continue,
            Err(error) => return Err(io_error(error)),
        }
    }
}

pub(super) fn mime_type(path: &Path) -> &'static str {
    let Some(extension) = path.extension().and_then(|ext| ext.to_str()) else { return "" };
    MEDIA_TYPES
        .iter()
        .find_map(|(mime, extensions)| extensions.iter().any(|ext| extension.eq_ignore_ascii_case(ext)).then_some(*mime))
        .unwrap_or("")
}

#[cfg(any(
    target_os = "windows",
    target_os = "linux",
    target_os = "freebsd",
    target_os = "dragonfly",
    target_os = "netbsd",
    target_os = "openbsd"
))]
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
        } else if let Some(group) = entry.strip_suffix("/*") {
            let before = extensions.len();
            extensions.extend(
                MEDIA_TYPES
                    .iter()
                    .filter(|(mime, _)| mime.split_once('/').is_some_and(|(top, _)| top == group))
                    .flat_map(|(_, extensions)| extensions.iter().map(ToString::to_string)),
            );
            if extensions.len() == before {
                return Vec::new();
            }
        } else if entry.contains('/') {
            // The desktop filter takes extensions and cannot express an unknown MIME type.
            let Some((_, matched)) = MEDIA_TYPES.iter().find(|(mime, _)| *mime == entry) else { return Vec::new() };
            extensions.extend(matched.iter().map(ToString::to_string));
        }
    }
    extensions.sort_unstable();
    extensions.dedup();
    extensions
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
    ("image/heif", &["heif"]),
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
    ("video/mpeg", &["mpeg", "mpg"]),
    ("video/quicktime", &["mov"]),
    ("video/webm", &["webm"]),
    ("video/x-matroska", &["mkv"]),
    ("video/x-msvideo", &["avi"]),
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CANCELLED, WORKING};
    use octara::module::CallTicket;
    use std::sync::{
        Arc,
        atomic::{AtomicU8, Ordering},
    };

    #[test]
    fn bounded_reads_distinguish_exact_limit_overflow_and_cancellation() {
        let job = Job { ticket: CallTicket(1), dispatcher: Default::default(), state: Arc::new(AtomicU8::new(WORKING)) };
        assert_eq!(read_limited(&b"abc"[..], 3, &job).unwrap(), b"abc");
        assert_eq!(read_limited(&b""[..], 0, &job).unwrap(), b"");
        assert_eq!(read_limited(&b"abc"[..], 2, &job).unwrap_err(), too_large(2));
        job.state.store(CANCELLED, Ordering::Release);
        assert_eq!(read_limited(&b"abc"[..], 3, &job).unwrap_err(), ModuleError::Cancelled);
    }
}
