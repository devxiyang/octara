//! Scoped, source-preserving highlighting for fenced code.

use octara::prelude::{ColorToken, Rich, Span, rich};

/// A highlighter registered via `BuildCx::provide::<Arc<dyn CodeHighlighter>>`.
pub trait CodeHighlighter: Send + Sync + 'static {
    /// The renderer normalizes code line endings to LF before calling this method.
    /// Return one rich line per `code.split('\n')`, preserving each line verbatim.
    /// Return `None` to use the built-in highlighter.
    fn highlight(&self, language: &str, code: &str) -> Option<Vec<Rich>>;
}

/// Languages supported by the lightweight built-in scanner.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LanguagePack {
    Rust,
    JavaScript,
    Python,
    Json,
    Shell,
}

impl LanguagePack {
    const ALL: &[Self] = &[Self::Rust, Self::JavaScript, Self::Python, Self::Json, Self::Shell];

    fn for_language(name: &str) -> Option<Self> {
        Some(match name.to_ascii_lowercase().as_str() {
            "rs" | "rust" => Self::Rust,
            "js" | "javascript" | "jsx" | "ts" | "typescript" | "tsx" => Self::JavaScript,
            "py" | "python" => Self::Python,
            "json" => Self::Json,
            "sh" | "bash" | "zsh" | "shell" => Self::Shell,
            _ => return None,
        })
    }

    fn line_comment(self, input: &str) -> bool {
        match self {
            Self::Rust | Self::JavaScript => input.starts_with("//"),
            Self::Python | Self::Shell => input.starts_with('#'),
            Self::Json => false,
        }
    }

    fn quote(self, first: char) -> bool {
        first == '"' || first == '\'' && self != Self::Json || first == '`' && matches!(self, Self::JavaScript | Self::Shell)
    }

    fn keyword(self, word: &str) -> bool {
        match self {
            Self::Rust => matches!(
                word,
                "as" | "async"
                    | "await"
                    | "break"
                    | "const"
                    | "continue"
                    | "crate"
                    | "dyn"
                    | "else"
                    | "enum"
                    | "extern"
                    | "false"
                    | "fn"
                    | "for"
                    | "if"
                    | "impl"
                    | "in"
                    | "let"
                    | "loop"
                    | "match"
                    | "mod"
                    | "move"
                    | "mut"
                    | "pub"
                    | "ref"
                    | "return"
                    | "self"
                    | "Self"
                    | "static"
                    | "struct"
                    | "super"
                    | "trait"
                    | "true"
                    | "type"
                    | "unsafe"
                    | "use"
                    | "where"
                    | "while"
                    | "yield"
            ),
            Self::JavaScript => matches!(
                word,
                "as" | "async"
                    | "await"
                    | "break"
                    | "case"
                    | "catch"
                    | "class"
                    | "const"
                    | "continue"
                    | "default"
                    | "delete"
                    | "do"
                    | "else"
                    | "export"
                    | "extends"
                    | "false"
                    | "finally"
                    | "for"
                    | "from"
                    | "function"
                    | "if"
                    | "import"
                    | "in"
                    | "instanceof"
                    | "interface"
                    | "let"
                    | "new"
                    | "null"
                    | "return"
                    | "static"
                    | "super"
                    | "switch"
                    | "this"
                    | "throw"
                    | "true"
                    | "try"
                    | "type"
                    | "typeof"
                    | "var"
                    | "void"
                    | "while"
                    | "yield"
            ),
            Self::Python => matches!(
                word,
                "False"
                    | "None"
                    | "True"
                    | "and"
                    | "as"
                    | "assert"
                    | "async"
                    | "await"
                    | "break"
                    | "class"
                    | "continue"
                    | "def"
                    | "del"
                    | "elif"
                    | "else"
                    | "except"
                    | "finally"
                    | "for"
                    | "from"
                    | "global"
                    | "if"
                    | "import"
                    | "in"
                    | "is"
                    | "lambda"
                    | "nonlocal"
                    | "not"
                    | "or"
                    | "pass"
                    | "raise"
                    | "return"
                    | "try"
                    | "while"
                    | "with"
                    | "yield"
            ),
            Self::Json => matches!(word, "true" | "false" | "null"),
            Self::Shell => matches!(
                word,
                "case"
                    | "do"
                    | "done"
                    | "elif"
                    | "else"
                    | "esac"
                    | "export"
                    | "fi"
                    | "for"
                    | "function"
                    | "if"
                    | "in"
                    | "local"
                    | "return"
                    | "then"
                    | "until"
                    | "while"
            ),
        }
    }
}

/// Built-in highlighting using the selected language packs.
#[derive(Clone, Debug)]
pub struct BuiltinHighlighter {
    packs: Vec<LanguagePack>,
}

impl Default for BuiltinHighlighter {
    fn default() -> Self {
        Self::with_packs(LanguagePack::ALL.iter().copied())
    }
}

impl BuiltinHighlighter {
    /// Limits which built-in languages are highlighted in this scope.
    pub fn with_packs(packs: impl IntoIterator<Item = LanguagePack>) -> Self {
        Self { packs: packs.into_iter().collect() }
    }
}

impl CodeHighlighter for BuiltinHighlighter {
    fn highlight(&self, language: &str, code: &str) -> Option<Vec<Rich>> {
        let pack = LanguagePack::for_language(language)?;
        if self.packs.contains(&pack) { Some(highlight(pack, code)) } else { Some(code.split('\n').map(Rich::from).collect()) }
    }
}

pub(crate) fn highlight_code(scoped: Option<&dyn CodeHighlighter>, lang: &str, code: &str) -> Option<Vec<Rich>> {
    if let Some(lines) = scoped.and_then(|scoped| scoped.highlight(lang, code))
        && lines.iter().map(Rich::as_str).eq(code.split('\n'))
    {
        return Some(lines);
    }
    LanguagePack::for_language(lang).map(|pack| highlight(pack, code))
}

fn highlight(pack: LanguagePack, mut rest: &str) -> Vec<Rich> {
    let mut lines = Vec::new();
    let mut line = rich();
    let mut plain = 0;
    while let Some(first) = rest[plain..].chars().next() {
        if first == '\n' {
            lines.push(std::mem::take(&mut line).text(&rest[..plain]));
            rest = &rest[plain + 1..];
            plain = 0;
        } else {
            let (len, color) = token(pack, &rest[plain..], first);
            let Some(color) = color else {
                plain += len;
                continue;
            };
            line = line.text(&rest[..plain]);
            let span = Span { color: Some(color.into()), ..Default::default() };
            let mut parts = rest[plain..plain + len].split('\n');
            line = line.span(parts.next().unwrap_or_default(), span);
            for part in parts {
                lines.push(line);
                line = rich().span(part, span);
            }
            rest = &rest[plain + len..];
            plain = 0;
        }
    }
    lines.push(line.text(rest));
    lines
}

fn token(pack: LanguagePack, input: &str, first: char) -> (usize, Option<ColorToken>) {
    if pack.line_comment(input) {
        return (input.find('\n').unwrap_or(input.len()), Some(ColorToken::TextMuted));
    }
    if matches!(pack, LanguagePack::Rust | LanguagePack::JavaScript) && input.starts_with("/*") {
        return (block_comment(input, pack == LanguagePack::Rust), Some(ColorToken::TextMuted));
    }
    if pack.quote(first)
        && let Some(len) = string_literal(input, first, pack)
    {
        return (len, Some(ColorToken::Success));
    }
    if first.is_ascii_digit() {
        let len = input.find(|c: char| !c.is_ascii_alphanumeric() && c != '_' && c != '.').unwrap_or(input.len());
        return (len, Some(ColorToken::Warning));
    }
    if first.is_alphabetic() || first == '_' {
        let len = input.find(|c: char| !c.is_alphanumeric() && c != '_').unwrap_or(input.len());
        return (len, pack.keyword(&input[..len]).then_some(ColorToken::Accent));
    }
    (first.len_utf8(), None)
}

fn block_comment(input: &str, nesting: bool) -> usize {
    let mut at = 2;
    let mut depth = 1;
    while let Some(next) = input[at..].find(['/', '*']) {
        at += next;
        if input[at..].starts_with("*/") {
            depth -= 1;
            if depth == 0 {
                return at + 2;
            }
            at += 2;
        } else if nesting && input[at..].starts_with("/*") {
            depth += 1;
            at += 2;
        } else {
            at += 1;
        }
    }
    input.len()
}

fn string_literal(input: &str, quote: char, pack: LanguagePack) -> Option<usize> {
    let triple = pack == LanguagePack::Python && input.as_bytes().starts_with(&[quote as u8; 3]);
    let width = if triple { 3 } else { 1 };
    let delimiter = &input[..width];
    let mut escaped = false;
    for (at, character) in input[width..].char_indices() {
        let at = at + width;
        if escaped {
            escaped = false;
        } else if character == '\\' {
            escaped = true;
        } else if character == quote && input[at..].starts_with(delimiter) {
            return Some(at + width);
        } else if character == '\n' && !triple && quote != '`' {
            return (pack != LanguagePack::Rust || quote != '\'').then_some(at);
        }
    }
    (pack != LanguagePack::Rust || quote != '\'').then_some(input.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Corrupt;

    impl CodeHighlighter for Corrupt {
        fn highlight(&self, _: &str, _: &str) -> Option<Vec<Rich>> {
            Some(vec![Rich::from("rewritten")])
        }
    }

    #[test]
    fn language_pack_highlights_exact_source_lines() {
        let source = "/* outer /* nested */\n  comment */\nlet café = \"🎬\";\r\n\n";
        let lines = highlight_code(None, "rs", source).unwrap();
        assert!(lines.iter().map(Rich::as_str).eq(source.split('\n')));
        assert_ne!(lines[1], Rich::from("  comment */"));
        assert_ne!(lines[2], Rich::from("let café = \"🎬\";\r"));
        assert_eq!(lines[3], Rich::from(""));
    }

    #[test]
    fn rejects_rewritten_scoped_output_and_can_trim_default_packs() {
        let source = "let n = 4;";
        let builtin = highlight_code(None, "rust", source);
        assert_eq!(highlight_code(Some(&Corrupt), "rust", source), builtin);
        let trimmed = BuiltinHighlighter::with_packs([LanguagePack::Json]);
        assert_eq!(highlight_code(Some(&trimmed), "rust", source), Some(vec![Rich::from(source)]));
        assert!(highlight_code(None, "unknown", source).is_none());
    }
}
