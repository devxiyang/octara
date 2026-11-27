//! Deterministic font assets for testing and reference capture. Register the CJK subset with
//! `NativeEngine::register_font` before mounting when a test needs CJK glyphs. The
//! [CJK integration test](https://github.com/octoii-works/octara/blob/main/examples/tests/cjk.rs)
//! demonstrates fallback registration, weight selection and deterministic rendering.
//!
//! ```
//! # #[cfg(feature = "cjk")]
//! # {
//! use octara_fonts::cjk;
//!
//! assert!(cjk::SCRIPTS.contains(b"Hani"));
//! for face in [cjk::REGULAR, cjk::SEMIBOLD] {
//!     assert!(face.starts_with(&[0, 1, 0, 0]));
//! }
//! # }
//! ```
//!

/// Noto Sans SC (SIL OFL 1.1), subset to the 3755 most common hanzi (GB2312 level 1), CJK
/// symbols and punctuation, kana, fullwidth forms and general punctuation, at weights 400 and
/// 600. `tools/subset_cjk.py` rebuilds it.
#[cfg(feature = "cjk")]
pub mod cjk {
    pub const REGULAR: &[u8] = include_bytes!("../fonts/NotoSansSC-Regular-subset.ttf");
    pub const SEMIBOLD: &[u8] = include_bytes!("../fonts/NotoSansSC-SemiBold-subset.ttf");
    /// The family name, for `text_family` or a theme's font stack.
    pub const FAMILY: &str = "Noto Sans SC";
    /// The scripts it covers, which fall back to it.
    pub const SCRIPTS: [[u8; 4]; 3] = [*b"Hani", *b"Hira", *b"Kana"];
}
