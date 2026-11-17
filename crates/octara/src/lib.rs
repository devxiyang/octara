//! Octara: agent-first native UI in Rust.
//!
//! - [`reactive`]: signals, named state, incremental collections, text buffers;
//! - [`kernel`]: the retained node tree, invalidation categories, layout, semantics, counters;
//! - [`view`]: elements, bindings, structure, list rendering;
//! - [`app`]: the app and its windows: input events, frames, focus, keymap, clock, recording;
//! - [`window`]: opening and closing windows from app code;
//! - [`task`]: tasks, timers, channels and resources on the app clock;
//! - [`platform`]: what the app needs from the system, and the headless platform;
//! - [`octara_primitives`]: drawing, paths, images, theme values, motion and presentation protocols;
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
pub(crate) use octara_primitives::canvas;
pub mod capture;
pub mod conformance;
pub mod diagnostics;
pub(crate) use octara_primitives::icon;
mod identity;
pub(crate) use octara_primitives::image;
pub mod inspect;
pub mod kernel;
pub mod menu;
pub(crate) use octara_primitives::motion;
pub(crate) use octara_primitives::native;
pub(crate) use octara_primitives::paint;
pub(crate) use octara_primitives::path;
pub mod platform;
pub mod reactive;
pub mod task;
pub mod view;
pub mod window;

pub mod prelude {
    pub use crate::app::{App, AppConfig, Drag, DragPhase, Event, InputEvent, Key, KeyEvent, Modifiers, Window};
    pub use crate::ask::{PathPromptOptions, PromptLevel, popup_menu, prompt, prompt_for_new_path, prompt_for_paths};
    pub use crate::kernel::select::Copied;
    pub use crate::kernel::{NumericRange, Role, State};
    pub use crate::menu::{Menu, MenuItem, set_menus};
    pub use crate::platform::ClipboardItem;
    pub use crate::reactive::list::{ReadList, StateList, state_list};
    pub use crate::reactive::on_cleanup;
    pub use crate::reactive::text::{TextBuffer, text_buffer};
    pub use crate::reactive::{ReadSignal, Signal, Watch, memo, provide, state, untrack, use_context, watch};
    pub use crate::task::{AsyncMemo, AsyncState, Receiver, Sender, Task, async_memo, background, channel, interval, sleep, spawn};
    pub use crate::view::{
        Behavior, El, IntoElement, IntoProp, IntoRows, IntoText, Length, Patch, Rich, Span, Styled, View, bind_keys, breakpoint, clock,
        col, each, el, focus, following_tail, icon, image, instant, invoke, invoke_after_update, is_composing, keyed, on_command, open_url,
        paced, pointer_position_in, px, read_from_clipboard, relative, rems, rich, row, scope, scroll_into_view, scroll_to, scroll_to_end,
        set_theme, settle_motion, show, show_else, size_of, sp, spacer, text, text_input, ui, viewport, virtual_each, write_to_clipboard,
    };
    pub use crate::window::{WindowId, WindowKind, WindowOptions, close_window, current_window, open_window, windows};
    pub use accesskit::Live;
    pub use octara_primitives::icon::Icon;
    pub use octara_primitives::image::{Image, ObjectFit};
    pub use octara_primitives::input::CursorStyle;
    pub use octara_primitives::layout::{Align, Placement, Side};
    pub use octara_primitives::motion::{Curve, Spring};
    pub use octara_primitives::paint::Rect;
    pub use octara_primitives::theme::{
        BoxShadow, Breakpoint, Color, ContainerStep, Family, Fill, Paint, RadiusStep, ShadowStep, TextStep, Theme, Token,
        linear_color_stop, linear_gradient, rgb,
    };
}
