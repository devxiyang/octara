//! Named values for colors, spacing, radii, shadows, text sizes, container widths and font
//! families. Lengths are in logical pixels.

use serde::{Deserialize, Serialize};
use smallvec::SmallVec;

pub mod palette;

/// A drop shadow (GPUI's `BoxShadow`). As in CSS (and Rune), `blur_radius` is twice the
/// Gaussian's standard deviation. Positive spread grows the shape, while negative spread shrinks it.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct BoxShadow {
    pub color: Color,
    pub offset: (f32, f32),
    pub blur_radius: f32,
    pub spread_radius: f32,
}

impl BoxShadow {
    /// A sharp shadow offset by `(x, y)`.
    pub fn new(x: f32, y: f32, color: Color) -> Self {
        BoxShadow { color, offset: (x, y), blur_radius: 0.0, spread_radius: 0.0 }
    }
    pub fn blur_radius(self, r: f32) -> Self {
        BoxShadow { blur_radius: r.max(0.0), ..self }
    }
    pub fn spread_radius(self, r: f32) -> Self {
        BoxShadow { spread_radius: r, ..self }
    }
}

pub type Color = color::AlphaColor<color::Srgb>;

/// `#rrggbb` as a color.
pub const fn rgb(hex: u32) -> Color {
    Color::from_rgb8((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}

/// Shown for a [`ColorToken::Custom`] the theme does not define (and reported by the kernel).
pub const MISSING: Color = rgb(0xff00ff);

/// A named color resolved against a [`Theme`]. Apps add tokens with [`ColorToken::custom`].
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum ColorToken {
    Background,
    Surface,
    SurfaceElevated,
    SurfaceHover,
    SurfacePressed,
    SurfaceSelected,
    Text,
    TextMuted,
    TextSubtle,
    Border,
    BorderSubtle,
    BorderStrong,
    Accent,
    AccentHover,
    AccentText,
    Focus,
    Selection,
    Info,
    /// A positive status, readable as text against background and surface.
    Success,
    /// Something to be careful about (a warning's icon and text).
    Warning,
    Danger,
    /// An app-defined token: `const BRAND: ColorToken = ColorToken::custom("brand");`
    Custom(&'static str),
}

impl ColorToken {
    pub const BUILTIN: [ColorToken; 21] = [
        ColorToken::Background,
        ColorToken::Surface,
        ColorToken::SurfaceElevated,
        ColorToken::SurfaceHover,
        ColorToken::SurfacePressed,
        ColorToken::SurfaceSelected,
        ColorToken::Text,
        ColorToken::TextMuted,
        ColorToken::TextSubtle,
        ColorToken::Border,
        ColorToken::BorderSubtle,
        ColorToken::BorderStrong,
        ColorToken::Accent,
        ColorToken::AccentHover,
        ColorToken::AccentText,
        ColorToken::Focus,
        ColorToken::Selection,
        ColorToken::Info,
        ColorToken::Success,
        ColorToken::Warning,
        ColorToken::Danger,
    ];

    pub const fn custom(name: &'static str) -> ColorToken {
        ColorToken::Custom(name)
    }

    /// Sets the alpha of this token when resolved.
    pub const fn alpha(self, alpha: f32) -> ColorValue {
        ColorValue::Alpha { base: self, alpha }
    }

    /// Mixes in `other` at the given weight (0–1) when both tokens are resolved.
    pub const fn mix(self, other: ColorToken, weight: f32) -> ColorValue {
        ColorValue::Mix { a: self, b: other, weight }
    }

    fn index(self) -> Option<usize> {
        ColorToken::BUILTIN.iter().position(|t| *t == self)
    }
}

/// The appearance of a theme, used by light and dark style conditions.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default, Serialize, Deserialize)]
pub enum ColorScheme {
    #[default]
    Light,
    Dark,
}

/// Corner radius steps (Tailwind v4 `rounded-xs` … `rounded-4xl`).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum RadiusToken {
    None,
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
pub enum ShadowToken {
    None,
    Xs2,
    Xs,
    Sm,
    Md,
    Lg,
    Xl,
    Xl2,
}

/// Tailwind v4's shadows (black at 5–25 %), as GPUI's `shadow_*` use them.
fn tailwind_shadows() -> [Vec<BoxShadow>; 8] {
    let black = |a: f32| Color::new([0.0, 0.0, 0.0, a]);
    let s = |y: f32, blur: f32, spread: f32, a: f32| BoxShadow::new(0.0, y, black(a)).blur_radius(blur).spread_radius(spread);
    [
        Vec::new(),
        vec![s(1.0, 0.0, 0.0, 0.05)],
        vec![s(1.0, 2.0, 0.0, 0.05)],
        vec![s(1.0, 3.0, 0.0, 0.1), s(1.0, 2.0, -1.0, 0.1)],
        vec![s(4.0, 6.0, -1.0, 0.1), s(2.0, 4.0, -2.0, 0.1)],
        vec![s(10.0, 15.0, -3.0, 0.1), s(4.0, 6.0, -4.0, 0.1)],
        vec![s(20.0, 25.0, -5.0, 0.1), s(8.0, 10.0, -6.0, 0.1)],
        vec![s(25.0, 50.0, -12.0, 0.25)],
    ]
}

/// Paired text size and line height steps (Tailwind `text-xs` … `text-9xl`).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum FontSizeToken {
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

/// Font size and default line height at a typography step, in logical pixels.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct TextScale {
    pub size: f32,
    pub line_height: f32,
}

impl TextScale {
    pub const fn new(size: f32, line_height: f32) -> Self {
        Self { size, line_height }
    }
}

/// Named Tailwind font weights.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
#[repr(u16)]
pub enum FontWeightToken {
    Thin = 100,
    ExtraLight = 200,
    Light = 300,
    Normal = 400,
    Medium = 500,
    Semibold = 600,
    Bold = 700,
    ExtraBold = 800,
    Black = 900,
}

impl FontWeightToken {
    pub const fn value(self) -> u16 {
        self as u16
    }
}

impl From<FontWeightToken> for u16 {
    fn from(token: FontWeightToken) -> Self {
        token.value()
    }
}

/// Named Tailwind line height multipliers.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum LeadingToken {
    None,
    Tight,
    Snug,
    Normal,
    Relaxed,
    Loose,
}

impl LeadingToken {
    pub const fn ratio(self) -> f32 {
        match self {
            Self::None => 1.0,
            Self::Tight => 1.25,
            Self::Snug => 1.375,
            Self::Normal => 1.5,
            Self::Relaxed => 1.625,
            Self::Loose => 2.0,
        }
    }
}

/// Container widths (Tailwind v4 `max-w-3xs` … `max-w-7xl`).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum ContainerWidthToken {
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

/// The minimum viewport widths used by Tailwind v4 responsive utilities.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum ResponsiveBreakpoint {
    Sm,
    Md,
    Lg,
    Xl,
    Xl2,
}

impl ResponsiveBreakpoint {
    pub const ALL: [Self; 5] = [Self::Sm, Self::Md, Self::Lg, Self::Xl, Self::Xl2];

    pub const fn min_width(self) -> f32 {
        match self {
            Self::Sm => 640.0,
            Self::Md => 768.0,
            Self::Lg => 1024.0,
            Self::Xl => 1280.0,
            Self::Xl2 => 1536.0,
        }
    }

    pub fn at_width(width: f32) -> Option<Self> {
        Self::ALL.into_iter().rev().find(|breakpoint| width >= breakpoint.min_width())
    }
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
    pub scheme: ColorScheme,
    colors: [Color; ColorToken::BUILTIN.len()],
    custom: Vec<(&'static str, Color)>,
    /// Pixels per spacing step. `p(sp(4.0))` uses four steps (4 px per step by default).
    pub spacing: f32,
    radius: [f32; 9],
    shadows: [Vec<BoxShadow>; 8],
    text: [TextScale; 13],
    /// Default line height multiplier for numeric font sizes (1.25).
    pub line_height: f32,
    containers: [f32; 13],
    breakpoints: [f32; 4],
    pub font_sans: &'static str,
    pub font_mono: &'static str,
    /// Pixels per `rem` (16), for GPUI-style `rems(..)`.
    pub rem: f32,
}

impl Theme {
    fn base(name: &'static str, scheme: ColorScheme, colors: [Color; ColorToken::BUILTIN.len()]) -> Theme {
        Theme {
            name,
            scheme,
            colors,
            custom: Vec::new(),
            spacing: 4.0,
            radius: [0.0, 2.0, 4.0, 6.0, 8.0, 12.0, 16.0, 24.0, 32.0],
            shadows: tailwind_shadows(),
            text: [
                TextScale::new(12.0, 16.0),
                TextScale::new(14.0, 20.0),
                TextScale::new(16.0, 24.0),
                TextScale::new(18.0, 28.0),
                TextScale::new(20.0, 28.0),
                TextScale::new(24.0, 32.0),
                TextScale::new(30.0, 36.0),
                TextScale::new(36.0, 40.0),
                TextScale::new(48.0, 48.0),
                TextScale::new(60.0, 60.0),
                TextScale::new(72.0, 72.0),
                TextScale::new(96.0, 96.0),
                TextScale::new(128.0, 128.0),
            ],
            line_height: 1.25,
            containers: [256.0, 288.0, 320.0, 384.0, 448.0, 512.0, 576.0, 672.0, 768.0, 896.0, 1024.0, 1152.0, 1280.0],
            breakpoints: [600.0, 840.0, 1200.0, 1600.0],
            font_sans: "system-ui",
            font_mono: "ui-monospace",
            rem: 16.0,
        }
    }

    pub fn light() -> Theme {
        use palette::{AMBER, BLUE, EMERALD, ROSE, WHITE, ZINC};
        Theme::base(
            "light",
            ColorScheme::Light,
            [
                WHITE,
                ZINC.s100,
                ZINC.s50,
                ZINC.s200,
                ZINC.s300,
                BLUE.s100,
                ZINC.s900,
                ZINC.s700,
                ZINC.s600,
                ZINC.s500,
                ZINC.s200,
                ZINC.s600,
                BLUE.s600,
                BLUE.s700,
                WHITE,
                BLUE.s600,
                BLUE.s200,
                BLUE.s700,
                EMERALD.s800,
                AMBER.s800,
                ROSE.s800,
            ],
        )
    }

    pub fn dark() -> Theme {
        use palette::{AMBER, BLUE, CYAN, EMERALD, ROSE, SLATE, ZINC};
        Theme::base(
            "dark",
            ColorScheme::Dark,
            [
                ZINC.s950,
                ZINC.s900,
                ZINC.s800,
                SLATE.s800,
                ZINC.s700,
                BLUE.s950,
                ZINC.s50,
                ZINC.s300,
                SLATE.s300,
                SLATE.s400,
                ZINC.s700,
                ZINC.s400,
                BLUE.s400,
                BLUE.s300,
                BLUE.s950,
                BLUE.s400,
                BLUE.s800,
                CYAN.s300,
                EMERALD.s300,
                AMBER.s300,
                ROSE.s300,
            ],
        )
    }

    // ---- reading ----

    /// Returns the token's color or [`MISSING`] for undefined tokens.
    pub fn get(&self, t: ColorToken) -> Color {
        self.try_get(t).unwrap_or(MISSING)
    }
    pub fn try_get(&self, t: ColorToken) -> Option<Color> {
        match t {
            ColorToken::Custom(name) => self.custom.iter().find(|(n, _)| *n == name).map(|(_, c)| *c),
            t => t.index().map(|i| self.colors[i]),
        }
    }
    #[doc(hidden)]
    pub fn custom_color_names(&self) -> impl Iterator<Item = &'static str> + '_ {
        self.custom.iter().map(|&(name, _)| name)
    }
    #[doc(hidden)]
    pub fn same_metrics(&self, other: &Self) -> bool {
        self.spacing == other.spacing
            && self.radius == other.radius
            && self.shadows == other.shadows
            && self.text == other.text
            && self.line_height == other.line_height
            && self.containers == other.containers
            && self.breakpoints == other.breakpoints
            && self.font_sans == other.font_sans
            && self.font_mono == other.font_mono
            && self.rem == other.rem
    }
    pub fn radius_of(&self, r: RadiusToken) -> f32 {
        self.radius[r as usize]
    }
    /// The shadows of a step.
    pub fn shadow_of(&self, s: ShadowToken) -> &[BoxShadow] {
        &self.shadows[s as usize]
    }
    /// Font size and line height of a text step.
    pub fn text_scale_of(&self, t: FontSizeToken) -> TextScale {
        self.text[t as usize]
    }
    pub fn container_of(&self, c: ContainerWidthToken) -> f32 {
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
    pub fn color(mut self, t: ColorToken, c: Color) -> Theme {
        self.set_color(t, c);
        self
    }
    fn set_color(&mut self, t: ColorToken, c: Color) {
        match t {
            ColorToken::Custom(name) => match self.custom.iter_mut().find(|(n, _)| *n == name) {
                Some((_, v)) => *v = c,
                None => self.custom.push((name, c)),
            },
            t => self.colors[t.index().expect("built-in token")] = c,
        }
    }
    /// Pixels per spacing step (default 4).
    pub fn spacing(mut self, px: f32) -> Theme {
        self.spacing = px;
        self
    }
    pub fn radius(mut self, r: RadiusToken, px: f32) -> Theme {
        self.radius[r as usize] = px;
        self
    }
    /// Replaces a shadow step (Tailwind v4 `--shadow-*`).
    pub fn shadow(mut self, s: ShadowToken, shadows: Vec<BoxShadow>) -> Theme {
        self.shadows[s as usize] = shadows;
        self
    }
    pub fn text_scale(mut self, t: FontSizeToken, scale: TextScale) -> Theme {
        self.text[t as usize] = scale;
        self
    }
    /// The default line height as a multiple of the font size.
    pub fn line_height(mut self, ratio: f32) -> Theme {
        self.line_height = ratio;
        self
    }
    pub fn container(mut self, c: ContainerWidthToken, px: f32) -> Theme {
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

/// Overrides inherited theme values on an element and its descendants.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ThemePatch {
    /// Selects light or dark styles while keeping inherited palette entries.
    pub scheme: Option<ColorScheme>,
    pub spacing: Option<f32>,
    /// Token references in these values resolve against the base theme before applying overrides.
    pub colors: SmallVec<[(ColorToken, ColorValue); 4]>,
    pub radius: SmallVec<[(RadiusToken, f32); 4]>,
    /// Replaces the inherited theme before applying the other overrides.
    pub base_theme: Option<Box<Theme>>,
}

impl ThemePatch {
    /// Starts a subtree with its own complete theme.
    pub fn scope(theme: Theme) -> Self {
        Self { base_theme: Some(Box::new(theme)), ..Self::default() }
    }

    pub fn scheme(mut self, scheme: ColorScheme) -> Self {
        self.scheme = Some(scheme);
        self
    }

    pub fn spacing(mut self, spacing: f32) -> Self {
        self.spacing = Some(spacing);
        self
    }

    pub fn color(mut self, token: ColorToken, value: impl Into<ColorValue>) -> Self {
        self.colors.push((token, value.into()));
        self
    }

    pub fn radius(mut self, token: RadiusToken, pixels: f32) -> Self {
        self.radius.push((token, pixels));
        self
    }

    /// Resolves this patch and reports custom tokens absent from its base theme.
    pub fn resolve(&self, inherited: &Theme) -> (Theme, SmallVec<[&'static str; 4]>) {
        let base = self.base_theme.as_deref().unwrap_or(inherited);
        let mut theme = base.clone();
        let mut missing = SmallVec::new();
        theme.scheme = self.scheme.unwrap_or(base.scheme);
        theme.spacing = self.spacing.unwrap_or(base.spacing);
        for &(token, value) in &self.colors {
            let (color, name) = value.resolve(base);
            missing.extend(name);
            theme.set_color(token, color);
        }
        for &(token, pixels) in &self.radius {
            theme.radius[token as usize] = pixels;
        }
        (theme, missing)
    }
}

/// A color as written in a style, optionally resolved from theme tokens.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum ColorValue {
    Token(ColorToken),
    Color(Color),
    Alpha { base: ColorToken, alpha: f32 },
    Mix { a: ColorToken, b: ColorToken, weight: f32 },
    MixAlpha { a: ColorToken, b: ColorToken, weight: f32, alpha: f32 },
}

impl From<ColorToken> for ColorValue {
    fn from(t: ColorToken) -> ColorValue {
        ColorValue::Token(t)
    }
}
impl From<Color> for ColorValue {
    fn from(c: Color) -> ColorValue {
        ColorValue::Color(c)
    }
}

/// A color stop of a gradient (GPUI's `LinearColorStop`): a color at a fraction (0–1) of the
/// gradient line.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct ColorStop {
    pub color: ColorValue,
    pub percentage: f32,
}

/// `color` at `percentage` (0–1) of the gradient line (GPUI's `linear_color_stop`).
pub fn linear_color_stop(color: impl Into<ColorValue>, percentage: f32) -> ColorStop {
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
/// linear_color_stop(ColorToken::Accent, 0.0), linear_color_stop(ColorToken::Surface, 1.0)))`.
pub fn linear_gradient(angle: f32, from: ColorStop, to: ColorStop) -> Fill {
    Fill::Linear(LinearGradient { angle, from, to })
}

/// What fills a box: a color or a gradient.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Fill {
    Solid(ColorValue),
    Linear(LinearGradient),
}

impl<P: Into<ColorValue>> From<P> for Fill {
    fn from(p: P) -> Fill {
        Fill::Solid(p.into())
    }
}

impl ColorValue {
    /// Replaces the alpha of a color, including colors resolved lazily from a theme.
    pub const fn with_alpha(self, alpha: f32) -> ColorValue {
        match self {
            Self::Color(color) => Self::Color(color.with_alpha(unit(alpha))),
            Self::Token(base) | Self::Alpha { base, .. } => Self::Alpha { base, alpha },
            Self::Mix { a, b, weight } | Self::MixAlpha { a, b, weight, .. } => Self::MixAlpha { a, b, weight, alpha },
        }
    }

    /// The color, and the custom token name if the theme lacks it.
    pub fn resolve(self, theme: &Theme) -> (Color, Option<&'static str>) {
        let token = |token| {
            theme.try_get(token).ok_or_else(|| match token {
                ColorToken::Custom(name) => name,
                _ => unreachable!("built-in tokens have default colors"),
            })
        };
        let mixed = |a, b, weight| Ok(token(a)?.lerp_rect(token(b)?, unit(weight)));
        let color = match self {
            Self::Color(color) => Ok(color),
            Self::Token(base) => token(base),
            Self::Alpha { base, alpha } => token(base).map(|color| color.with_alpha(unit(alpha))),
            Self::Mix { a, b, weight } => mixed(a, b, weight),
            Self::MixAlpha { a, b, weight, alpha } => mixed(a, b, weight).map(|color| color.with_alpha(unit(alpha))),
        };
        match color {
            Ok(color) => (color, None),
            Err(name) => (MISSING, Some(name)),
        }
    }
}

const fn unit(value: f32) -> f32 {
    if value.is_nan() { 0.0 } else { value.clamp(0.0, 1.0) }
}

/// WCAG relative luminance of an sRGB color.
fn luminance(c: Color) -> f32 {
    let lin = |v: f32| if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) };
    let [r, g, b, _] = c.components;
    0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b)
}

/// `fg` (with its alpha) over the opaque `bg`.
pub fn over(fg: Color, bg: Color) -> Color {
    let a = fg.components[3];
    let mix = |i: usize| fg.components[i] * a + bg.components[i] * (1.0 - a);
    Color::new([mix(0), mix(1), mix(2), 1.0])
}

/// The WCAG contrast ratio of two opaque colors (1 to 21).
pub fn contrast(a: Color, b: Color) -> f32 {
    let (la, lb) = (luminance(a), luminance(b));
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

/// A resolved gradient with two colors at fractional positions on its gradient line.
/// The angle is in degrees. Zero points up and 90 points right. Colors interpolate in premultiplied form.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Gradient {
    pub angle: f32,
    pub from: (Color, f32),
    pub to: (Color, f32),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn custom_tokens_scales_and_breakpoints() {
        const BRAND: ColorToken = ColorToken::custom("brand");
        let t = Theme::light().color(BRAND, rgb(0xff6600)).spacing(3.0).breakpoint(Breakpoint::Expanded, 900.0);
        assert_eq!(t.get(BRAND), rgb(0xff6600));
        assert_eq!(t.get(ColorToken::custom("nope")), MISSING);
        assert_eq!(t.spacing, 3.0);
        assert_eq!(t.breakpoint_of(899.0), Breakpoint::Medium);
        assert_eq!(t.breakpoint_of(900.0), Breakpoint::Expanded);
        assert_eq!(t.breakpoint_of(10.0), Breakpoint::Compact);
        assert_eq!(t.text_scale_of(FontSizeToken::Sm), TextScale::new(14.0, 20.0));
        assert_eq!(t.radius_of(RadiusToken::Sm), 4.0);
    }

    #[test]
    fn semantic_defaults_meet_text_and_control_contrast() {
        use ColorToken::*;
        let surfaces = [Background, Surface, SurfaceElevated, SurfaceHover, SurfacePressed, SurfaceSelected];
        for theme in [Theme::light(), Theme::dark()] {
            let check = |foreground, background, required| {
                let ratio = contrast(theme.get(foreground), theme.get(background));
                assert!(ratio >= required, "{} {foreground:?} on {background:?}: {ratio:.2} < {required:.1}", theme.name);
            };
            for surface in surfaces {
                for foreground in [Text, TextMuted, TextSubtle, Info, Success, Warning, Danger] {
                    check(foreground, surface, 4.5);
                }
                for foreground in [Border, BorderStrong, Focus] {
                    check(foreground, surface, 3.0);
                }
            }
            for accent in [Accent, AccentHover] {
                check(AccentText, accent, 4.5);
            }
            check(Text, Selection, 4.5);
        }
    }

    #[test]
    fn tailwind_typography_and_shape_scales() {
        use FontSizeToken::*;
        let theme = Theme::default();
        let sizes = [Xs, Sm, Base, Lg, Xl, Xl2, Xl3, Xl4, Xl5, Xl6, Xl7, Xl8, Xl9];
        assert_eq!(
            sizes.map(|size| theme.text_scale_of(size)).map(|s| (s.size, s.line_height)),
            [
                (12.0, 16.0),
                (14.0, 20.0),
                (16.0, 24.0),
                (18.0, 28.0),
                (20.0, 28.0),
                (24.0, 32.0),
                (30.0, 36.0),
                (36.0, 40.0),
                (48.0, 48.0),
                (60.0, 60.0),
                (72.0, 72.0),
                (96.0, 96.0),
                (128.0, 128.0),
            ]
        );
        use FontWeightToken::*;
        let weights = [Thin, ExtraLight, Light, Normal, Medium, Semibold, Bold, ExtraBold, Black];
        assert_eq!(weights.map(FontWeightToken::value), [100, 200, 300, 400, 500, 600, 700, 800, 900]);
        let leading =
            [LeadingToken::None, LeadingToken::Tight, LeadingToken::Snug, LeadingToken::Normal, LeadingToken::Relaxed, LeadingToken::Loose];
        assert_eq!(leading.map(LeadingToken::ratio), [1.0, 1.25, 1.375, 1.5, 1.625, 2.0]);
        let radii = [
            RadiusToken::None,
            RadiusToken::Xs,
            RadiusToken::Sm,
            RadiusToken::Md,
            RadiusToken::Lg,
            RadiusToken::Xl,
            RadiusToken::Xl2,
            RadiusToken::Xl3,
            RadiusToken::Xl4,
        ];
        assert_eq!(radii.map(|r| theme.radius_of(r)), [0.0, 2.0, 4.0, 6.0, 8.0, 12.0, 16.0, 24.0, 32.0]);
        let shadows = [
            ShadowToken::None,
            ShadowToken::Xs2,
            ShadowToken::Xs,
            ShadowToken::Sm,
            ShadowToken::Md,
            ShadowToken::Lg,
            ShadowToken::Xl,
            ShadowToken::Xl2,
        ];
        assert_eq!(shadows.map(|s| theme.shadow_of(s).len()), [0, 1, 1, 2, 2, 2, 2, 1]);
        use ContainerWidthToken as C;
        let containers = [C::Xs3, C::Xs2, C::Xs, C::Sm, C::Md, C::Lg, C::Xl, C::Xl2, C::Xl3, C::Xl4, C::Xl5, C::Xl6, C::Xl7];
        assert_eq!(
            containers.map(|c| theme.container_of(c)),
            [256.0, 288.0, 320.0, 384.0, 448.0, 512.0, 576.0, 672.0, 768.0, 896.0, 1024.0, 1152.0, 1280.0]
        );
    }

    #[test]
    fn scoped_colors_resolve_alpha_mixes_and_missing_tokens() {
        use ColorToken::*;
        let base = Theme::light().color(Accent, palette::BLACK).color(Surface, palette::WHITE);
        let custom = ColorToken::custom("chart");
        let omitted = ColorToken::custom("unknown");
        let mix = Accent.mix(Surface, 0.25);
        assert_eq!(mix.resolve(&base), (Color::new([0.25, 0.25, 0.25, 1.0]), None));
        assert_eq!(mix.with_alpha(0.4).resolve(&base), (Color::new([0.25, 0.25, 0.25, 0.4]), None));
        assert_eq!(omitted.alpha(0.1).resolve(&base), (MISSING, Some("unknown")));
        assert_eq!(Surface.mix(omitted, 0.5).resolve(&base), (MISSING, Some("unknown")));

        let (scoped, missing) = ThemePatch::default().scheme(ColorScheme::Dark).color(custom, mix).color(Accent, omitted).resolve(&base);
        assert_eq!(scoped.scheme, ColorScheme::Dark);
        assert_eq!(scoped.get(Surface), palette::WHITE);
        assert_eq!(custom.alpha(0.2).resolve(&scoped), (Color::new([0.25, 0.25, 0.25, 0.2]), None));
        assert_eq!(scoped.get(Accent), MISSING);
        assert_eq!(missing.as_slice(), &["unknown"]);
    }
}
