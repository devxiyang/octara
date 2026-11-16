//! Octara: agent-first native UI in Rust.
//!
//! - [`reactive`]: signals, named state, incremental collections, text buffers;
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
pub mod ask;
pub mod canvas;
pub mod capture;
pub mod conformance;
pub mod diagnostics;
pub mod icon;
mod identity;
pub mod image;
pub mod inspect;
pub mod kernel;
pub mod menu;
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
    pub use crate::ask::{PathPromptOptions, PromptLevel, popup_menu, prompt, prompt_for_new_path, prompt_for_paths};
    pub use crate::canvas::Canvas;
    pub use crate::icon::Icon;
    pub use crate::image::{Image, ObjectFit};
    pub use crate::kernel::select::Copied;
    pub use crate::kernel::{
        Align, BoxShadow, Breakpoint, Color, ContainerStep, Family, Fill, NumericRange, Paint, Placement, RadiusStep, Role, ShadowStep,
        Side, State, TextStep, Theme, Token, linear_color_stop, linear_gradient, rgb,
    };
    pub use crate::menu::{Menu, MenuItem, set_menus};
    pub use crate::motion::{Curve, Spring};
    pub use crate::native::{NativeEvent, Slot};
    pub use crate::paint::Rect;
    pub use crate::path::{FillRule, LineCap, LineJoin, Path, Stroke, Transform, point};
    pub use crate::platform::{ClipboardItem, CursorStyle};
    pub use crate::reactive::list::{ReadList, StateList, state_list};
    pub use crate::reactive::on_cleanup;
    pub use crate::reactive::text::{TextBuffer, text_buffer};
    pub use crate::reactive::{ReadSignal, Signal, Watch, memo, provide, state, untrack, use_context, watch};
    pub use crate::task::{AsyncMemo, AsyncState, Receiver, Sender, Task, async_memo, background, channel, interval, sleep, spawn};
    pub use crate::view::{
        Behavior, El, IntoElement, IntoProp, IntoRows, IntoText, Length, Patch, Rich, Span, Styled, View, bind_keys, breakpoint, canvas,
        clock, col, component, component_keyed, each, el, focus, following_tail, icon, instant, invoke, invoke_after_update, is_composing,
        keyed, native_view, on_command, open_url, paced, pointer_position_in, px, read_from_clipboard, relative, rems, rich, row,
        scroll_into_view, scroll_to, scroll_to_end, set_theme, settle_motion, show, show_else, size_of, sp, spacer, text, text_input,
        viewport, virtual_each, web_view, write_to_clipboard,
    };
    pub use crate::window::{WindowId, WindowKind, WindowOptions, close_window, current_window, open_window, windows};
    pub use accesskit::Live;
    pub use serde_json::json;
}
