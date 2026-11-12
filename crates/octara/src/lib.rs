//! Octara: agent-first native UI in Rust.
//!
//! - [`reactive`]: signals, named state, incremental collections, live text;
//! - [`kernel`]: the retained node tree, invalidation categories, layout, semantics, counters;
//! - [`view`]: elements, bindings, structure, list rendering;
//! - [`app`]: the app and its windows: input events, frames, focus, keymap, clock, recording;
//! - [`window`]: opening and closing windows from app code;
//! - [`task`]: tasks, timers, channels and resources on the app clock;
//! - [`platform`]: what the app needs from the system, and the headless platform;
//! - [`paint`], [`path`], [`icon`], [`image`], [`canvas`], [`motion`], [`native`]: what is drawn
//!   and how it moves;
//! - [`diagnostics`]: every reported problem's code, cause and fix;
//! - [`inspect`] and [`capture`]: the semantic tree, scripts, checks and pictures agents drive an
//!   app with.
//!
//! Most apps only need the [`prelude`].

pub use accesskit;
/// Native views' props and events are JSON values.
pub use serde_json;

pub mod app;
pub mod canvas;
pub mod capture;
pub mod diagnostics;
pub mod icon;
pub mod image;
pub mod inspect;
pub mod kernel;
pub mod motion;
pub mod native;
pub mod paint;
pub mod path;
pub mod platform;
pub mod reactive;
pub mod task;
pub mod view;
pub mod window;

pub mod prelude {
    pub use crate::app::{App, Drag, DragPhase, Event, InputEvent, Key, KeyEvent, Modifiers, Window};
    pub use crate::canvas::Canvas;
    pub use crate::icon::Icon;
    pub use crate::image::{Image, ObjectFit};
    pub use crate::kernel::{
        Align, BoxShadow, Breakpoint, Color, ContainerStep, Family, Fill, Paint, Placement, RadiusStep, Role, ShadowStep, Side, State,
        TextStep, Theme, Token, linear_color_stop, linear_gradient, rgb,
    };
    pub use crate::motion::{Curve, Spring};
    pub use crate::native::{NativeEvent, Slot};
    pub use crate::paint::Rect;
    pub use crate::path::{FillRule, LineCap, LineJoin, Path, Stroke, Transform, point};
    pub use crate::reactive::graph::{Memo, ReadSignal, Signal, memo, provide, state, untrack, use_context, watch};
    pub use crate::reactive::list::{Derived, FlatItem, FlatKey, GroupInfo, Groups, List, Source, list};
    pub use crate::reactive::text::{LiveText, live_text};
    pub use crate::task::{Load, Receiver, Resource, Sender, Task, background, channel, interval, resource, sleep, spawn};
    pub use crate::view::*;
    pub use crate::window::{WindowId, WindowKind, WindowOptions, close_window, current_window, open_window, windows};
    pub use accesskit::Live;
    pub use serde_json::json;
}
