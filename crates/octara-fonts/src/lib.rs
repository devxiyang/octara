//! Fonts for deterministic tests and screenshots: octara-text bundles only Latin faces (Inter, IBM
//! Plex Mono), and windows use the platform's fonts, so text in other scripts renders as
//! missing glyphs headless. Register these before mounting instead of relying on the machine's fonts:
//!
//! ```
//! # #[cfg(feature = "cjk")]
//! # {
//! use octara::app::AppConfig;
//! use octara::prelude::*;
//! use octara_fonts::cjk;
//! let mut config = AppConfig::default();
//! for data in [cjk::REGULAR, cjk::SEMIBOLD] {
//!     config.text_engine.register_fallback_font(data, &cjk::SCRIPTS);
//! }
//! let app = App::with_config(octara::platform::HeadlessPlatform::default(), config);
//! let window = app.open_window(WindowOptions::new("CJK"), || text("你好")).unwrap();
//! app.shutdown();
//! # }
//! ```
//!
//! This asset-only package has no runtime dependencies. Tests can use it as a dev-dependency.

/// Noto Sans SC (SIL OFL 1.1), subset to the 3755 most common hanzi (GB2312 level 1), CJK
/// symbols and punctuation, kana, fullwidth forms and general punctuation, at weights 400 and
/// 600. `tools/subset_cjk.py` rebuilds it.
#[cfg(feature = "cjk")]
pub mod cjk {
    pub const REGULAR: &[u8] = include_bytes!("../fonts/NotoSansSC-Regular-subset.ttf");
    pub const SEMIBOLD: &[u8] = include_bytes!("../fonts/NotoSansSC-SemiBold-subset.ttf");
    /// The family name, for `font_family` or a theme's font stack.
    pub const FAMILY: &str = "Noto Sans SC";
    /// The scripts it covers, which fall back to it.
    pub const SCRIPTS: [[u8; 4]; 3] = [*b"Hani", *b"Hira", *b"Kana"];
}
