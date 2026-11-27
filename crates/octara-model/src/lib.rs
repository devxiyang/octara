//! Shared Octara values and host protocols.

pub mod ask;
pub mod audit;
pub mod capture;
pub mod diagnostics;
pub mod geometry;
pub mod identity;
pub mod image;
pub mod input;
pub mod motion;
pub mod native;
pub mod overlay;
pub mod paint;
pub mod semantics;
pub mod style;
pub mod text;
pub mod theme;
pub mod window;

/// Keep independently updated, shared state on separate cache lines on both
/// 64-byte and 128-byte cache-line architectures.
#[doc(hidden)]
#[derive(Default)]
#[repr(C, align(128))]
pub struct CacheLine<T>(pub T);

impl<T> std::ops::Deref for CacheLine<T> {
    type Target = T;

    fn deref(&self) -> &T {
        &self.0
    }
}
