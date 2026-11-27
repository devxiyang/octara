use std::hash::BuildHasher;

use octara_model::style::{State, StyleProp, When};
use octara_model::theme::{Breakpoint, ColorToken, ColorValue, Fill, ResponsiveBreakpoint, Theme};

use crate::tables::AuthoredStyle;

// Dedicated ranges keep unrelated events (especially hover and the default text and
// border colors) from colliding. Custom colors and group names use hashed bits.
pub(crate) const SCHEME: u64 = 1 << 12;
pub(crate) const TOUCH: u64 = 1 << 13;
const METRICS: u64 = 1 << 24;
pub(crate) const PATCH: u64 = 1 << 25;
pub(crate) const PROJECTION: u64 = 1 << 26;
const BORDER_INDEX: usize = 9;
const _: () = assert!(matches!(ColorToken::BUILTIN[BORDER_INDEX], ColorToken::Border));
pub(crate) const DEFAULT: u64 = METRICS | (1 << (27 + BORDER_INDEX));

pub(crate) fn state(state: State) -> u64 {
    u64::from(state.bits() & State::all().bits())
}

fn screen(breakpoint: Breakpoint) -> u64 {
    1 << (14 + breakpoint as u8)
}

fn responsive(breakpoint: ResponsiveBreakpoint) -> u64 {
    1 << (19 + breakpoint as u8)
}

pub(crate) fn group(name: &str) -> u64 {
    1 << (56 + (rustc_hash::FxBuildHasher.hash_one(name) & 7))
}

pub(crate) fn token(token: ColorToken) -> u64 {
    if let ColorToken::Custom(name) = token {
        return 1 << (48 + (rustc_hash::FxBuildHasher.hash_one(name) & 7));
    }
    1 << (27 + ColorToken::BUILTIN.iter().position(|&builtin| builtin == token).expect("built-in color"))
}

pub(crate) fn color(value: ColorValue) -> u64 {
    match value {
        ColorValue::Token(base) | ColorValue::Alpha { base, .. } => token(base),
        ColorValue::Mix { a, b, .. } | ColorValue::MixAlpha { a, b, .. } => token(a) | token(b),
        ColorValue::Color(_) => 0,
    }
}

fn props(props: &[StyleProp]) -> u64 {
    props.iter().fold(0, |mask, prop| {
        mask | match *prop {
            StyleProp::TextColor(value)
            | StyleProp::BorderColor(value)
            | StyleProp::RingColor(value)
            | StyleProp::Background(Some(Fill::Solid(value))) => color(value),
            StyleProp::Background(Some(Fill::Linear(gradient))) => color(gradient.from.color) | color(gradient.to.color),
            StyleProp::OverflowX(overflow) | StyleProp::OverflowY(overflow) if overflow.scrolls() => token(ColorToken::TextSubtle),
            _ => 0,
        }
    })
}

pub(crate) fn authored(style: &AuthoredStyle) -> u64 {
    style.overrides.iter().fold(props(&style.base), |mask, (when, declarations)| {
        let condition = match *when {
            When::Light | When::Dark => SCHEME,
            When::GroupHover(name) => group(name),
            When::State(flags) => state(flags),
            When::Screen(breakpoint) => screen(breakpoint),
            When::Responsive(breakpoint) => responsive(breakpoint),
            When::Touch => TOUCH,
        };
        mask | condition | props(declarations)
    })
}

pub(crate) fn viewport(
    old: (Breakpoint, Option<ResponsiveBreakpoint>, bool),
    new: (Breakpoint, Option<ResponsiveBreakpoint>, bool),
) -> u64 {
    let mut changed = if old.2 != new.2 { TOUCH } else { 0 };
    for breakpoint in Breakpoint::ALL {
        if (breakpoint <= old.0) != (breakpoint <= new.0) {
            changed |= screen(breakpoint);
        }
    }
    for breakpoint in ResponsiveBreakpoint::ALL {
        if (Some(breakpoint) <= old.1) != (Some(breakpoint) <= new.1) {
            changed |= responsive(breakpoint);
        }
    }
    changed
}

pub(crate) fn theme(old: &Theme, new: &Theme) -> u64 {
    if old == new {
        return 0;
    }
    let mut changed = PATCH | PROJECTION;
    if old.scheme != new.scheme {
        changed |= SCHEME;
    }
    if !old.same_metrics(new) {
        changed |= METRICS;
    }
    for token_key in ColorToken::BUILTIN.into_iter().chain(old.custom_color_names().chain(new.custom_color_names()).map(ColorToken::Custom))
    {
        if old.try_get(token_key) != new.try_get(token_key) {
            changed |= token(token_key);
        }
    }
    changed
}
