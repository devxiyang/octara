//! Octara: agent-first native UI in Rust.
//!
//! - [`kernel`]: the retained node tree, invalidation categories, layout, semantics, counters;
//! - [`reactive`]: signals, named state, incremental collections, live text;
//! - [`view`]: elements, bindings, structure, list rendering;
//! - [`app`]: the runtime that mounts a view and dispatches events;
//! - [`testing`]: headless driving and from-scratch consistency checks;
//! - [`diagnostics`]: every reported problem's code, cause and fix;
//! - [`inspect`]: the semantic tree, scripts and checks tools drive a headless app with.
//!
//! Most apps only need the [`prelude`].

pub use accesskit;

pub mod app;
pub mod canvas;
pub mod diagnostics;
pub mod icon;
pub mod image;
pub mod inspect;
pub mod kernel;
pub mod motion;
pub mod native;
pub mod paint;
pub mod path;
pub mod reactive;
pub mod task;
pub mod testing;
pub mod view;

pub mod prelude {
    pub use crate::app::{App, Drag, DragPhase, Event, Key, KeyEvent, Modifiers};
    pub use crate::canvas::Canvas;
    pub use crate::icon::Icon;
    pub use crate::paint::Rect;
    pub use crate::image::{Image, ObjectFit};
    pub use crate::path::{FillRule, LineCap, LineJoin, Path, Stroke, Transform, point};
    pub use crate::kernel::{
        Align, BoxShadow, Breakpoint, Color, ContainerStep, Family, Fill, Paint, Placement, RadiusStep, Role, ShadowStep, Side, State,
        TextStep, Theme, Token, linear_color_stop, linear_gradient, rgb,
    };
    pub use crate::motion::{Curve, Spring};
    pub use crate::reactive::graph::{Memo, ReadSignal, Signal, memo, provide, state, untrack, use_context, watch};
    pub use crate::reactive::list::{Derived, FlatItem, FlatKey, GroupInfo, Groups, List, Source, list};
    pub use crate::reactive::text::{LiveText, live_text};
    pub use crate::task::{Load, Receiver, Resource, Sender, Task, background, channel, interval, resource, sleep, spawn};
    pub use crate::view::*;
}
