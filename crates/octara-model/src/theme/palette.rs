//! Tailwind CSS v4.1 palette ramps, converted from OKLCH and clipped to 8-bit sRGB.
//! <https://github.com/tailwindlabs/tailwindcss/blob/v4.1.0/packages/tailwindcss/theme.css>

use super::{Color, rgb};

/// A step on the Tailwind color ramp.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u16)]
pub enum Shade {
    S50 = 50,
    S100 = 100,
    S200 = 200,
    S300 = 300,
    S400 = 400,
    S500 = 500,
    S600 = 600,
    S700 = 700,
    S800 = 800,
    S900 = 900,
    S950 = 950,
}

impl Shade {
    pub const fn from_step(step: u16) -> Option<Self> {
        match step {
            50 => Some(Self::S50),
            100 => Some(Self::S100),
            200 => Some(Self::S200),
            300 => Some(Self::S300),
            400 => Some(Self::S400),
            500 => Some(Self::S500),
            600 => Some(Self::S600),
            700 => Some(Self::S700),
            800 => Some(Self::S800),
            900 => Some(Self::S900),
            950 => Some(Self::S950),
            _ => None,
        }
    }
}

/// All 11 shades of a color family, from lightest to darkest.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ColorScale {
    pub s50: Color,
    pub s100: Color,
    pub s200: Color,
    pub s300: Color,
    pub s400: Color,
    pub s500: Color,
    pub s600: Color,
    pub s700: Color,
    pub s800: Color,
    pub s900: Color,
    pub s950: Color,
}

impl ColorScale {
    pub const fn shade(&self, shade: Shade) -> Color {
        match shade {
            Shade::S50 => self.s50,
            Shade::S100 => self.s100,
            Shade::S200 => self.s200,
            Shade::S300 => self.s300,
            Shade::S400 => self.s400,
            Shade::S500 => self.s500,
            Shade::S600 => self.s600,
            Shade::S700 => self.s700,
            Shade::S800 => self.s800,
            Shade::S900 => self.s900,
            Shade::S950 => self.s950,
        }
    }

    pub const fn step(&self, step: u16) -> Option<Color> {
        match Shade::from_step(step) {
            Some(shade) => Some(self.shade(shade)),
            None => None,
        }
    }

    pub const fn all(&self) -> [Color; 11] {
        [self.s50, self.s100, self.s200, self.s300, self.s400, self.s500, self.s600, self.s700, self.s800, self.s900, self.s950]
    }

    const fn from_hex(hex: [u32; 11]) -> Self {
        Self {
            s50: rgb(hex[0]),
            s100: rgb(hex[1]),
            s200: rgb(hex[2]),
            s300: rgb(hex[3]),
            s400: rgb(hex[4]),
            s500: rgb(hex[5]),
            s600: rgb(hex[6]),
            s700: rgb(hex[7]),
            s800: rgb(hex[8]),
            s900: rgb(hex[9]),
            s950: rgb(hex[10]),
        }
    }
}

pub const BLACK: Color = rgb(0x000000);
pub const WHITE: Color = rgb(0xffffff);
pub const TRANSPARENT: Color = Color::TRANSPARENT;

pub const RED: ColorScale =
    ColorScale::from_hex([0xfef2f2, 0xffe2e2, 0xffc9c9, 0xffa2a2, 0xff6467, 0xfb2c36, 0xe7000b, 0xc10007, 0x9f0712, 0x82181a, 0x460809]);
pub const ORANGE: ColorScale =
    ColorScale::from_hex([0xfff7ed, 0xffedd4, 0xffd6a7, 0xffb86a, 0xff8904, 0xff6900, 0xf54900, 0xca3500, 0x9f2d00, 0x7e2a0c, 0x441306]);
pub const AMBER: ColorScale =
    ColorScale::from_hex([0xfffbeb, 0xfef3c6, 0xfee685, 0xffd230, 0xffb900, 0xfe9a00, 0xe17100, 0xbb4d00, 0x973c00, 0x7b3306, 0x461901]);
pub const YELLOW: ColorScale =
    ColorScale::from_hex([0xfefce8, 0xfef9c2, 0xfff085, 0xffdf20, 0xfdc700, 0xf0b100, 0xd08700, 0xa65f00, 0x894b00, 0x733e0a, 0x432004]);
pub const LIME: ColorScale =
    ColorScale::from_hex([0xf7fee7, 0xecfcca, 0xd8f999, 0xbbf451, 0x9ae600, 0x7ccf00, 0x5ea500, 0x497d00, 0x3c6300, 0x35530e, 0x192e03]);
pub const GREEN: ColorScale =
    ColorScale::from_hex([0xf0fdf4, 0xdcfce7, 0xb9f8cf, 0x7bf1a8, 0x05df72, 0x00c950, 0x00a63e, 0x008236, 0x016630, 0x0d542b, 0x032e15]);
pub const EMERALD: ColorScale =
    ColorScale::from_hex([0xecfdf5, 0xd0fae5, 0xa4f4cf, 0x5ee9b5, 0x00d492, 0x00bc7d, 0x009966, 0x007a55, 0x006045, 0x004f3b, 0x002c22]);
pub const TEAL: ColorScale =
    ColorScale::from_hex([0xf0fdfa, 0xcbfbf1, 0x96f7e4, 0x46ecd5, 0x00d5be, 0x00bba7, 0x009689, 0x00786f, 0x005f5a, 0x0b4f4a, 0x022f2e]);
pub const CYAN: ColorScale =
    ColorScale::from_hex([0xecfeff, 0xcefafe, 0xa2f4fd, 0x53eafd, 0x00d3f2, 0x00b8db, 0x0092b8, 0x007595, 0x005f78, 0x104e64, 0x053345]);
pub const SKY: ColorScale =
    ColorScale::from_hex([0xf0f9ff, 0xdff2fe, 0xb8e6fe, 0x74d4ff, 0x00bcff, 0x00a6f4, 0x0084d1, 0x0069a8, 0x00598a, 0x024a70, 0x052f4a]);
pub const BLUE: ColorScale =
    ColorScale::from_hex([0xeff6ff, 0xdbeafe, 0xbedbff, 0x8ec5ff, 0x51a2ff, 0x2b7fff, 0x155dfc, 0x1447e6, 0x193cb8, 0x1c398e, 0x162456]);
pub const INDIGO: ColorScale =
    ColorScale::from_hex([0xeef2ff, 0xe0e7ff, 0xc6d2ff, 0xa3b3ff, 0x7c86ff, 0x615fff, 0x4f39f6, 0x432dd7, 0x372aac, 0x312c85, 0x1e1a4d]);
pub const VIOLET: ColorScale =
    ColorScale::from_hex([0xf5f3ff, 0xede9fe, 0xddd6ff, 0xc4b4ff, 0xa684ff, 0x8e51ff, 0x7f22fe, 0x7008e7, 0x5d0ec0, 0x4d179a, 0x2f0d68]);
pub const PURPLE: ColorScale =
    ColorScale::from_hex([0xfaf5ff, 0xf3e8ff, 0xe9d4ff, 0xdab2ff, 0xc27aff, 0xad46ff, 0x9810fa, 0x8200db, 0x6e11b0, 0x59168b, 0x3c0366]);
pub const FUCHSIA: ColorScale =
    ColorScale::from_hex([0xfdf4ff, 0xfae8ff, 0xf6cfff, 0xf4a8ff, 0xed6aff, 0xe12afb, 0xc800de, 0xa800b7, 0x8a0194, 0x721378, 0x4b004f]);
pub const PINK: ColorScale =
    ColorScale::from_hex([0xfdf2f8, 0xfce7f3, 0xfccee8, 0xfda5d5, 0xfb64b6, 0xf6339a, 0xe60076, 0xc6005c, 0xa3004c, 0x861043, 0x510424]);
pub const ROSE: ColorScale =
    ColorScale::from_hex([0xfff1f2, 0xffe4e6, 0xffccd3, 0xffa1ad, 0xff637e, 0xff2056, 0xec003f, 0xc70036, 0xa50036, 0x8b0836, 0x4d0218]);
pub const SLATE: ColorScale =
    ColorScale::from_hex([0xf8fafc, 0xf1f5f9, 0xe2e8f0, 0xcad5e2, 0x90a1b9, 0x62748e, 0x45556c, 0x314158, 0x1d293d, 0x0f172b, 0x020618]);
pub const GRAY: ColorScale =
    ColorScale::from_hex([0xf9fafb, 0xf3f4f6, 0xe5e7eb, 0xd1d5dc, 0x99a1af, 0x6a7282, 0x4a5565, 0x364153, 0x1e2939, 0x101828, 0x030712]);
pub const ZINC: ColorScale =
    ColorScale::from_hex([0xfafafa, 0xf4f4f5, 0xe4e4e7, 0xd4d4d8, 0x9f9fa9, 0x71717b, 0x52525c, 0x3f3f46, 0x27272a, 0x18181b, 0x09090b]);
pub const NEUTRAL: ColorScale =
    ColorScale::from_hex([0xfafafa, 0xf5f5f5, 0xe5e5e5, 0xd4d4d4, 0xa1a1a1, 0x737373, 0x525252, 0x404040, 0x262626, 0x171717, 0x0a0a0a]);
pub const STONE: ColorScale =
    ColorScale::from_hex([0xfafaf9, 0xf5f5f4, 0xe7e5e4, 0xd6d3d1, 0xa6a09b, 0x79716b, 0x57534d, 0x44403b, 0x292524, 0x1c1917, 0x0c0a09]);

/// Gets a shade of a named color family, such as `by_name("violet", 500)`.
pub fn by_name(family: &str, step: u16) -> Option<Color> {
    let scale = match family {
        "red" => RED,
        "orange" => ORANGE,
        "amber" => AMBER,
        "yellow" => YELLOW,
        "lime" => LIME,
        "green" => GREEN,
        "emerald" => EMERALD,
        "teal" => TEAL,
        "cyan" => CYAN,
        "sky" => SKY,
        "blue" => BLUE,
        "indigo" => INDIGO,
        "violet" => VIOLET,
        "purple" => PURPLE,
        "fuchsia" => FUCHSIA,
        "pink" => PINK,
        "rose" => ROSE,
        "slate" => SLATE,
        "gray" => GRAY,
        "zinc" => ZINC,
        "neutral" => NEUTRAL,
        "stone" => STONE,
        _ => return None,
    };
    scale.step(step)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::luminance;

    #[test]
    fn all_color_families_support_the_same_eleven_shades() {
        let families = [
            ("red", RED),
            ("orange", ORANGE),
            ("amber", AMBER),
            ("yellow", YELLOW),
            ("lime", LIME),
            ("green", GREEN),
            ("emerald", EMERALD),
            ("teal", TEAL),
            ("cyan", CYAN),
            ("sky", SKY),
            ("blue", BLUE),
            ("indigo", INDIGO),
            ("violet", VIOLET),
            ("purple", PURPLE),
            ("fuchsia", FUCHSIA),
            ("pink", PINK),
            ("rose", ROSE),
            ("slate", SLATE),
            ("gray", GRAY),
            ("zinc", ZINC),
            ("neutral", NEUTRAL),
            ("stone", STONE),
        ];
        let steps = [50, 100, 200, 300, 400, 500, 600, 700, 800, 900, 950];
        for (name, scale) in families {
            for (step, color) in steps.into_iter().zip(scale.all()) {
                assert_eq!(by_name(name, step), Some(color));
                assert_eq!(scale.shade(Shade::from_step(step).unwrap()), color);
            }
            assert!(scale.all().windows(2).all(|adjacent| luminance(adjacent[0]) > luminance(adjacent[1])), "{name}");
        }
        assert_eq!(by_name("blue", 500), Some(rgb(0x2b7fff)));
        assert_eq!(by_name("blue", 55), None);
        assert_eq!(by_name("misspelled", 500), None);
        assert_eq!(TRANSPARENT.components[3], 0.0);
    }
}
