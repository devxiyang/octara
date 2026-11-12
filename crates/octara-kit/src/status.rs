//! Status: what is going on (a spinner), small labels (chips), messages that need attention
//! (callouts).

use crate::icon_button;
use octara::prelude::*;
use octara_icons as icons;
use std::time::Duration;

/// How often the spinner's arc steps round (twelve steps a second turn, as AppKit's).
const STEP: Duration = Duration::from_millis(83);

/// An activity indicator: an arc stepping round its track. Every spinner on screen keeps one
/// phase (from the app's clock). Sized like an icon (`size_4`); its name for assistive
/// technology is "Loading" (`label` to change it).
#[component]
#[track_caller]
pub fn spinner() -> El {
    let at_now = || (octara::task::now().as_millis() / STEP.as_millis()) as u32 % 12;
    let phase = state("phase", at_now);
    interval(STEP, move || phase.set(at_now()));
    canvas(move |c| {
        let (w, h) = c.size();
        let (cx, cy) = (w / 2.0, h / 2.0);
        let r = (w.min(h) / 2.0 - 1.5).max(1.0);
        let stroke = Stroke::new(2.0).cap(LineCap::Round);
        let at = |deg: f32| {
            let (s, co) = deg.to_radians().sin_cos();
            point(cx + r * co, cy + r * s)
        };
        let track = Path::new().move_to(at(0.0)).arc_to((r, r), 0.0, true, true, at(180.0)).arc_to((r, r), 0.0, true, true, at(360.0));
        c.stroke(&track, &stroke, Token::Border);
        let from = phase.get() as f32 * 30.0 - 90.0;
        let arc = Path::new().move_to(at(from)).arc_to((r, r), 0.0, false, true, at(from + 90.0));
        c.stroke(&arc, &stroke, Token::TextMuted);
    })
    .size_4()
    .flex_shrink_0()
    .role(Role::ProgressIndicator)
    .label("Loading")
}

/// A small label (an attachment, a filter, a tag).
#[component]
#[track_caller]
pub fn chip(label: impl IntoText) -> El {
    row()
        .items_center()
        .gap_1()
        .h(24.0)
        .px_2()
        .rounded_md()
        .border()
        .border_color(Token::Border)
        .bg(Token::Surface)
        .text_xs()
        .text_color(Token::Text)
        .child(text(label).truncate())
}

/// A chip with a button removing it (an attachment before it is sent): `remove` runs on its
/// press. The button is named "Remove {label}".
#[component]
#[track_caller]
pub fn removable_chip(label: &str, remove: impl Fn() + 'static) -> El {
    chip(label.to_owned()).pr_0p5().child(icon_button(icons::X, format!("Remove {label}")).size(20.0).on_press(remove))
}

/// A message that needs attention, with its level's icon and color, and a detail below
/// (`prompt`'s shape, in the page): an error with a retry, a warning. Actions chained as
/// children go after the text. Assistive technology hears it when it appears; a warning or a
/// critical one is an alert.
#[component]
#[track_caller]
pub fn callout(level: PromptLevel, message: impl IntoText, detail: Option<&str>) -> El {
    let (mark, color, role) = match level {
        PromptLevel::Info => (icons::INFO, Token::Accent, Role::Group),
        PromptLevel::Warning => (icons::TRIANGLE_EXCLAMATION_POINT, Token::Warning, Role::Alert),
        PromptLevel::Critical => (icons::CIRCLE_EXCLAMATION_POINT, Token::Danger, Role::Alert),
    };
    row()
        .role(role)
        .live(Live::Polite)
        .items_start()
        .gap_3()
        .p_3()
        .rounded_lg()
        .border()
        .border_color(Token::Border)
        .bg(Token::Surface)
        .child(icon(mark).text_color(color).mt_0p5().flex_shrink_0().a11y_hidden())
        .child(
            col()
                .flex_1()
                .gap_1()
                .child(text(message).text_sm().font_medium().text_color(Token::Text))
                .children(detail.map(|d| text(d.to_owned()).text_sm().text_color(Token::TextMuted))),
        )
}
