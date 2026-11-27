//! Markdown media authored through Octara's viewport-aware image and native video views.

use octara::prelude::*;
use std::ops::Range;
use std::sync::Arc;

pub(crate) fn block_image(url: &str, alt: &str) -> El {
    if url.is_empty() {
        return text(alt.to_owned()).label(alt.to_owned()).role(Role::Image).text_color(ColorToken::TextMuted);
    }
    image_source(ImageSource::Uri(Arc::from(url))).object_fit(ObjectFit::Contain).max_w(rel(1.0)).role(Role::Image).label(alt.to_owned())
}

pub(crate) fn block_video(src: &str, poster: Option<&str>) -> El {
    let mut source = VideoSource::new(src);
    source.poster = poster.map(str::to_owned);
    video(source).w_full().aspect_ratio(16.0 / 9.0)
}

pub(crate) fn inline_image(range: Range<usize>, url: &str) -> Option<InlineAttachment> {
    if url.is_empty() {
        return None;
    }
    Some(InlineAttachment {
        range,
        size: Size::new(24.0, 20.0),
        align: InlineAlign::Middle,
        baseline: None,
        content: InlineContent::ImageSource { source: ImageSource::Uri(Arc::from(url)), fit: ObjectFit::Contain, radius: 0.0 },
    })
}

/// Recognizes a single empty video element with a source and optional poster.
/// Attributes, child elements, and URL schemes outside the allowlist stay inert.
pub(crate) fn parse_video(html: &str) -> Option<(String, Option<String>)> {
    let mut rest = html.trim().strip_prefix('<')?;
    let name_end = rest.find(|c: char| c.is_ascii_whitespace() || c == '>')?;
    if !rest[..name_end].eq_ignore_ascii_case("video") {
        return None;
    }
    rest = &rest[name_end..];
    let mut source = None;
    let mut poster = None;
    let mut controls = false;
    loop {
        if let Some(body) = rest.strip_prefix('>') {
            let closing = body.trim().strip_prefix("</")?.strip_suffix('>')?;
            if !closing.trim_end_matches(|c: char| c.is_ascii_whitespace()).eq_ignore_ascii_case("video") {
                return None;
            }
            return Some((source?, poster));
        }
        let attr = rest.trim_start_matches(|c: char| c.is_ascii_whitespace());
        if attr.len() == rest.len() {
            return None;
        }
        if attr.starts_with('>') {
            rest = attr;
            continue;
        }
        let name_end = attr.find(|c: char| !c.is_ascii_alphabetic())?;
        let (name, tail) = attr.split_at(name_end);
        let value = tail.trim_start_matches(|c: char| c.is_ascii_whitespace()).strip_prefix('=');
        if name.eq_ignore_ascii_case("controls") && !controls {
            controls = true;
            rest = tail;
            if let Some(value) = value {
                let (value, tail) = quoted_value(value)?;
                if !value.is_empty() && !value.eq_ignore_ascii_case("controls") {
                    return None;
                }
                rest = tail;
            }
            continue;
        }
        let (value, tail) = quoted_value(value?)?;
        if name.eq_ignore_ascii_case("src") && source.is_none() {
            source = Some(video_url(value)?);
        } else if name.eq_ignore_ascii_case("poster") && poster.is_none() {
            poster = Some(video_url(value)?);
        } else {
            return None;
        }
        rest = tail;
    }
}

fn quoted_value(value: &str) -> Option<(&str, &str)> {
    let value = value.trim_start_matches(|c: char| c.is_ascii_whitespace());
    let quote = *value.as_bytes().first()?;
    if !matches!(quote, b'\'' | b'"') {
        return None;
    }
    let (value, rest) = value[1..].split_once(char::from(quote))?;
    Some((value, rest))
}

fn video_url(url: &str) -> Option<String> {
    if url.is_empty()
        || url.starts_with(['#', '?'])
        || url.starts_with("//")
        || url.contains(|c: char| c.is_whitespace() || c.is_control() || matches!(c, '\\' | '<' | '>' | '"' | '\''))
        || url.contains("&#")
    {
        return None;
    }
    if let Some((scheme, absolute)) = url.split(['/', '?', '#']).next()?.split_once(':') {
        if !scheme.eq_ignore_ascii_case("http") && !scheme.eq_ignore_ascii_case("https") || !absolute.is_empty() {
            return None;
        }
        let authority = url[scheme.len() + 1..].strip_prefix("//")?.split(['/', '?', '#']).next()?;
        if authority.is_empty() {
            return None;
        }
    }
    Some(url.replace("&amp;", "&"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allowlisted_video_accepts_http_and_relative_urls() {
        assert_eq!(
            parse_video(" <VIDEO poster='/art.jpg' controls src = 'https://media.test/clip.mp4?a=1&amp;b=2'>\n</Video >\n"),
            Some(("https://media.test/clip.mp4?a=1&b=2".to_owned(), Some("/art.jpg".to_owned())))
        );
        assert_eq!(parse_video("<video src=\"clips/first.mp4\"></video>"), Some(("clips/first.mp4".to_owned(), None)));
    }

    #[test]
    fn other_html_and_non_web_video_schemes_remain_inert() {
        for html in [
            "<script src='clip.mp4'></script>",
            "<video src='javascript:alert(1)'></video>",
            "<video src='java&#115;cript:alert(1)'></video>",
            "<video src='data:video/mp4,1'></video>",
            "<video src='file:/etc/passwd'></video>",
            "<video src='clip.mp4' poster='file:///poster.png'></video>",
            "<video src='clip.mp4' onclick='alert(1)'></video>",
            "<video src='clip.mp4' autoplay></video>",
            "<video src='clip.mp4' src='other.mp4'></video>",
            "<video src='clip.mp4'controls></video>",
            "<video src='clip.mp4'><source src='extra.mp4'></video>",
            "<video src='clip.mp4'></video><video src='other.mp4'></video>",
            "<video></video>",
        ] {
            assert_eq!(parse_video(html), None, "{html}");
        }
    }
}
