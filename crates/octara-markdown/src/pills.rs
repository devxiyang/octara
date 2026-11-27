//! Auto-link recognition and static and reactive link pill presentation.

use aho_corasick::{AhoCorasick, AhoCorasickKind, BuildError, MatchKind};
use octara::prelude::{ColorValue, Icon, Span};
use std::{
    ops::Range,
    sync::{Arc, OnceLock},
};

/// A literal URL pattern and its static link presentation.
#[derive(Clone, Debug, PartialEq)]
pub struct LinkVariantRule {
    pub pattern: Arc<str>,
    pub style: Span,
}

impl LinkVariantRule {
    pub fn new(pattern: impl Into<Arc<str>>, style: Span) -> Self {
        Self { pattern: pattern.into(), style }
    }
}

/// URL rules compiled once into a leftmost-longest DFA.
#[derive(Clone, Debug)]
pub struct LinkVariantRules {
    matcher: AhoCorasick,
    rules: Arc<[LinkVariantRule]>,
}

impl LinkVariantRules {
    /// Compiles case-sensitive substring rules. Empty patterns are ignored. For a tie at
    /// the same byte, the longest pattern wins, followed by definition order.
    pub fn new(rules: impl IntoIterator<Item = LinkVariantRule>) -> Result<Self, BuildError> {
        let rules: Arc<[_]> = rules.into_iter().filter(|rule| !rule.pattern.is_empty()).collect();
        let matcher = dfa().build(rules.iter().map(|rule| rule.pattern.as_ref()))?;
        Ok(Self { matcher, rules })
    }

    /// Looks up the static presentation for a link's original URL in a single scan.
    pub fn find(&self, url: &str) -> Option<&LinkVariantRule> {
        let found = self.matcher.find(url)?;
        Some(&self.rules[found.pattern().as_usize()])
    }
}

/// Per-URL presentation that can follow application signals. Missing fields use the
/// document label, no icon, and the static link color respectively. The original URL
/// remains the link destination when an alternate label is displayed.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LinkPillContent {
    pub label: Option<Arc<str>>,
    pub icon: Option<Icon>,
    pub tint: Option<ColorValue>,
}

/// A recognized link and its UTF-8 byte range in the scanned plain text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecognizedLink {
    pub range: Range<usize>,
    pub url: Arc<str>,
}

type Resolve = Arc<dyn Fn(&str) -> Option<String> + Send + Sync>;

/// Recognizes opt-in link candidates from plain text in a single pass. Each resolver receives
/// the entire candidate, including its marker or file location, and can decline it with `None`.
#[derive(Clone, Default)]
pub struct TextRecognizer {
    file: Option<Resolve>,
    issue: Option<Resolve>,
    mention: Option<Resolve>,
}

impl TextRecognizer {
    pub fn new() -> Self {
        Self::default()
    }

    /// Enables path candidates such as `src/main.rs`, `./Makefile#L10-L20`, and
    /// `main.rs:42:8`. Line fragments can specify a single line or a range.
    /// A path without an explicit root or relative prefix needs a separator and an
    /// extension, or a line location. Names support letters, digits, underscores,
    /// hyphens, and periods.
    pub fn file_paths(mut self, resolve: impl Fn(&str) -> Option<String> + Send + Sync + 'static) -> Self {
        self.file = Some(Arc::new(resolve));
        self
    }

    /// Enables issue candidates in the form `#` followed by ASCII digits.
    pub fn issues(mut self, resolve: impl Fn(&str) -> Option<String> + Send + Sync + 'static) -> Self {
        self.issue = Some(Arc::new(resolve));
        self
    }

    /// Enables mention candidates in the form `@name`. Names may contain letters,
    /// digits, underscores, and internal hyphens.
    pub fn mentions(mut self, resolve: impl Fn(&str) -> Option<String> + Send + Sync + 'static) -> Self {
        self.mention = Some(Arc::new(resolve));
        self
    }

    /// Returns nonoverlapping, source-ordered links. Callers can ignore ranges already
    /// occupied by explicit links or literal Markdown spans.
    pub fn recognize(&self, text: &str) -> Vec<RecognizedLink> {
        if self.file.is_none() && self.issue.is_none() && self.mention.is_none() {
            return Vec::new();
        }
        let mut links = Vec::new();
        let mut scanned_to = 0;
        for marker in text_markers().find_iter(text) {
            let start = marker.start();
            if start < scanned_to {
                continue;
            }

            let (range, resolver) = match &text[marker.range()] {
                "://" => {
                    scanned_to = text[marker.end()..]
                        .find(|ch: char| ch.is_whitespace() || matches!(ch, '<' | '>' | '"' | '\'' | '`'))
                        .map_or(text.len(), |end| marker.end() + end);
                    continue;
                }
                "#" if self.file.is_none() || !text[marker.end()..].starts_with('L') || !text[..start].ends_with(path_char) => {
                    (self.issue.as_ref().and_then(|_| marked_range(text, start, issue_char)), &self.issue)
                }
                "@" => (self.mention.as_ref().and_then(|_| marked_range(text, start, word_char)), &self.mention),
                _ if self.file.is_some() => {
                    let token = path_token(text, start);
                    // Each maximal path token is examined once, even if it has many markers
                    // and the path or the application resolver rejects it.
                    scanned_to = token.end;
                    (file_range(text, token), &self.file)
                }
                _ => continue,
            };
            if let (Some(range), Some(resolve)) = (range, resolver) {
                scanned_to = scanned_to.max(range.end);
                if let Some(url) = resolve(&text[range.clone()]) {
                    links.push(RecognizedLink { range, url: url.into() });
                }
            }
        }
        links
    }
}

fn dfa() -> aho_corasick::AhoCorasickBuilder {
    let mut builder = AhoCorasick::builder();
    builder.kind(Some(AhoCorasickKind::DFA)).match_kind(MatchKind::LeftmostLongest);
    builder
}

fn text_markers() -> &'static AhoCorasick {
    static MARKERS: OnceLock<AhoCorasick> = OnceLock::new();
    MARKERS.get_or_init(|| dfa().build(["://", "@", "#", ".", "/", "\\", ":"]).expect("fixed recognizer markers fit a DFA"))
}

fn word_char(ch: char) -> bool {
    ch.is_alphanumeric() || matches!(ch, '_' | '-')
}

fn issue_char(ch: char) -> bool {
    ch.is_ascii_digit()
}

fn token_neighbor(ch: char) -> bool {
    word_char(ch) || matches!(ch, '@' | '#' | '/' | '\\')
}

fn marked_range(text: &str, start: usize, allowed: fn(char) -> bool) -> Option<Range<usize>> {
    if text[..start].ends_with(|ch| token_neighbor(ch) || ch == '.') {
        return None;
    }
    let after_marker = start + 1;
    let end = text[after_marker..].find(|ch| !allowed(ch)).map_or(text.len(), |end| after_marker + end);
    let name = &text[after_marker..end];
    let rest = &text[end..];
    if name.is_empty()
        || name.starts_with('-')
        || name.ends_with('-')
        || rest.starts_with(token_neighbor)
        || rest.strip_prefix('.').is_some_and(|rest| rest.starts_with(word_char))
    {
        return None;
    }
    Some(start..end)
}

fn path_char(ch: char) -> bool {
    word_char(ch) || matches!(ch, '.' | '/' | '\\' | ':' | '~')
}

fn path_token(text: &str, marker: usize) -> Range<usize> {
    let start = text[..marker].char_indices().rev().find(|&(_, ch)| !path_char(ch)).map_or(0, |(index, ch)| index + ch.len_utf8());
    let end = text[marker..].find(|ch| !path_char(ch) && ch != '#').map_or(text.len(), |end| marker + end);
    start..end
}

fn file_range(text: &str, token: Range<usize>) -> Option<Range<usize>> {
    if text[..token.start].ends_with(['@', '#']) || text[token.end..].starts_with(['@', '#']) {
        return None;
    }
    let raw = text[token.clone()].trim_end_matches(['.', ':']);
    let (path_and_location, fragment) = match raw.split_once('#') {
        Some((path, fragment)) if valid_line_fragment(fragment) => (path, true),
        Some(_) => return None,
        None => (raw, false),
    };
    let drive = matches!(path_and_location.as_bytes(), [letter, b':', b'/' | b'\\', ..] if letter.is_ascii_alphabetic());
    let path_and_location = if drive { &path_and_location[2..] } else { path_and_location };
    let mut fields = path_and_location.split(':');
    let path = fields.next()?;
    let line = fields.next();
    let column = fields.next();
    if fields.next().is_some()
        || [line, column].into_iter().flatten().any(|field| !line_digits(field))
        || (fragment && line.is_some())
        || path.contains("//")
    {
        return None;
    }

    let (parent, name) = path.rsplit_once(['/', '\\']).unwrap_or(("", path));
    if name.is_empty() || matches!(name, "." | "..") {
        return None;
    }
    let qualified = matches!(path.split_once(['/', '\\']), Some(("" | "." | ".." | "~", _)));
    let has_extension = name.rsplit_once('.').is_some_and(|(_, extension)| extension.chars().any(char::is_alphabetic));
    (qualified || line.is_some() || fragment || (!parent.is_empty() && has_extension)).then_some(token.start..token.start + raw.len())
}

fn valid_line_fragment(fragment: &str) -> bool {
    let Some(fragment) = fragment.strip_prefix('L') else { return false };
    match fragment.split_once('-') {
        Some((start, end)) => line_digits(start) && line_digits(end.strip_prefix('L').unwrap_or(end)),
        None => line_digits(fragment),
    }
}

fn line_digits(value: &str) -> bool {
    !value.is_empty() && value.chars().all(issue_char)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rules_choose_the_longest_match_at_the_first_start() {
        let rule = |pattern, underline| LinkVariantRule::new(pattern, Span { underline, ..Span::default() });
        let rules = LinkVariantRules::new([
            rule("", true),
            rule("example.com", false),
            rule("example.com/issues/", true),
            rule("example.com/issues/", false),
            rule("/issues/128?source=chat", false),
        ])
        .unwrap();
        let matched = rules.find("https://example.com/issues/128?source=chat").unwrap();
        assert_eq!(&*matched.pattern, "example.com/issues/");
        assert!(matched.style.underline);
        assert!(rules.find("https://another.test").is_none());
        assert!(LinkVariantRules::new([]).unwrap().find("file://test").is_none());
    }

    #[test]
    fn recognizes_in_order_with_utf8_byte_ranges() {
        let text = "🍵 Check (src/résumé.rs:42:8), #128, and @álîçé.";
        let recognizer = TextRecognizer::new()
            .file_paths(|path| Some(format!("file:{path}")))
            .issues(|issue| Some(format!("issue:{}", &issue[1..])))
            .mentions(|mention| Some(format!("user:{}", &mention[1..])));
        let links = recognizer.recognize(text);
        let result: Vec<_> = links.iter().map(|link| (&text[link.range.clone()], link.url.as_ref())).collect();
        assert_eq!(result, [("src/résumé.rs:42:8", "file:src/résumé.rs:42:8"), ("#128", "issue:128"), ("@álîçé", "user:álîçé")]);
        assert_eq!(links[0].range.start, text.find("src/").unwrap());
    }

    #[test]
    fn ignores_embedded_markers_and_resolver_declines() {
        let recognizer = TextRecognizer::new()
            .file_paths(|path| Some(path.to_owned()))
            .issues(|issue| (issue != "#404").then(|| issue.to_owned()))
            .mentions(|mention| Some(mention.to_owned()));
        let text = "admin@example.com abc#12 #3oops @-oops @user.host \
                    https://example.com/?user=@alice#128 and/or path/file.rs:12x #404 (#1) @bob";
        let links = recognizer.recognize(text);
        assert_eq!(links.iter().map(|link| &text[link.range.clone()]).collect::<Vec<_>>(), ["#1", "@bob"]);
        assert!(TextRecognizer::default().recognize(text).is_empty());
    }

    #[test]
    fn paths_cover_roots_and_trailing_punctuation() {
        let recognizer = TextRecognizer::new().file_paths(|path| Some(format!("file:{path}")));
        let text = r"/home/app, ./Makefile. ../../README:40 ~/app/.env C:\src\app.rs:5 main.rs:42";
        let links = recognizer.recognize(text);
        assert_eq!(
            links.iter().map(|link| &text[link.range.clone()]).collect::<Vec<_>>(),
            ["/home/app", "./Makefile", "../../README:40", "~/app/.env", r"C:\src\app.rs:5", "main.rs:42"]
        );
    }

    #[test]
    fn paths_cover_line_fragments_and_ranges() {
        let recognizer = TextRecognizer::new().file_paths(|path| Some(format!("file:{path}"))).issues(|issue| Some(issue.into()));
        let text = "🦀 (./Cargo.toml#L10-L20), src/résumé.rs#L42, main.rs#L5-7. Makefile#L2 and #128";
        let links = recognizer.recognize(text);
        let result: Vec<_> = links.iter().map(|link| (&text[link.range.clone()], link.url.as_ref())).collect();
        assert_eq!(
            result,
            [
                ("./Cargo.toml#L10-L20", "file:./Cargo.toml#L10-L20"),
                ("src/résumé.rs#L42", "file:src/résumé.rs#L42"),
                ("main.rs#L5-7", "file:main.rs#L5-7"),
                ("Makefile#L2", "file:Makefile#L2"),
                ("#128", "#128"),
            ],
        );

        let malformed = "src/lib.rs#L src/lib.rs#L4- src/lib.rs#L4-L src/lib.rs#L4-L5x src/lib.rs#note src/lib.rs:2#L5 https://example.test/src/lib.rs#L2";
        assert!(recognizer.recognize(malformed).is_empty());
    }
}
