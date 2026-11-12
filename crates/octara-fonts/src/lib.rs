//! Fonts for deterministic tests and screenshots: the core bundles only Latin faces (Inter, IBM
//! Plex Mono), and windows use the platform's fonts, so text in other scripts renders as
//! missing glyphs headless. A test registers these instead of relying on the machine's fonts:
//!
//! ```
//! let mut app = octara::app::Window::headless(|| octara::view::text("你好").into());
//! octara_fonts::cjk::install(&app.app());
//! ```
//!
//! They are a dev-dependency, not part of an app.

/// Noto Sans SC (SIL OFL 1.1), subset to the 3755 most common hanzi (GB2312 level 1), CJK
/// symbols and punctuation, kana, fullwidth forms and general punctuation, at weights 400 and
/// 600. `tools/subset_cjk.py` rebuilds it.
#[cfg(feature = "cjk")]
pub mod cjk {
    use octara::app::App;

    pub const REGULAR: &[u8] = include_bytes!("../fonts/NotoSansSC-Regular-subset.ttf");
    pub const SEMIBOLD: &[u8] = include_bytes!("../fonts/NotoSansSC-SemiBold-subset.ttf");
    /// The family name, for `font_family` or a theme's font stack.
    pub const FAMILY: &str = "Noto Sans SC";
    /// The scripts it covers, which fall back to it.
    pub const SCRIPTS: [[u8; 4]; 3] = [*b"Hani", *b"Hira", *b"Kana"];

    /// Registers both weights as the fallback for hanzi and kana.
    pub fn install(app: &App) {
        for data in [REGULAR, SEMIBOLD] {
            app.register_fallback_font(data, &SCRIPTS);
        }
    }
}
