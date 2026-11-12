//! The built-in themes keep their text readable: every text token on every surface it is used
//! on meets WCAG's 4.5:1 (`octara check` judges apps by the same rule).

use octara::kernel::{Theme, Token, contrast};

#[test]
fn text_tokens_meet_wcag_on_their_surfaces() {
    let pairs = [
        (Token::Text, Token::Background),
        (Token::Text, Token::Surface),
        (Token::Text, Token::SurfaceHover),
        (Token::TextMuted, Token::Background),
        (Token::TextMuted, Token::Surface),
        (Token::AccentText, Token::Accent),
        (Token::AccentText, Token::AccentHover),
        (Token::Danger, Token::Background),
        (Token::Warning, Token::Background),
        (Token::Warning, Token::Surface),
    ];
    let mut failures = Vec::new();
    for theme in [Theme::light(), Theme::dark()] {
        for (fg, bg) in pairs {
            let ratio = contrast(theme.get(fg), theme.get(bg));
            if ratio < 4.5 {
                failures.push(format!("{}: {fg:?} on {bg:?} is {ratio:.2}:1", theme.name));
            }
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}
