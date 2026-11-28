/// The representation consumed by the output's compositor or reader.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum OutputColor {
    /// Linear-light, premultiplied RGBA in an `Rgba16Float` texture.
    Linear = 0,
    /// sRGB display color with alpha ignored by the compositor.
    SrgbOpaque = 1,
    /// Straight-alpha sRGB, as used by PNG captures.
    SrgbStraight = 2,
    /// sRGB RGB components multiplied by alpha after encoding.
    SrgbPremultiplied = 3,
}
