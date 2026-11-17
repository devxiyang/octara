//! Window-independent UI values, geometry and drawing protocols.
//!
//! These modules have no dependency on the Octara App, reactive runtime, UI kit or OS shell.
//! Renderers and component libraries share their types without depending on a running UI.
//! Stateful UI construction and input coordination remain in `octara`.

pub mod canvas;
pub mod capture;
pub mod icon;
pub mod identity;
pub mod image;
pub mod input;
pub mod layout;
pub mod motion;
pub mod native;
pub mod paint;
pub mod path;
pub mod text;
pub mod theme;
