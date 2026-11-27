//! Bounded header probing. Returns dimensions only from recognized image containers.

/// Reads dimensions available in the first 16 KiB of a PNG, JPEG, GIF, WebP or AVIF.
pub fn probe_image_size(bytes: &[u8]) -> Option<(u32, u32)> {
    let bytes = &bytes[..bytes.len().min(16 * 1024)];
    let (width, height) = if bytes.starts_with(b"\x89PNG\r\n\x1a\n") && bytes.get(12..16)? == b"IHDR" {
        (be32(bytes.get(16..20)?), be32(bytes.get(20..24)?))
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        (u16::from_le_bytes(bytes.get(6..8)?.try_into().ok()?).into(), u16::from_le_bytes(bytes.get(8..10)?.try_into().ok()?).into())
    } else if bytes.starts_with(b"\xff\xd8") {
        jpeg(&bytes[2..])?
    } else if bytes.starts_with(b"RIFF") && bytes.get(8..12)? == b"WEBP" {
        webp(&bytes[12..])?
    } else if bytes.get(4..8)? == b"ftyp" {
        let ftyp = bytes.get(8..(be32(&bytes[..4]) as usize).min(bytes.len()))?;
        if !ftyp.as_chunks::<4>().0.iter().any(|brand| brand == b"avif" || brand == b"avis") {
            return None;
        }
        avif(bytes, 0)?
    } else {
        return None;
    };
    (width != 0 && height != 0).then_some((width, height))
}

fn be32(bytes: &[u8]) -> u32 {
    u32::from_be_bytes(bytes.try_into().expect("four-byte header field"))
}

fn jpeg(mut bytes: &[u8]) -> Option<(u32, u32)> {
    while let Some(rest) = bytes.strip_prefix(&[0xff]) {
        let index = rest.iter().position(|&byte| byte != 0xff)?;
        let (&marker, rest) = rest.get(index..)?.split_first()?;
        if matches!(marker, 0xda | 0xd9 | 0) {
            return None;
        }
        if matches!(marker, 0x01 | 0xd0..=0xd7) {
            bytes = rest;
            continue;
        }
        let length = u16::from_be_bytes(rest.get(..2)?.try_into().ok()?) as usize;
        if length < 2 {
            return None;
        }
        if matches!(marker, 0xc0..=0xc3 | 0xc5..=0xc7 | 0xc9..=0xcb | 0xcd..=0xcf) && length >= 7 {
            let height = u16::from_be_bytes(rest.get(3..5)?.try_into().ok()?);
            let width = u16::from_be_bytes(rest.get(5..7)?.try_into().ok()?);
            return Some((width.into(), height.into()));
        }
        bytes = rest.get(length..)?;
    }
    None
}

fn webp(bytes: &[u8]) -> Option<(u32, u32)> {
    let data = bytes.get(8..)?;
    match bytes.get(..4)? {
        b"VP8X" => {
            let le24 = |start| -> Option<u32> {
                let &[a, b, c] = data.get(start..start + 3)? else { return None };
                Some(u32::from_le_bytes([a, b, c, 0]) + 1)
            };
            Some((le24(4)?, le24(7)?))
        }
        b"VP8L" if data.first() == Some(&0x2f) => {
            let bits = u32::from_le_bytes(data.get(1..5)?.try_into().ok()?);
            Some(((bits & 0x3fff) + 1, ((bits >> 14) & 0x3fff) + 1))
        }
        b"VP8 " if data.get(3..6)? == b"\x9d\x01\x2a" => {
            let width = u16::from_le_bytes(data.get(6..8)?.try_into().ok()?) & 0x3fff;
            let height = u16::from_le_bytes(data.get(8..10)?.try_into().ok()?) & 0x3fff;
            Some((width.into(), height.into()))
        }
        _ => None,
    }
}

fn avif(mut bytes: &[u8], depth: u8) -> Option<(u32, u32)> {
    if depth == 4 {
        return None;
    }
    while let Some(box_header) = bytes.get(..8) {
        let mut header_len = 8;
        let len = match be32(&box_header[..4]) {
            0 => bytes.len(),
            1 => {
                header_len = 16;
                usize::try_from(u64::from_be_bytes(bytes.get(8..16)?.try_into().ok()?)).ok()?
            }
            length => length as usize,
        };
        let contents = bytes.get(header_len..len.min(bytes.len()))?;
        let found = match &box_header[4..] {
            b"meta" => avif(contents.get(4..)?, depth + 1),
            b"iprp" | b"ipco" => avif(contents, depth + 1),
            b"ispe" => Some((be32(contents.get(4..8)?), be32(contents.get(8..12)?))),
            _ => None,
        };
        if found.is_some() {
            return found;
        }
        bytes = bytes.get(len..)?;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::probe_image_size;

    #[test]
    fn probes_partial_headers_for_supported_formats() {
        let png = b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR\0\0\0\x20\0\0\0\x10";
        let jpeg = b"\xff\xd8\xff\xe0\0\x04AB\xff\xc2\0\x10\x08\0\x10\0\x20";
        let gif = b"GIF89a\x20\0\x10\0";
        let webp = b"RIFF\xff\0\0\0WEBPVP8X\x0a\0\0\0\0\0\0\0\x1f\0\0\x0f\0\0";
        let avif =
            b"\0\0\0\x10ftypavif\0\0\0\0\0\0\x01\x00meta\0\0\0\0\0\0\0\x24iprp\0\0\0\x1cipco\0\0\0\x14ispe\0\0\0\0\0\0\0\x20\0\0\0\x10";
        for bytes in [png.as_slice(), jpeg, gif, webp, avif] {
            assert_eq!(probe_image_size(bytes), Some((32, 16)));
            for length in 0..bytes.len() {
                assert_eq!(probe_image_size(&bytes[..length]), None);
            }
        }
    }

    #[test]
    fn ignores_zero_or_unrecognized_dimensions() {
        assert_eq!(probe_image_size(b"GIF89a\0\0\x10\0"), None);
        assert_eq!(probe_image_size(b"\xff\xd8\xff\xe1\0\0"), None);
        assert_eq!(probe_image_size(b"\0\0\0\x08ispe"), None);
    }
}
