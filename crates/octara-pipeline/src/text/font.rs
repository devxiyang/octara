//! Read-only file backing for fonts shipped as individual assets.

use memmap2::Mmap;
use octara_model::paint::FontData;
use std::fs::File;
use std::io;
use std::path::Path;
use std::sync::Arc;

struct MappedFont(Mmap);

impl AsRef<[u8]> for MappedFont {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

#[cfg(unix)]
impl Drop for MappedFont {
    fn drop(&mut self) {
        // SAFETY: The last owner is dropping the read-only mapping. No borrowed slices
        // can remain live, and the caller guarantees the underlying file's immutability.
        let _ = unsafe { self.0.unchecked_advise(memmap2::UncheckedAdvice::DontNeed) };
    }
}

/// # Safety
///
/// The file must not be modified or truncated for the life of the returned data.
pub(crate) unsafe fn map_font(path: &Path) -> io::Result<FontData> {
    let file = File::open(path)?;
    // SAFETY: The caller guarantees that the opened file is immutable.
    let map = unsafe { Mmap::map(&file)? };
    #[cfg(unix)]
    let _ = map.advise(memmap2::Advice::WillNeed);
    Ok(FontData::new(Arc::new(MappedFont(map))))
}
