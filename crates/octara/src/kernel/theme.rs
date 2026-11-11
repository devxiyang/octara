//! The theme: everything a style can name instead of spelling out, as data: colors by
//! [`Token`], the spacing unit, the radius, text and container scales, window size classes and
//! font families. Scale names follow Tailwind; lengths are logical pixels (points / dp). The kernel resolves styles against it, so switching themes
//! (dark mode, a brand, a denser spacing) reruns no closure.

use super::style::BoxShadow;

pub type Color = color::AlphaColor<color::Srgb>;

/// `#rrggbb` as a color.
pub const fn rgb(hex: u32) -> Color {
    Color::from_rgb8((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}

/// Shown for a [`Token::Custom`] the theme does not define (and reported by the kernel).
pub const MISSING: Color = rgb(0xff00ff);

/// A semantic color slot. Components use tokens; a [`Theme`] gives them values. Apps add their
/// own with [`Token::custom`].
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Token {
    Background,
    Surface,
    SurfaceHover,
    SurfacePressed,
    Text,
    TextMuted,
    Border,
    Accent,
    AccentHover,
    AccentText,
    Focus,
    Danger,
    /// An app-defined token: `const BRAND: Token = Token::custom("brand");`
    Custom(&'static str),
}

impl Token {
    pub const BUILTIN: [Token; 12] = [
        Token::Background,
        Token::Surface,
        Token::SurfaceHover,
        Token::SurfacePressed,
        Token::Text,
        Token::TextMuted,
        Token::Border,
        Token::Accent,
        Token::AccentHover,
        Token::AccentText,
        Token::Focus,
        Token::Danger,
    ];

    pub const fn custom(name: &'static str) -> Token {
        Token::Custom(name)
    }

    fn index(self) -> Option<usize> {
        Token::BUILTIN.iter().position(|t| *t == self)
    }
}

/// Corner radius steps (Tailwind v4 `rounded-xs` … `rounded-4xl`).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum RadiusStep {
    Xs,
    Sm,
    Md,
    Lg,
    Xl,
    Xl2,
    Xl3,
    Xl4,
}

/// Shadow steps (Tailwind `shadow-2xs` … `shadow-2xl`, GPUI's `shadow_*`).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum ShadowStep {
    Xs2,
    Xs,
    Sm,
    Md,
    Lg,
    Xl,
    Xl2,
}

/// Tailwind v4's shadows (black at 5–25 %), as GPUI's `shadow_*` use them.
fn tailwind_shadows() -> [Vec<BoxShadow>; 7] {
    let black = |a: f32| Color::new([0.0, 0.0, 0.0, a]);
    let s = |y: f32, blur: f32, spread: f32, a: f32| BoxShadow::new(0.0, y, black(a)).blur_radius(blur).spread_radius(spread);
    [
        vec![s(1.0, 0.0, 0.0, 0.05)],
        vec![s(1.0, 2.0, 0.0, 0.05)],
        vec![s(1.0, 3.0, 0.0, 0.1), s(1.0, 2.0, -1.0, 0.1)],
        vec![s(4.0, 6.0, -1.0, 0.1), s(2.0, 4.0, -2.0, 0.1)],
        vec![s(10.0, 15.0, -3.0, 0.1), s(4.0, 6.0, -4.0, 0.1)],
        vec![s(20.0, 25.0, -5.0, 0.1), s(8.0, 10.0, -6.0, 0.1)],
        vec![s(25.0, 50.0, -12.0, 0.25)],
    ]
}

/// Text size steps (Tailwind `text-xs` … `text-9xl`). As in GPUI they set the size only; the
/// line height is the theme's `line_height` × size unless set.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum TextStep {
    Xs,
    Sm,
    Base,
    Lg,
    Xl,
    Xl2,
    Xl3,
    Xl4,
    Xl5,
    Xl6,
    Xl7,
    Xl8,
    Xl9,
}

/// Container widths (Tailwind v4 `max-w-3xs` … `max-w-7xl`).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum ContainerStep {
    Xs3,
    Xs2,
    Xs,
    Sm,
    Md,
    Lg,
    Xl,
    Xl2,
    Xl3,
    Xl4,
    Xl5,
    Xl6,
    Xl7,
}

/// A window size class (Material 3), smallest first: styles for a class apply from its
/// minimum width up. Widths come from the theme (dp / logical pixels).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Default)]
pub enum Breakpoint {
    /// Below 600: phones in portrait.
    #[default]
    Compact,
    /// From 600: tablets in portrait, foldables, small windows.
    Medium,
    /// From 840: tablets in landscape, desktop windows.
    Expanded,
    /// From 1200: large desktop windows.
    Large,
    /// From 1600: very wide windows.
    ExtraLarge,
}

impl Breakpoint {
    pub const ALL: [Breakpoint; 5] =
        [Breakpoint::Compact, Breakpoint::Medium, Breakpoint::Expanded, Breakpoint::Large, Breakpoint::ExtraLarge];
}

/// A font family as written in a style.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub enum Family {
    /// The theme's sans-serif family (the default).
    #[default]
    Sans,
    /// The theme's monospace family.
    Mono,
    /// A registered family by name.
    Named(&'static str),
}

/// Values for everything a style can name. Defaults follow Tailwind v4.
#[derive(Clone, Debug, PartialEq)]
pub struct Theme {
    pub name: &'static str,
    colors: [Color; 12],
    custom: Vec<(&'static str, Color)>,
    /// Pixels per spacing step: `p_4` is `4 × spacing` (Tailwind `--spacing`, 4 px).
    pub spacing: f32,
    radius: [f32; 8],
    shadows: [Vec<BoxShadow>; 7],
    text: [f32; 13],
    /// Line height as a multiple of the font size when a style does not set one (1.25).
    pub line_height: f32,
    containers: [f32; 13],
    breakpoints: [f32; 4],
    pub font_sans: &'static str,
    pub font_mono: &'static str,
    /// Pixels per `rem` (16), for GPUI-style `rems(..)`.
    pub rem: f32,
}

impl Theme {
    fn base(name: &'static str, colors: [Color; 12]) -> Theme {
        Theme {
            name,
            colors,
            custom: Vec::new(),
            spacing: 4.0,
            radius: [2.0, 4.0, 6.0, 8.0, 12.0, 16.0, 24.0, 32.0],
            shadows: tailwind_shadows(),
            text: [12.0, 14.0, 16.0, 18.0, 20.0, 24.0, 30.0, 36.0, 48.0, 60.0, 72.0, 96.0, 128.0],
            line_height: 1.25,
            containers: [256.0, 288.0, 320.0, 384.0, 448.0, 512.0, 576.0, 672.0, 768.0, 896.0, 1024.0, 1152.0, 1280.0],
            breakpoints: [600.0, 840.0, 1200.0, 1600.0],
            font_sans: "Inter",
            font_mono: "IBM Plex Mono",
            rem: 16.0,
        }
    }

    pub fn light() -> Theme {
        Theme::base(
            "light",
            [
                rgb(0xffffff),
                rgb(0xf4f4f5),
                rgb(0xe4e4e7),
                rgb(0xd4d4d8),
                rgb(0x18181b),
                // A shade under zinc-500 (#71717a, 4.40:1 on Surface): 4.80:1.
                rgb(0x6b6b74),
                rgb(0xe4e4e7),
                rgb(0x2563eb),
                rgb(0x1d4ed8),
                rgb(0xffffff),
                rgb(0x3b82f6),
                rgb(0xdc2626),
            ],
        )
    }

    pub fn dark() -> Theme {
        Theme::base(
            "dark",
            [
                rgb(0x09090b),
                rgb(0x18181b),
                rgb(0x27272a),
                rgb(0x3f3f46),
                rgb(0xfafafa),
                rgb(0xa1a1aa),
                rgb(0x27272a),
                // Accent fills carry white text: blue-600 / blue-700 keep it above 4.5:1 (blue-500
                // gave 3.68:1). Focus rings stay the bright blue.
                rgb(0x2563eb),
                rgb(0x1d4ed8),
                rgb(0xffffff),
                rgb(0x60a5fa),
                rgb(0xef4444),
            ],
        )
    }

    // ---- reading ----

    /// The color of `t`; [`MISSING`] for a custom token the theme does not define.
    pub fn get(&self, t: Token) -> Color {
        self.try_get(t).unwrap_or(MISSING)
    }
    pub fn try_get(&self, t: Token) -> Option<Color> {
        match t {
            Token::Custom(name) => self.custom.iter().find(|(n, _)| *n == name).map(|(_, c)| *c),
            t => t.index().map(|i| self.colors[i]),
        }
    }
    pub fn radius_of(&self, r: RadiusStep) -> f32 {
        self.radius[r as usize]
    }
    /// The shadows of a step.
    pub fn shadow_of(&self, s: ShadowStep) -> &[BoxShadow] {
        &self.shadows[s as usize]
    }
    /// Font size of a text step.
    pub fn text_of(&self, t: TextStep) -> f32 {
        self.text[t as usize]
    }
    pub fn container_of(&self, c: ContainerStep) -> f32 {
        self.containers[c as usize]
    }
    /// The minimum width of a size class (0 for `Compact`).
    pub fn breakpoint_width(&self, b: Breakpoint) -> f32 {
        match b {
            Breakpoint::Compact => 0.0,
            b => self.breakpoints[b as usize - 1],
        }
    }
    /// The size class of a window `width` wide.
    pub fn breakpoint_of(&self, width: f32) -> Breakpoint {
        Breakpoint::ALL.into_iter().rev().find(|b| width >= self.breakpoint_width(*b)).unwrap_or_default()
    }
    pub fn family(&self, f: Family) -> &'static str {
        match f {
            Family::Sans => self.font_sans,
            Family::Mono => self.font_mono,
            Family::Named(n) => n,
        }
    }

    // ---- building (Tailwind `@theme { --color-*: ..; --spacing: ..; .. }`) ----

    /// Sets a token's color (built-in or custom).
    pub fn color(mut self, t: Token, c: Color) -> Theme {
        match t {
            Token::Custom(name) => match self.custom.iter_mut().find(|(n, _)| *n == name) {
                Some((_, v)) => *v = c,
                None => self.custom.push((name, c)),
            },
            t => self.colors[t.index().expect("built-in token")] = c,
        }
        self
    }
    /// Pixels per spacing step (default 4).
    pub fn spacing(mut self, px: f32) -> Theme {
        self.spacing = px;
        self
    }
    pub fn radius(mut self, r: RadiusStep, px: f32) -> Theme {
        self.radius[r as usize] = px;
        self
    }
    /// Replaces a shadow step (Tailwind v4 `--shadow-*`).
    pub fn shadow(mut self, s: ShadowStep, shadows: Vec<BoxShadow>) -> Theme {
        self.shadows[s as usize] = shadows;
        self
    }
    pub fn text(mut self, t: TextStep, size: f32) -> Theme {
        self.text[t as usize] = size;
        self
    }
    /// The default line height as a multiple of the font size.
    pub fn line_height(mut self, ratio: f32) -> Theme {
        self.line_height = ratio;
        self
    }
    pub fn container(mut self, c: ContainerStep, px: f32) -> Theme {
        self.containers[c as usize] = px;
        self
    }
    /// The minimum width of a size class (`Compact` is always 0).
    pub fn breakpoint(mut self, b: Breakpoint, px: f32) -> Theme {
        assert!(b != Breakpoint::Compact, "Compact starts at 0");
        self.breakpoints[b as usize - 1] = px;
        self
    }
    /// The sans-serif family (register its font data with `App::register_font`).
    pub fn font_sans(mut self, family: &'static str) -> Theme {
        self.font_sans = family;
        self
    }
    pub fn font_mono(mut self, family: &'static str) -> Theme {
        self.font_mono = family;
        self
    }
}

impl Default for Theme {
    fn default() -> Theme {
        Theme::light()
    }
}

/// A color as written in a style: a theme token or an explicit color.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Paint {
    Token(Token),
    Color(Color),
}

impl From<Token> for Paint {
    fn from(t: Token) -> Paint {
        Paint::Token(t)
    }
}
impl From<Color> for Paint {
    fn from(c: Color) -> Paint {
        Paint::Color(c)
    }
}

/// A color stop of a gradient (GPUI's `LinearColorStop`): a color at a fraction (0–1) of the
/// gradient line.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct ColorStop {
    pub color: Paint,
    pub percentage: f32,
}

/// `color` at `percentage` (0–1) of the gradient line (GPUI's `linear_color_stop`).
pub fn linear_color_stop(color: impl Into<Paint>, percentage: f32) -> ColorStop {
    ColorStop { color: color.into(), percentage }
}

/// A linear gradient between two stops. `angle` is in degrees as in CSS: 0 runs up, 90 to the
/// right, 180 down.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct LinearGradient {
    pub angle: f32,
    pub from: ColorStop,
    pub to: ColorStop,
}

/// A linear gradient (GPUI's `linear_gradient`): `.bg(linear_gradient(90.0,
/// linear_color_stop(Token::Accent, 0.0), linear_color_stop(Token::Surface, 1.0)))`.
pub fn linear_gradient(angle: f32, from: ColorStop, to: ColorStop) -> Fill {
    Fill::Linear(LinearGradient { angle, from, to })
}

/// What fills a box: a color or a gradient.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Fill {
    Solid(Paint),
    Linear(LinearGradient),
}

impl<P: Into<Paint>> From<P> for Fill {
    fn from(p: P) -> Fill {
        Fill::Solid(p.into())
    }
}

impl Paint {
    /// The color, and the custom token name if the theme lacks it.
    pub(crate) fn resolve(self, theme: &Theme) -> (Color, Option<&'static str>) {
        match self {
            Paint::Color(c) => (c, None),
            Paint::Token(t) => match theme.try_get(t) {
                Some(c) => (c, None),
                None => (MISSING, if let Token::Custom(n) = t { Some(n) } else { None }),
            },
        }
    }
}

/// WCAG relative luminance of an sRGB color.
fn luminance(c: Color) -> f32 {
    let lin = |v: f32| if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) };
    let [r, g, b, _] = c.components;
    0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b)
}

/// `fg` (with its alpha) over the opaque `bg`.
pub(crate) fn over(fg: Color, bg: Color) -> Color {
    let a = fg.components[3];
    let mix = |i: usize| fg.components[i] * a + bg.components[i] * (1.0 - a);
    Color::new([mix(0), mix(1), mix(2), 1.0])
}

/// The WCAG contrast ratio of two opaque colors (1 to 21).
pub fn contrast(a: Color, b: Color) -> f32 {
    let (la, lb) = (luminance(a), luminance(b));
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn custom_tokens_scales_and_breakpoints() {
        const BRAND: Token = Token::custom("brand");
        let t = Theme::light().color(BRAND, rgb(0xff6600)).spacing(3.0).breakpoint(Breakpoint::Expanded, 900.0);
        assert_eq!(t.get(BRAND), rgb(0xff6600));
        assert_eq!(t.get(Token::custom("nope")), MISSING);
        assert_eq!(t.spacing, 3.0);
        assert_eq!(t.breakpoint_of(899.0), Breakpoint::Medium);
        assert_eq!(t.breakpoint_of(900.0), Breakpoint::Expanded);
        assert_eq!(t.breakpoint_of(10.0), Breakpoint::Compact);
        assert_eq!(t.text_of(TextStep::Sm), 14.0);
        assert_eq!(t.radius_of(RadiusStep::Sm), 4.0);
    }
}
