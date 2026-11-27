//! Status: what is going on (a spinner), small labels (chips), messages that need attention
//! (callouts).

use crate::{KitLocale, Tone, icon_button};
use octara::prelude::*;
use octara_icons as icons;
use std::time::Duration;

/// How often the spinner's arc steps round (twelve steps a second turn, as AppKit's).
const STEP: Duration = Duration::from_millis(83);

/// An activity indicator: an arc stepping round its track. Every spinner on screen keeps one
/// phase (from the app's clock). Sized like an icon (`size(sp(4.0))`); its name for assistive
/// technology comes from [`KitLocale::loading`] (`label` to change it).
#[ui]
#[track_caller]
pub fn spinner(build: &mut BuildCx<'_>) -> El {
    let phase = build.signal(0);
    build.spawn("spinner", move |task| async move {
        loop {
            let next = (task.now().as_millis() / STEP.as_millis()) % 12;
            if task.write(|w| w.set_if_changed(phase, next)).is_none() {
                break;
            }
            task.sleep(STEP).await;
        }
    });
    canvas(move |read, c| {
        let phase = read.get(phase);
        let Size { width: w, height: h } = c.size();
        let (cx, cy) = (w / 2.0, h / 2.0);
        let r = (w.min(h) / 2.0 - 1.5).max(1.0);
        let stroke = Stroke::new(2.0).cap(LineCap::Round);
        let at = |deg: f32| {
            let (s, co) = deg.to_radians().sin_cos();
            point(cx + r * co, cy + r * s)
        };
        let track = Path::new().move_to(at(0.0)).arc_to((r, r), 0.0, true, true, at(180.0)).arc_to((r, r), 0.0, true, true, at(360.0));
        c.stroke(&track, &stroke, ColorToken::Border);
        let from = phase as f32 * 30.0 - 90.0;
        let arc = Path::new().move_to(at(from)).arc_to((r, r), 0.0, false, true, at(from + 90.0));
        c.stroke(&arc, &stroke, ColorToken::TextMuted);
    })
    .size(sp(4.0))
    .shrink(0.0)
    .role(Role::ProgressIndicator)
    .label(KitLocale::get(build).loading)
}

/// A small label (an attachment, a filter, a tag).
#[ui]
#[track_caller]
pub fn chip(label: impl IntoText) -> El {
    row()
        .align(AlignItems::Center)
        .gap(sp(1.0))
        .min_h(24.0)
        .px(sp(2.0))
        .rounded(RadiusToken::Md)
        .border(1.0)
        .border_color(ColorToken::Border)
        .bg(ColorToken::Surface)
        .text_size(FontSizeToken::Xs)
        .text_color(ColorToken::Text)
        .child(text(label).text_overflow(TextOverflow::Ellipsis))
}

/// A chip with a localized remove button. Use `configure_remove` to set the button's
/// ID and press handler. The returned element is the chip container.
#[ui]
#[track_caller]
pub fn removable_chip(build: &mut BuildCx<'_>, label: &str, configure_remove: impl FnOnce(El) -> El) -> El {
    let remove_label = KitLocale::get(build).remove_item.replace("{item}", label);
    let remove = configure_remove(icon_button(icons::X, remove_label).size(20.0));
    chip(label).pr(sp(0.5)).child(remove)
}

fn tone_color(tone: Tone) -> ColorToken {
    match tone {
        Tone::Neutral => ColorToken::TextMuted,
        Tone::Accent => ColorToken::Accent,
        Tone::Success => ColorToken::Success,
        Tone::Warning => ColorToken::Warning,
        Tone::Danger => ColorToken::Danger,
    }
}

/// A compact, noninteractive status label.
#[ui]
#[track_caller]
pub fn badge(label: impl IntoText, tone: Tone) -> El {
    let color = tone_color(tone);
    text(label)
        .px(sp(2.0))
        .py(sp(0.5))
        .items_center()
        .justify_center()
        .text_align(TextAlign::Center)
        .rounded(Radius::Full)
        .border(1.0)
        .border_color(color.alpha(0.25))
        .bg(color.alpha(0.10))
        .text_size(FontSizeToken::Xs)
        .text_weight(600)
        .text_color(color)
}

/// A circular profile image. When no picture is available, the first and last initials
/// of the person's name appear in its place.
#[ui]
#[track_caller]
pub fn avatar(name: impl Into<String>, picture: Option<&Image>) -> El {
    let name = name.into();
    let visual = match picture {
        Some(picture) => image(picture).size(rel(1.0)).object_fit(ObjectFit::Cover),
        None => {
            let mut words = name.split_whitespace();
            if let Some(first) = words.next().and_then(|word| word.chars().next()) {
                let last = words.next_back().and_then(|word| word.chars().next());
                let initials: String = std::iter::once(first).chain(last).flat_map(char::to_uppercase).collect();
                text(initials).text_size(FontSizeToken::Xs).text_weight(600)
            } else {
                icon(icons::USER.into()).size(sp(4.0))
            }
        }
    };
    el().role(Role::Image)
        .label(name)
        .size(sp(8.0))
        .shrink(0.0)
        .overflow_x(Overflow::Hidden)
        .overflow_y(Overflow::Hidden)
        .rounded(Radius::Full)
        .bg(ColorToken::SurfacePressed)
        .text_color(ColorToken::Text)
        .align(AlignItems::Center)
        .justify(JustifyContent::Center)
        .child(visual.a11y_hidden())
}

/// A decorative loading placeholder. Chain width, height and radius to match the content.
#[ui]
#[track_caller]
pub fn skeleton() -> El {
    el().w(rel(1.0)).h(sp(4.0)).rounded(RadiusToken::Sm).bg(ColorToken::SurfaceHover).a11y_hidden()
}

/// An inline announcement with a semantic icon, message and optional supporting text.
/// Append application actions as children.
#[ui]
#[track_caller]
pub fn callout(tone: Tone, message: impl IntoText, detail: Option<&str>) -> El {
    let (mark, role) = match tone {
        Tone::Neutral | Tone::Accent => (icons::INFO, Role::Status),
        Tone::Success => (icons::CIRCLE_CHECK, Role::Status),
        Tone::Warning => (icons::TRIANGLE_EXCLAMATION_POINT, Role::Alert),
        Tone::Danger => (icons::CIRCLE_EXCLAMATION_POINT, Role::Alert),
    };
    let color = tone_color(tone);
    row()
        .role(role)
        .live(Live::Polite)
        .align(AlignItems::Start)
        .gap(sp(3.0))
        .p(sp(3.0))
        .rounded(RadiusToken::Lg)
        .border(1.0)
        .border_color(color.alpha(0.25))
        .bg(color.alpha(0.10))
        .child(icon(mark.into()).text_color(color).mt(sp(0.5)).shrink(0.0).a11y_hidden())
        .child(
            col()
                .grow(1.0)
                .basis(0.0)
                .gap(sp(1.0))
                .child(text(message).text_size(FontSizeToken::Sm).text_weight(500).text_color(ColorToken::Text))
                .children(detail.map(|d| text(d.to_owned()).text_size(FontSizeToken::Sm).text_color(ColorToken::TextMuted))),
        )
}
