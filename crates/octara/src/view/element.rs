//! Elements: constructors, properties (fixed or bound), semantics, interaction, and how an
//! element becomes a kernel node.

use super::style::{Patch, Styled};
use super::{Behavior, Children, Handler, IntoElement, Parent, Slot, Slots, View, mount, show};
use crate::app::{Drag, KeyEvent};
use crate::diagnostics as codes;
use crate::kernel::rich::Rich;

type Draw = Rc<dyn Fn(&mut crate::canvas::Canvas)>;
type OnNative = Rc<dyn Fn(&crate::native::NativeEvent)>;

use crate::kernel::{
    Breakpoint, Color, Fill, HandlerId, HandlerKind, Kernel, Kind, NodeId, NumericRange, Paint, Placement, Role, State, StyleProp, Token,
    When,
};
use crate::reactive::graph::{self, rt};
use crate::reactive::log::Changes;
use crate::reactive::text::{TextBuffer, TextChange};
use crate::reactive::{ReadSignal, Signal, untrack};
use std::cell::RefCell;
use std::panic::Location;
use std::rc::Rc;

// ---- property values ----

/// A property value: fixed, or a closure that becomes a binding.
#[doc(hidden)]
pub enum Prop<T> {
    Static(T),
    Dyn(Box<dyn Fn() -> T>),
}

/// What a property accepts: a value, signal, or closure (a binding re-run when its reads change).
pub trait IntoProp<T> {
    fn into_prop(self) -> Prop<T>;
}

impl IntoProp<String> for &str {
    fn into_prop(self) -> Prop<String> {
        Prop::Static(self.to_owned())
    }
}
impl IntoProp<String> for String {
    fn into_prop(self) -> Prop<String> {
        Prop::Static(self)
    }
}
impl<F: Fn() -> String + 'static> IntoProp<String> for F {
    fn into_prop(self) -> Prop<String> {
        Prop::Dyn(Box::new(self))
    }
}
impl IntoProp<Option<String>> for Option<String> {
    fn into_prop(self) -> Prop<Option<String>> {
        Prop::Static(self)
    }
}
impl<F: Fn() -> Option<String> + 'static> IntoProp<Option<String>> for F {
    fn into_prop(self) -> Prop<Option<String>> {
        Prop::Dyn(Box::new(self))
    }
}
impl IntoProp<bool> for bool {
    fn into_prop(self) -> Prop<bool> {
        Prop::Static(self)
    }
}
impl<F: Fn() -> bool + 'static> IntoProp<bool> for F {
    fn into_prop(self) -> Prop<bool> {
        Prop::Dyn(Box::new(self))
    }
}
impl IntoProp<f32> for f32 {
    fn into_prop(self) -> Prop<f32> {
        Prop::Static(self)
    }
}
impl<F: Fn() -> f32 + 'static> IntoProp<f32> for F {
    fn into_prop(self) -> Prop<f32> {
        Prop::Dyn(Box::new(self))
    }
}
impl IntoProp<NumericRange> for NumericRange {
    fn into_prop(self) -> Prop<NumericRange> {
        Prop::Static(self)
    }
}
impl<F: Fn() -> NumericRange + 'static> IntoProp<NumericRange> for F {
    fn into_prop(self) -> Prop<NumericRange> {
        Prop::Dyn(Box::new(self))
    }
}
impl IntoProp<serde_json::Value> for serde_json::Value {
    fn into_prop(self) -> Prop<serde_json::Value> {
        Prop::Static(self)
    }
}
impl<F: Fn() -> serde_json::Value + 'static> IntoProp<serde_json::Value> for F {
    fn into_prop(self) -> Prop<serde_json::Value> {
        Prop::Dyn(Box::new(self))
    }
}
impl IntoProp<Paint> for Token {
    fn into_prop(self) -> Prop<Paint> {
        Prop::Static(self.into())
    }
}
impl IntoProp<Paint> for Color {
    fn into_prop(self) -> Prop<Paint> {
        Prop::Static(self.into())
    }
}
impl IntoProp<Paint> for Paint {
    fn into_prop(self) -> Prop<Paint> {
        Prop::Static(self)
    }
}
macro_rules! static_fill {
    ($($t:ty),*) => {$(
        impl IntoProp<Fill> for $t {
            fn into_prop(self) -> Prop<Fill> {
                Prop::Static(self.into())
            }
        }
    )*};
}
static_fill!(Token, Color, Paint, Fill);
/// A closure returning a `Token`, a `Color`, a `Paint` or a `Fill`.
impl<F: Fn() -> P + 'static, P: Into<Fill>> IntoProp<Fill> for F {
    fn into_prop(self) -> Prop<Fill> {
        Prop::Dyn(Box::new(move || self().into()))
    }
}
/// A closure returning a `Token`, a `Color` or a `Paint`.
impl<F: Fn() -> P + 'static, P: Into<Paint>> IntoProp<Paint> for F {
    fn into_prop(self) -> Prop<Paint> {
        Prop::Dyn(Box::new(move || self().into()))
    }
}

/// Rich text: a fixed one, or a closure returning a string or a `Rich`.
impl IntoProp<Rich> for &str {
    fn into_prop(self) -> Prop<Rich> {
        Prop::Static(self.into())
    }
}
impl IntoProp<Rich> for String {
    fn into_prop(self) -> Prop<Rich> {
        Prop::Static(self.into())
    }
}
impl IntoProp<Rich> for &String {
    fn into_prop(self) -> Prop<Rich> {
        Prop::Static(self.into())
    }
}
impl IntoProp<Rich> for Rich {
    fn into_prop(self) -> Prop<Rich> {
        Prop::Static(self)
    }
}
impl<F: Fn() -> R + 'static, R: Into<Rich>> IntoProp<Rich> for F {
    fn into_prop(self) -> Prop<Rich> {
        Prop::Dyn(Box::new(move || self().into()))
    }
}
/// A string property (a component's `label`) shown as text.
impl IntoProp<Rich> for Prop<String> {
    fn into_prop(self) -> Prop<Rich> {
        match self {
            Prop::Static(s) => Prop::Static(s.into()),
            Prop::Dyn(f) => Prop::Dyn(Box::new(move || f().into())),
        }
    }
}

// Signals use the same bindings as closures. Converted text/paint values follow the existing
// closure conversions; TextBuffer keeps its separate incremental path below.
macro_rules! signal_prop {
    ($handle:ident, $target:ty) => {
        impl<T: Clone + Into<$target> + 'static> IntoProp<$target> for $handle<T> {
            fn into_prop(self) -> Prop<$target> {
                Prop::Dyn(Box::new(move || self.get().into()))
            }
        }
    };
}
macro_rules! signal_props {
    ($handle:ident) => {
        signal_prop!($handle, String);
        signal_prop!($handle, Option<String>);
        signal_prop!($handle, bool);
        signal_prop!($handle, f32);
        signal_prop!($handle, NumericRange);
        signal_prop!($handle, serde_json::Value);
        signal_prop!($handle, Rich);
        signal_prop!($handle, Paint);
        signal_prop!($handle, Fill);
    };
}
signal_props!(Signal);
signal_props!(ReadSignal);

/// What `text` accepts: a string or [`Rich`] text, fixed or from a closure (re-computed whole),
/// or a [`TextBuffer`] (its edits reach the kernel as edits: an append costs O(appended)).
#[doc(hidden)]
pub enum TextContent {
    Prop(Prop<Rich>),
    Buffer(TextBuffer),
}

pub trait IntoText {
    fn into_text(self) -> TextContent;
}
impl<T: IntoProp<Rich>> IntoText for T {
    fn into_text(self) -> TextContent {
        TextContent::Prop(self.into_prop())
    }
}
impl IntoText for TextBuffer {
    fn into_text(self) -> TextContent {
        TextContent::Buffer(self)
    }
}
impl IntoText for &TextBuffer {
    fn into_text(self) -> TextContent {
        TextContent::Buffer(self.clone())
    }
}

enum StyleValue {
    Static(StyleProp),
    Dyn(Box<dyn Fn() -> StyleProp>),
    /// Styles a closure gives (`style_with`), re-run when what it reads changes.
    Patch(Box<dyn Fn(Patch) -> Patch>),
}

struct Overlay {
    open: Box<dyn Fn() -> bool>,
    placement: Option<Placement>,
    content: Rc<dyn Fn() -> View>,
    at: &'static Location<'static>,
    made: Option<crate::kernel::Component>,
    /// Placed by this node instead of the element (the window's tooltip, by its element).
    by: Option<NodeId>,
}

// ---- elements ----

enum Content {
    Box(Vec<View>),
    Text(TextContent),
    Input(Input),
    Image { image: crate::image::Image, fit: crate::image::ObjectFit },
    Icon { icon: crate::icon::Icon, stroke_width: Option<f32> },
    Canvas(Draw),
    Native(Native),
}

#[derive(Default)]
struct Input {
    placeholder: Option<Prop<String>>,
    multiline: bool,
    rows: Option<(u16, u16)>,
    submit_on_enter: bool,
    secure: bool,
    revealed: Option<Prop<bool>>,
}

struct Native {
    kind: &'static str,
    slot: crate::native::Slot,
    props: Option<Prop<serde_json::Value>>,
    on_event: Option<OnNative>,
}

impl Content {
    #[track_caller]
    fn children_mut(&mut self, method: &str) -> &mut Vec<View> {
        match self {
            Self::Box(children) => children,
            _ => wrong_content(method, "a container: el(), row() or col()"),
        }
    }

    fn kind(&self) -> Kind {
        match self {
            Self::Text(_) => Kind::Text,
            Self::Input(_) => Kind::TextInput,
            _ => Kind::Box,
        }
    }

    #[track_caller]
    fn input_mut(&mut self, method: &str) -> &mut Input {
        match self {
            Self::Input(input) => input,
            _ => wrong_content(method, "text_input()"),
        }
    }

    #[track_caller]
    fn native_mut(&mut self, method: &str) -> &mut Native {
        match self {
            Self::Native(native) => native,
            _ => wrong_content(method, "native_view() or web_view()"),
        }
    }
}

// Builders can be assembled before entering an App. Reject misuse at the caller, without
// requiring a runtime or creating a deferred invalid-content state to report at mount time.
#[track_caller]
fn wrong_content(method: &str, expected: &str) -> ! {
    let code = &codes::ELEMENT_CONTENT;
    panic!("{code}: .{method}() requires {expected}")
}

/// An element with one content kind and shared styles, semantics and handlers.
/// Only containers (`el`, `row`, `col`) accept application children; compose atomic content
/// such as text, inputs and images inside an external container.
/// Content-specific methods panic with `ELEMENT_CONTENT` on a mismatched kind; for example,
/// use `text_input().rows(..)` and `el().img(..).object_fit(..)`.
pub struct El {
    content: Content,
    role: Option<Role>,
    at: &'static Location<'static>,
    /// The `#[component]` function building it.
    made: Option<crate::kernel::Component>,
    test_id: Option<Prop<String>>,
    value: Option<Prop<String>>,
    numeric_range: Option<Prop<NumericRange>>,
    checked: Option<Prop<bool>>,
    required: Option<Prop<bool>>,
    invalid: Option<Prop<bool>>,
    label: Option<Prop<String>>,
    description: Option<Prop<String>>,
    tooltip: Option<Prop<String>>,
    expanded: Option<Prop<bool>>,
    active_descendant: Option<Prop<Option<String>>>,
    selected: Option<Prop<bool>>,
    disabled: Option<Prop<bool>>,
    hidden: bool,
    focusable: bool,
    styles: Vec<StyleValue>,
    overrides: Vec<(When, Vec<StyleProp>)>,
    handlers: Behavior,
    behaviors: Vec<Behavior>,
    tab_index: i32,
    focus_group: bool,
    preserve_focus_on_press: bool,
    autofocus: bool,
    modal: Option<(bool, Rc<dyn Fn()>)>,
    key_context: Option<String>,
    commands: Vec<(String, Rc<dyn Fn()>)>,
    group: Option<&'static str>,
    hover_alone: bool,
    region: bool,
    level: Option<u8>,
    live: Option<accesskit::Live>,
    copy_markdown: Option<crate::kernel::select::CopyMarkdown>,
    cursor: Option<octara_primitives::input::CursorStyle>,
    follow_tail: bool,
    select_none: bool,
    overlays: Vec<Overlay>,
    anchor: Option<(NodeId, Placement)>,
    overlay_owner: Option<NodeId>,
}

impl From<El> for View {
    fn from(e: El) -> View {
        View::element(Box::new(e))
    }
}

fn new_el(content: Content, at: &'static Location<'static>) -> El {
    El {
        content,
        role: None,
        at,
        made: super::building(),
        test_id: None,
        value: None,
        numeric_range: None,
        checked: None,
        required: None,
        invalid: None,
        label: None,
        description: None,
        tooltip: None,
        expanded: None,
        active_descendant: None,
        selected: None,
        disabled: None,
        hidden: false,
        focusable: false,
        styles: Vec::new(),
        overrides: Vec::new(),
        handlers: Behavior::default(),
        behaviors: Vec::new(),
        tab_index: 0,
        focus_group: false,
        preserve_focus_on_press: false,
        autofocus: false,
        modal: None,
        key_context: None,
        commands: Vec::new(),
        group: None,
        hover_alone: false,
        region: false,
        level: None,
        live: None,
        copy_markdown: None,
        cursor: None,
        follow_tail: false,
        select_none: false,
        overlays: Vec::new(),
        anchor: None,
        overlay_owner: None,
    }
}

fn boxed(children: impl Children, at: &'static Location<'static>) -> El {
    new_el(Content::Box(children.into_views()), at)
}

/// A box: a flex container laying children out in a row until told otherwise (Rune's `el()`).
/// Add children with `.child(..)` / `.children(..)`.
#[track_caller]
pub fn el() -> El {
    new_el(Content::Box(Vec::new()), Location::caller())
}

/// A horizontal stack (Rune `row()`, SwiftUI `HStack`, Compose `Row`).
#[track_caller]
pub fn row() -> El {
    el().flex_row()
}

/// A vertical stack (Rune `col()`, SwiftUI `VStack`, Compose `Column`).
#[doc(alias = "column")]
#[track_caller]
pub fn col() -> El {
    el().flex_col()
}

/// Takes the free space along its parent's main axis.
#[track_caller]
pub fn spacer() -> El {
    el().flex_grow().a11y_hidden()
}

/// Text: `text("fixed")`, `text(move || format!(..))`, or `text(live)` for a [`TextBuffer`].
#[track_caller]
pub fn text(t: impl IntoText) -> El {
    text_at(t, Location::caller())
}

pub(crate) fn text_at(t: impl IntoText, at: &'static Location<'static>) -> El {
    new_el(Content::Text(t.into_text()), at)
}

/// An icon (`icon(icons::SEARCH)`, from `octara-icons`, or an [`Icon`](crate::icon::Icon) of
/// your own): a square, 16 px by default (`size_5`, `size_6` to change), drawn in the text color,
/// so it follows its button's color and states. Decorative (hidden from assistive technology)
/// unless it has a `label`, which makes it an image.
#[track_caller]
pub fn icon(icon: crate::icon::Icon) -> El {
    new_el(Content::Icon { icon, stroke_width: None }, Location::caller()).size_4()
}

/// A canvas: `draw` records shapes, images, transforms and clips into a
/// [`Canvas`](crate::canvas::Canvas) sized to the element's content box. It reruns only when what
/// it reads (signals, the element's size) changes. A canvas has no size of its own: size it like
/// a box (`w`, `h`, `flex_1`, `size_full`). It is an image to assistive technology: give it a
/// `label`, or `a11y_hidden()` when it is decoration. See [`crate::canvas`].
#[track_caller]
pub fn canvas(draw: impl Fn(&mut crate::canvas::Canvas) + 'static) -> El {
    new_el(Content::Canvas(Rc::new(draw)), Location::caller())
}

/// A native view of `kind` (a platform view the shell's host creates): a leaf box sized by its
/// authored size, else the size the view reports; in the overlay slot unless `.slot(..)` says
/// otherwise. Its props go to the host whole (`.props(..)`), its events come back through
/// `.on_native(..)`. Headless, it draws as a placeholder box. See [`crate::native`].
#[track_caller]
pub fn native_view(kind: &'static str) -> El {
    new_el(Content::Native(Native { kind, slot: crate::native::Slot::Overlay, props: None, on_event: None }), Location::caller())
}

/// A web view showing `url` (a native view of kind `web`, in the overlay): its props are
/// `{"url": ..}`; its host sends `load` events (`{"url": ..}`) through `.on_native(..)`.
#[track_caller]
pub fn web_view(url: impl IntoProp<String>) -> El {
    let props = match url.into_prop() {
        Prop::Static(u) => Prop::Static(serde_json::json!({ "url": u })),
        Prop::Dyn(f) => Prop::Dyn(Box::new(move || serde_json::json!({ "url": f() }))),
    };
    let mut e = native_view("web");
    e.content.native_mut("web_view").props = Some(props);
    e
}

/// A text input. Text input needs the platform's input protocol (IME), so it is a core
/// primitive; `octara-kit`'s `TextField` builds on it.
#[track_caller]
pub fn text_input() -> El {
    new_el(Content::Input(Input::default()), Location::caller()).focusable()
}

impl Styled for El {
    fn style(mut self, p: StyleProp) -> Self {
        self.styles.push(StyleValue::Static(p));
        self
    }
}

impl El {
    /// Adds a component's event behavior before the application's `on_*` callbacks.
    /// Each call adds a layer in registration order; within a layer, setting the same event
    /// twice replaces it. Application callbacks remain replaceable independently of layers.
    /// A consumed key or retired mounted scope stops subsequent callbacks. IME routing runs
    /// first. Commands, modal dismissal and native protocols keep their dedicated owners.
    pub fn behavior(mut self, configure: impl FnOnce(Behavior) -> Behavior) -> Self {
        self.behaviors.push(configure(Behavior::default()));
        self
    }

    fn dyn_style<T: 'static>(mut self, prop: Prop<T>, make: fn(T) -> StyleProp) -> Self {
        self.styles.push(match prop {
            Prop::Static(p) => StyleValue::Static(make(p)),
            Prop::Dyn(f) => StyleValue::Dyn(Box::new(move || make(f()))),
        });
        self
    }

    /// Background: a token, a color, a gradient (`linear_gradient(..)`), or a closure returning one.
    pub fn bg(self, p: impl IntoProp<Fill>) -> Self {
        self.dyn_style(p.into_prop(), |p| StyleProp::Background(Some(p)))
    }
    pub fn text_color(self, p: impl IntoProp<Paint>) -> Self {
        self.dyn_style(p.into_prop(), StyleProp::TextColor)
    }
    pub fn border_color(self, p: impl IntoProp<Paint>) -> Self {
        self.dyn_style(p.into_prop(), StyleProp::BorderColor)
    }
    pub fn opacity(mut self, o: impl IntoProp<f32>) -> Self {
        self.styles.push(match o.into_prop() {
            Prop::Static(v) => StyleValue::Static(StyleProp::Opacity(v)),
            Prop::Dyn(f) => StyleValue::Dyn(Box::new(move || StyleProp::Opacity(f()))),
        });
        self
    }

    /// Styles from a closure, re-run when what it reads changes: any style method, bound
    /// (`.style_with(move |s| s.w(width.get()))`, a pane a person resizes). Its declaration
    /// position stays fixed: later base styles win. Each run replaces this patch; omitted
    /// properties fall back to earlier declarations or defaults. State overrides apply after
    /// base styles; use `style_when` for interaction states.
    pub fn style_with(mut self, f: impl Fn(Patch) -> Patch + 'static) -> Self {
        self.styles.push(StyleValue::Patch(Box::new(f)));
        self
    }

    // ---- state overrides ----

    /// Style while hovered: the mouse is over this element or anything inside it (GPUI, CSS).
    /// Touch does not hover.
    pub fn hover(self, f: impl FnOnce(Patch) -> Patch) -> Self {
        self.style_when(State::HOVER, f)
    }
    /// Names this element a hover group: descendants style themselves on its hover with
    /// `group_hover` (GPUI's `group`): a row showing its delete button while hovered.
    pub fn group(mut self, name: &'static str) -> Self {
        self.group = Some(name);
        self
    }
    /// Style while the nearest ancestor named `group` is hovered (GPUI's `group_hover`).
    pub fn group_hover(mut self, group: &'static str, f: impl FnOnce(Patch) -> Patch) -> Self {
        self.overrides.push((When::GroupHover(group), f(Patch::default()).0));
        self
    }
    /// While hovered, the elements around this one are not (Rune's `hover_alone`): a card inside
    /// a hoverable card lights up alone.
    pub fn hover_alone(mut self) -> Self {
        self.hover_alone = true;
        self
    }
    /// Its texts can be selected as one document: press and drag across them, double press a
    /// word, triple press a text, Shift+press to extend, `mod+a` and `mod+c` (a chat transcript,
    /// a message, Markdown). Rows of its lists keep their part of a selection while they scroll
    /// out and back.
    pub fn selectable(mut self) -> Self {
        self.region = true;
        self
    }
    /// A scroll container keeping the end of its content in view as the content grows (a
    /// transcript, a log, a terminal; GPUI's `FollowMode::Tail`). Scrolling away from the end
    /// lets go of it; scrolling back to the end, or `scroll_to_end`, follows again. Read whether
    /// it follows with `following_tail` (to offer a way back to the end).
    pub fn follow_tail(mut self) -> Self {
        self.follow_tail = true;
        self
    }
    /// Its texts are not selected (list markers, labels of controls inside a selectable region).
    pub fn select_none(mut self) -> Self {
        self.select_none = true;
        self
    }
    /// Style while pressed (Tailwind's `active:`).
    #[doc(alias = "pressed")]
    pub fn active(self, f: impl FnOnce(Patch) -> Patch) -> Self {
        self.style_when(State::PRESSED, f)
    }
    /// Style while focused and the last input was the keyboard: the focus ring (GPUI
    /// `.focus_visible(..)`). A press focuses without it.
    pub fn focus_visible(self, f: impl FnOnce(Patch) -> Patch) -> Self {
        self.style_when(State::FOCUS_VISIBLE, f)
    }
    /// Style while focused, however focus got there (GPUI `.focus(..)`).
    pub fn focus(self, f: impl FnOnce(Patch) -> Patch) -> Self {
        self.style_when(State::FOCUSED, f)
    }
    /// Style while `state` is active (for `SELECTED` and `DISABLED`, set with `selected` / `disabled`).
    pub fn style_when(mut self, state: State, f: impl FnOnce(Patch) -> Patch) -> Self {
        self.overrides.push((When::State(state), f(Patch::default()).0));
        self
    }

    /// Style while input is touch (iOS, Android): touch sizes for controls, as a coarse-pointer
    /// media query does. Resolved by the kernel when the platform changes: no closure reruns.
    pub fn when_touch(mut self, f: impl FnOnce(Patch) -> Patch) -> Self {
        self.overrides.push((When::Touch, f(Patch::default()).0));
        self
    }

    // ---- window size classes (smallest first) ----

    /// Style from size class `bp` up. Resolved by the kernel when the window crosses a class
    /// boundary: no closure reruns.
    pub fn screen(mut self, bp: Breakpoint, f: impl FnOnce(Patch) -> Patch) -> Self {
        self.overrides.push((When::Screen(bp), f(Patch::default()).0));
        self
    }
    /// From the medium size class up (600 dp: tablets in portrait, small windows).
    pub fn when_medium(self, f: impl FnOnce(Patch) -> Patch) -> Self {
        self.screen(Breakpoint::Medium, f)
    }
    /// From the expanded size class up (840 dp: tablets in landscape, desktop windows).
    pub fn when_expanded(self, f: impl FnOnce(Patch) -> Patch) -> Self {
        self.screen(Breakpoint::Expanded, f)
    }
    /// From the large size class up (1200 dp).
    pub fn when_large(self, f: impl FnOnce(Patch) -> Patch) -> Self {
        self.screen(Breakpoint::Large, f)
    }
    /// From the extra-large size class up (1600 dp).
    pub fn when_extra_large(self, f: impl FnOnce(Patch) -> Patch) -> Self {
        self.screen(Breakpoint::ExtraLarge, f)
    }

    // ---- semantics ----

    /// The test id, which is also the node's semantic id.
    pub fn id(mut self, id: impl IntoProp<String>) -> Self {
        self.test_id = Some(id.into_prop());
        self
    }
    pub fn role(mut self, r: Role) -> Self {
        self.role = Some(r);
        self
    }
    pub fn label(mut self, l: impl IntoProp<String>) -> Self {
        self.label = Some(l.into_prop());
        self
    }
    /// Text that shows when the mouse rests on it (GPUI's `tooltip`): the window's one tooltip,
    /// below it, gone when hover leaves or on a press, a scroll or a key. It is also its
    /// description for assistive technology (unless it has one). An icon button names its action
    /// this way.
    pub fn tooltip(mut self, t: impl IntoProp<String>) -> Self {
        self.tooltip = Some(t.into_prop());
        self
    }
    pub fn description(mut self, d: impl IntoProp<String>) -> Self {
        self.description = Some(d.into_prop());
        self
    }
    pub fn expanded(mut self, e: impl IntoProp<bool>) -> Self {
        self.expanded = Some(e.into_prop());
        self
    }
    /// The active option's test id, while keyboard focus stays on this element (a searchable
    /// combo box's input, for example). This does not select the option or move focus. Missing,
    /// detached and accessibility-hidden targets are omitted until they are shown again.
    pub fn active_descendant(mut self, id: impl IntoProp<Option<String>>) -> Self {
        self.active_descendant = Some(id.into_prop());
        self
    }
    pub fn checked(mut self, c: impl IntoProp<bool>) -> Self {
        self.checked = Some(c.into_prop());
        self
    }
    /// Whether user input or selection is required, for assistive technology and inspection.
    pub fn required(mut self, required: impl IntoProp<bool>) -> Self {
        self.required = Some(required.into_prop());
        self
    }
    /// Whether the current value is invalid, for assistive technology and inspection.
    pub fn invalid(mut self, invalid: impl IntoProp<bool>) -> Self {
        self.invalid = Some(invalid.into_prop());
        self
    }
    pub fn value(mut self, v: impl IntoProp<String>) -> Self {
        self.value = Some(v.into_prop());
        self
    }
    /// Copies the selected texts in it as Markdown (`edit.copy_markdown`, `mod+alt+c`): `f` gets
    /// them in order (the keys of the list rows below this element, the selected part, whether
    /// it is the whole text) and gives the Markdown, or `None` to copy their text.
    pub fn copy_markdown(mut self, f: impl Fn(&[crate::kernel::select::Copied]) -> Option<String> + 'static) -> Self {
        self.copy_markdown = Some(Rc::new(f));
        self
    }
    /// How the pointer looks over it and its descendants (unless one sets its own).
    pub fn cursor(mut self, style: octara_primitives::input::CursorStyle) -> Self {
        self.cursor = Some(style);
        self
    }
    /// The pointing hand (a link's look; native buttons keep the arrow).
    pub fn cursor_pointer(self) -> Self {
        self.cursor(octara_primitives::input::CursorStyle::PointingHand)
    }
    /// The I-beam, as over text.
    pub fn cursor_text(self) -> Self {
        self.cursor(octara_primitives::input::CursorStyle::IBeam)
    }
    /// The arrow (over selectable text or a field inside it too).
    pub fn cursor_default(self) -> Self {
        self.cursor(octara_primitives::input::CursorStyle::Arrow)
    }
    /// The open hand (something to drag).
    pub fn cursor_grab(self) -> Self {
        self.cursor(octara_primitives::input::CursorStyle::OpenHand)
    }
    /// The closed hand (dragging it).
    pub fn cursor_grabbing(self) -> Self {
        self.cursor(octara_primitives::input::CursorStyle::ClosedHand)
    }
    /// Its level for assistive technology: a heading's (1 to 6), a list item's nesting.
    pub fn level(mut self, level: u8) -> Self {
        self.level = Some(level);
        self
    }
    /// A live region: assistive technology announces its changes, politely (after what it is
    /// saying) or assertively (at once).
    pub fn live(mut self, live: accesskit::Live) -> Self {
        self.live = Some(live);
        self
    }
    /// Hidden from assistive technology (decorative parts, spacers).
    pub fn a11y_hidden(mut self) -> Self {
        self.hidden = true;
        self
    }

    // ---- interaction ----

    pub fn focusable(mut self) -> Self {
        self.focusable = true;
        self
    }
    /// Tab order: positive values come before the rest, in order; 0 (the default) follows the
    /// tree; a negative value keeps the element focusable but out of the Tab order.
    pub fn tab_index(mut self, i: i32) -> Self {
        self.tab_index = i;
        self
    }
    /// Arrow keys move focus among the focusable elements inside, and Tab stops once (on the
    /// focused or selected one; radio groups use their checked radio). Menus, lists and trees
    /// move on Up and Down; other groups (tabs, toolbars, radio groups) on Left and Right too.
    /// Home and End go to the ends. Moving to a radio with these keys presses it to choose it.
    pub fn focus_group(mut self) -> Self {
        self.focus_group = true;
        self
    }
    /// Keys while focus is on this element or inside it, before the keymap: return `true` to
    /// consume the key. Innermost handlers run first. Replaces the previous application
    /// callback; [`Self::behavior`] handlers remain and run first.
    pub fn on_key(mut self, f: impl Fn(&KeyEvent) -> bool + 'static) -> Self {
        self.handlers = self.handlers.on_key(f);
        self
    }

    /// Handles `command` (`"save"`, `"edit.copy"`) while focus is on this element or inside it:
    /// its chords in the keymap, `App::invoke` and menus run it.
    pub fn on_command(mut self, command: &str, f: impl Fn() + 'static) -> Self {
        self.commands.push((command.to_owned(), Rc::new(f)));
        self
    }
    /// Names a key context for this subtree: bindings with that context apply while focus is
    /// inside, over less specific ones (GPUI's `key_context`).
    pub fn key_context(mut self, name: &str) -> Self {
        self.key_context = Some(name.to_owned());
        self
    }
    /// Takes focus when the modal around it opens (else the modal's first text field, else its
    /// first Tab stop).
    pub fn autofocus(mut self) -> Self {
        self.autofocus = true;
        self
    }
    /// While shown, the rest of the window is inert: presses outside reach nothing, and Tab
    /// stays inside. It takes focus when it opens and gives it back when it closes. Escape —
    /// and, with `click_outside`, a press outside — calls `dismiss`, which should close it.
    pub fn modal(mut self, click_outside: bool, dismiss: impl Fn() + 'static) -> Self {
        self.modal = Some((click_outside, Rc::new(dismiss)));
        self
    }
    pub fn selected(mut self, s: impl IntoProp<bool>) -> Self {
        self.selected = Some(s.into_prop());
        self
    }
    /// A disabled element refuses events and applies its `DISABLED` overrides.
    pub fn disabled(mut self, d: impl IntoProp<bool>) -> Self {
        self.disabled = Some(d.into_prop());
        self
    }
    /// A press: a click or tap that ends on the element, Enter or Space while it is focused, or
    /// assistive technology's activate.
    pub fn on_press(mut self, f: impl Fn() + 'static) -> Self {
        self.handlers = self.handlers.on_press(f);
        self
    }
    /// Pointer presses in this subtree keep the current keyboard focus. Useful for an
    /// editor's suggestions: choosing a row must not blur the editor or end its composition.
    /// Keyboard and semantic focus operations are unchanged. This does not make a popup modal.
    pub fn preserve_focus_on_press(mut self, preserve: bool) -> Self {
        self.preserve_focus_on_press = preserve;
        self
    }
    /// A text field's submission: Enter while it has focus (and no input method is composing).
    /// Without one, Enter presses the field like any focused element. Replaces only the
    /// application callback; component behavior still runs first.
    pub fn on_submit(mut self, f: impl Fn() + 'static) -> Self {
        self.handlers = self.handlers.on_submit(f);
        self
    }
    /// It got keyboard focus (GPUI's `on_focus`).
    pub fn on_focus(mut self, f: impl Fn() + 'static) -> Self {
        self.handlers = self.handlers.on_focus(f);
        self
    }
    /// It lost keyboard focus (GPUI's `on_blur`): focus moved elsewhere or was cleared, not the
    /// window going inactive (it keeps its focus) nor the element leaving the tree. A field
    /// edited in place commits here. Replaces the previous application callback, after any
    /// [`Self::behavior`] handlers.
    pub fn on_blur(mut self, f: impl Fn() + 'static) -> Self {
        self.handlers = self.handlers.on_blur(f);
        self
    }

    /// The second press of a double click or double tap (after its `on_press`).
    pub fn on_double_press(mut self, f: impl Fn() + 'static) -> Self {
        self.handlers = self.handlers.on_double_press(f);
        self
    }
    /// A context menu request: a right click (Control-click on macOS), a long press on touch
    /// (without `on_long_press`), or assistive technology's "show menu". The nearest element
    /// with one, from the pointer up, gets it.
    pub fn on_secondary_press(mut self, f: impl Fn() + 'static) -> Self {
        self.handlers = self.handlers.on_secondary_press(f);
        self
    }
    /// A touch held still for the long-press duration (500 ms by default); the release then
    /// presses nothing.
    pub fn on_long_press(mut self, f: impl Fn() + 'static) -> Self {
        self.handlers = self.handlers.on_long_press(f);
        self
    }
    /// A drag starting on the element: `Start` once the pointer moves past the slop (4 pt with a
    /// mouse, 8 pt with touch), then `Move`, then `End` (or `Cancel`). A drag is not a press.
    pub fn on_drag(mut self, f: impl Fn(&Drag) + 'static) -> Self {
        self.handlers = self.handlers.on_drag(f);
        self
    }
    /// A finite numeric value requested by assistive technology or an agent. The component's
    /// behavior applies its own clamping and stepping before this callback. The argument remains
    /// the proposed value; read the model to observe its accepted value.
    pub fn on_numeric_input(mut self, f: impl Fn(f64) + 'static) -> Self {
        self.handlers = self.handlers.on_numeric_input(f);
        self
    }
    /// Numeric value, bounds and optional step for accessibility and Inspector. A missing
    /// value means indeterminate progress or a draft which does not currently parse.
    pub fn numeric_range(mut self, range: impl IntoProp<NumericRange>) -> Self {
        self.numeric_range = Some(range.into_prop());
        self
    }
    /// Receives proposed input, after any [`Self::behavior`] handlers. Replaces the
    /// previous application callback. A bare text input uses this callback to write its value;
    /// a bound component writes its model in `behavior` before notifying this callback.
    pub fn on_input(mut self, f: impl Fn(String) + 'static) -> Self {
        self.handlers = self.handlers.on_input(f);
        self
    }

    pub fn on_toggle(mut self, f: impl Fn() + 'static) -> Self {
        self.handlers = self.handlers.on_toggle(f);
        self
    }
    /// While `open` holds, shows `content` above everything else, placed next to this element
    /// (flipped and clamped to stay in the window). Its state lives while it is open.
    #[track_caller]
    pub fn overlay(mut self, open: impl Fn() -> bool + 'static, placement: Placement, content: impl Fn() -> View + 'static) -> Self {
        self.overlays.push(Overlay {
            open: Box::new(open),
            placement: Some(placement),
            content: Rc::new(content),
            at: Location::caller(),
            made: super::building(),
            by: None,
        });
        self
    }
    /// While `open` holds, mounts `content` in the window's overlay layer, without an anchor.
    /// A full-window surface can use `absolute().inset_0()` for a dialog scrim. Its state lives
    /// while open and is disposed with this element; focus and dismissal come from `modal`.
    #[track_caller]
    pub fn overlay_window(mut self, open: impl Fn() -> bool + 'static, content: impl Fn() -> View + 'static) -> Self {
        self.overlays.push(Overlay {
            open: Box::new(open),
            placement: None,
            content: Rc::new(content),
            at: Location::caller(),
            made: super::building(),
            by: None,
        });
        self
    }
    /// An overlay placed by node `by` (not this element), open while this element is mounted.
    #[track_caller]
    pub(crate) fn overlay_by(mut self, by: NodeId, placement: Placement, content: impl Fn() -> View + 'static) -> Self {
        self.overlays.push(Overlay {
            open: Box::new(|| true),
            placement: Some(placement),
            content: Rc::new(content),
            at: Location::caller(),
            made: super::building(),
            by: Some(by),
        });
        self
    }

    /// Shows `image` in the element, fitted by [`object_fit`](Self::object_fit) (stretched by
    /// default) and clipped to its corners. With only a width or a height, the other side follows
    /// the image's aspect ratio; with neither, the element takes the image's size. New pixels set
    /// on the handle draw on the next frame without rerunning the view. Requires an empty box
    /// or an existing image; put the image and other content inside an external container.
    #[track_caller]
    pub fn img(mut self, image: &crate::image::Image) -> Self {
        match &mut self.content {
            Content::Box(children) if children.is_empty() => {
                self.content = Content::Image { image: image.clone(), fit: crate::image::ObjectFit::Fill };
            }
            Content::Image { image: current, .. } => *current = image.clone(),
            _ => wrong_content("img", "an empty box or an existing image"),
        }
        self
    }
    /// Display-only hint for an empty input without composition. It never supplies the
    /// accessible name or editable value and does not affect automatic height. Single-line
    /// hints truncate; multiline hints wrap within the content box. Use `.label(..)` as well.
    #[track_caller]
    pub fn placeholder(mut self, hint: impl IntoProp<String>) -> Self {
        let input = self.content.input_mut("placeholder");
        input.placeholder = Some(hint.into_prop());
        self
    }

    /// Makes a text input multi-line: the text wraps at its width and the field grows with it,
    /// Enter (and Shift+Enter) breaks the line, Cmd+Enter (Ctrl+Enter) submits; Up, Down, Page
    /// Up, Page Down, Home and End move by visual lines; a paste keeps its line breaks.
    #[track_caller]
    pub fn multiline(mut self) -> Self {
        self.content.input_mut("multiline").multiline = true;
        self
    }
    /// A multi-line text input's height: it follows its text from `min` to `max` lines and
    /// scrolls inside beyond (the caret kept in view). Makes the input multi-line.
    #[track_caller]
    pub fn rows(mut self, min: u16, max: u16) -> Self {
        let input = self.content.input_mut("rows");
        input.multiline = true;
        input.rows = Some((min, max));
        self
    }
    /// A multi-line text input as a chat composer: Enter submits, Shift+Enter (or Option+Enter)
    /// breaks the line. Makes the input multi-line.
    #[track_caller]
    pub fn submit_on_enter(mut self) -> Self {
        let input = self.content.input_mut("submit_on_enter");
        input.multiline = true;
        input.submit_on_enter = true;
        self
    }
    /// Makes a text input a secure field (a password): the value shows as bullets; copy, cut,
    /// undo and the input method are off; assistive technology, `inspect`, `check`, logs and
    /// `Debug` see bullets only. On touch platforms the last typed character shows for a moment.
    #[track_caller]
    pub fn secure(mut self) -> Self {
        self.content.input_mut("secure").secure = true;
        self
    }
    /// Shows a secure field's text (a "show password" toggle: `.revealed(move || show.get())`);
    /// its other protections stay.
    #[track_caller]
    pub fn revealed(mut self, on: impl IntoProp<bool>) -> Self {
        let input = self.content.input_mut("revealed");
        input.revealed = Some(on.into_prop());
        self
    }
    /// Which side of Octara's surface a native view is on: `Slot::Overlay` (above it, the
    /// default: web views, pickers) or `Slot::Underlay` (behind it, through a cutout, so content
    /// can draw over it: maps, video).
    #[track_caller]
    pub fn slot(mut self, slot: crate::native::Slot) -> Self {
        self.content.native_mut("slot").slot = slot;
        self
    }
    /// A native view's props: one JSON value, handed to its host whole when it changes.
    #[track_caller]
    pub fn props(mut self, props: impl IntoProp<serde_json::Value>) -> Self {
        let native = self.content.native_mut("props");
        native.props = Some(props.into_prop());
        self
    }
    /// Events from a native view (a page loaded, a map moved).
    #[track_caller]
    pub fn on_native(mut self, f: impl Fn(&crate::native::NativeEvent) + 'static) -> Self {
        self.content.native_mut("on_native").on_event = Some(Rc::new(f));
        self
    }
    /// A link in this element's rich text, or in its descendants', was pressed (a click, Enter
    /// on it, assistive technology): `f` gets its target.
    pub fn on_link(mut self, f: impl Fn(&str) + 'static) -> Self {
        self.handlers = self.handlers.on_link(f);
        self
    }
    /// Files dragged from the system (Finder, Explorer) were dropped on it or its descendants:
    /// `f` gets their paths (GPUI's `on_drop::<ExternalPaths>`).
    pub fn on_drop(mut self, f: impl Fn(&[std::path::PathBuf]) + 'static) -> Self {
        self.handlers = self.handlers.on_drop(f);
        self
    }
    /// Style while files dragged from the system are over it and it takes them (`on_drop`).
    pub fn drag_over(self, f: impl FnOnce(Patch) -> Patch) -> Self {
        self.style_when(State::DRAG_OVER, f)
    }
    /// An icon's stroke width on its grid (2 for the bundled icons; 1.5 is lighter).
    #[track_caller]
    pub fn stroke_width(mut self, width: f32) -> Self {
        match &mut self.content {
            Content::Icon { stroke_width, .. } => *stroke_width = Some(width),
            _ => wrong_content("stroke_width", "icon()"),
        }
        self
    }
    /// How the image fills the element (GPUI's `object_fit`). Call after `.img(..)`.
    #[track_caller]
    pub fn object_fit(mut self, fit: crate::image::ObjectFit) -> Self {
        match &mut self.content {
            Content::Image { fit: current, .. } => *current = fit,
            _ => wrong_content("object_fit", ".img(..) first"),
        }
        self
    }

    /// Adds a container child: an element, a view (`each(..)`, `show(..)`, a component), or a
    /// string shown as text. Atomic content rejects this call with `ELEMENT_CONTENT`.
    #[track_caller]
    pub fn child(mut self, v: impl IntoElement) -> Self {
        self.content.children_mut("child").push(v.into_view(Location::caller()));
        self
    }
    /// Adds children from any iterator: `.children(items.iter().map(|i| ..))`.
    /// Requires a container, even for an empty iterator; validates before consuming it.
    #[track_caller]
    pub fn children<C: IntoElement>(mut self, c: impl IntoIterator<Item = C>) -> Self {
        let at = Location::caller();
        self.content.children_mut("children").extend(c.into_iter().map(|v| v.into_view(at)));
        self
    }
}

// ---- building ----

fn bind<T: 'static>(at: &'static Location<'static>, node: NodeId, prop: Prop<T>, set: impl Fn(&mut Kernel, NodeId, &T) -> bool + 'static) {
    let u = crate::app::window();
    match prop {
        Prop::Static(v) => {
            set(&mut u.kernel.borrow_mut(), node, &v);
        }
        Prop::Dyn(f) => graph::binding(at, move || {
            let v = f();
            set(&mut u.kernel.borrow_mut(), node, &v);
        }),
    }
}

/// Builds a detached node for `e` with its bindings, handlers and children. The node is
/// destroyed when the current scope is disposed.
pub(crate) fn build_el(mut e: El) -> NodeId {
    let u = crate::app::window();
    let at = e.at;
    let node = u.kernel.borrow_mut().create(e.content.kind());
    u.kernel.borrow_mut().set_origin(node, at);
    u.kernel.borrow_mut().set_component(node, e.made);
    {
        let u = u.clone();
        rt(|rt| {
            rt.on_cleanup(Box::new(move || {
                u.retire_interactions(node);
                let mut k = u.kernel.borrow_mut();
                super::structure::destroy(&mut k, [node]);
            }))
        });
    }
    {
        let mut k = u.kernel.borrow_mut();
        if let Some(r) = e.role {
            k.set_role(node, r);
        }
        // Establish exactly one content kind before its bindings and layout styles.
        match &e.content {
            Content::Box(_) | Content::Text(_) => {}
            Content::Input(input) => {
                k.set_multiline(node, input.multiline);
                if let Some((min, max)) = input.rows {
                    k.set_rows(node, min, max);
                }
                k.set_submit_on_enter(node, input.submit_on_enter);
                k.set_secure(node, input.secure);
            }
            Content::Image { image, fit } => k.set_image(node, Some((image.clone(), *fit))),
            Content::Icon { icon, stroke_width } => {
                // An icon is an image when labeled, else decoration.
                match (&e.label, e.role) {
                    (Some(_), None) => {
                        k.set_role(node, Role::Image);
                    }
                    (None, None) => {
                        k.set_hidden(node, true);
                    }
                    _ => {}
                }
                k.set_icon(node, Some((icon, *stroke_width)));
            }
            Content::Canvas(_) => {
                if e.role.is_none() {
                    k.set_role(node, Role::Image);
                }
            }
            Content::Native(native) => k.set_native(node, native.kind, native.slot),
        }
        if e.hidden {
            k.set_hidden(node, true);
        }
        if e.anchor.is_some() {
            k.set_anchor(node, e.anchor);
        }
        if let Some(owner) = e.overlay_owner {
            k.set_overlay_owner(node, owner);
        }
        // Static-only elements need no retained declaration slots. Dynamic elements below
        // replace this base in declaration order; state/screen overrides remain in the kernel.
        let fixed = e.styles.iter().filter_map(|s| match s {
            StyleValue::Static(p) => Some(*p),
            StyleValue::Dyn(_) | StyleValue::Patch(_) => None,
        });
        k.set_styles(node, fixed, e.overrides);
    }
    if e.styles.iter().any(|s| !matches!(s, StyleValue::Static(_))) {
        let slots: Vec<Vec<StyleProp>> = e
            .styles
            .iter()
            .map(|s| match s {
                StyleValue::Static(p) => vec![*p],
                _ => Vec::new(),
            })
            .collect();
        let slots = Rc::new(RefCell::new(slots));
        for (index, s) in e.styles.into_iter().enumerate() {
            let evaluate: Box<dyn Fn() -> Vec<StyleProp>> = match s {
                StyleValue::Static(_) => continue,
                StyleValue::Dyn(f) => Box::new(move || vec![f()]),
                StyleValue::Patch(f) => Box::new(move || f(Patch::default()).0),
            };
            let (u, slots) = (u.clone(), slots.clone());
            graph::binding(at, move || {
                // Run user code before borrowing the cache. Only this binding's dependencies
                // are read; composing the cached values never reruns neighbouring closures.
                let props = evaluate();
                let mut slots = slots.borrow_mut();
                slots[index] = props;
                u.kernel.borrow_mut().replace_base_styles(node, slots.iter().flatten().copied());
            });
        }
    }
    if let Some(p) = e.test_id {
        bind(at, node, p, |k, n, v: &String| k.set_test_id(n, Some(v)));
    }
    let mut children = Vec::new();
    match e.content {
        Content::Text(TextContent::Prop(p)) => bind(at, node, p, |k, n, v: &Rich| k.set_rich(n, v.as_str(), v.runs())),
        Content::Text(TextContent::Buffer(t)) => {
            let cursor = t.subscribe();
            u.kernel.borrow_mut().set_text(node, &untrack(|| t.get()));
            let u = u.clone();
            graph::binding(at, move || {
                t.track();
                match t.take(&cursor) {
                    Changes::Delta(changes) => {
                        let mut k = u.kernel.borrow_mut();
                        for c in changes {
                            match c {
                                TextChange::Append(s) => k.append_text(node, &s),
                                TextChange::Splice { start, end, insert } => k.splice_text(node, start, end, &insert),
                            };
                        }
                    }
                    Changes::Reset => {
                        let snapshot = untrack(|| t.get());
                        let mut k = u.kernel.borrow_mut();
                        let current = k.text(node);
                        if snapshot.starts_with(current) {
                            // Keep selection and append-only layout progress when the current
                            // text is still an authoritative prefix after falling behind.
                            let from = current.len();
                            k.append_text(node, &snapshot[from..]);
                        } else {
                            k.set_text(node, &snapshot);
                        }
                    }
                }
            });
        }
        Content::Input(input) => {
            if let Some(p) = input.placeholder {
                bind(at, node, p, |k, n, v: &String| k.set_placeholder(n, v));
            }
            if let Some(p) = input.revealed {
                bind(at, node, p, |k, n, v: &bool| {
                    k.set_revealed(n, *v);
                    true
                });
            }
        }
        Content::Native(native) => {
            if let Some(p) = native.props {
                bind(at, node, p, |k, n, v: &serde_json::Value| k.set_native_props(n, v));
            }
            if let Some(f) = native.on_event {
                let id = u.register(Handler::Native(f));
                u.kernel.borrow_mut().set_handler(node, HandlerKind::Native, Some(HandlerId(id)));
            }
        }
        Content::Canvas(draw) => mount_canvas(at, node, draw),
        Content::Box(content) => children = content,
        Content::Image { .. } | Content::Icon { .. } => {}
    }
    if let Some(p) = e.value {
        bind(at, node, p, |k, n, v: &String| k.set_value(n, v));
    }
    if let Some(p) = e.numeric_range {
        bind(at, node, p, |k, n, v: &NumericRange| k.set_numeric_range(n, Some(*v)));
    }
    if let Some(p) = e.checked {
        bind(at, node, p, |k, n, v: &bool| k.set_checked(n, Some(*v)));
    }
    if let Some(p) = e.required {
        bind(at, node, p, |k, n, v: &bool| k.set_required(n, Some(*v)));
    }
    if let Some(p) = e.invalid {
        bind(at, node, p, |k, n, v: &bool| k.set_invalid(n, Some(*v)));
    }
    if let Some(p) = e.label {
        bind(at, node, p, |k, n, v: &String| k.set_label(n, Some(v)));
    }
    if let Some(p) = e.tooltip {
        bind(at, node, p, |k, n, v: &String| k.set_tooltip(n, Some(v)));
    }
    if let Some(p) = e.description {
        bind(at, node, p, |k, n, v: &String| k.set_description(n, Some(v)));
    }
    if let Some(p) = e.expanded {
        bind(at, node, p, |k, n, v: &bool| k.set_expanded(n, Some(*v)));
    }
    if let Some(p) = e.active_descendant {
        bind(at, node, p, |k, n, v: &Option<String>| k.set_active_descendant(n, v.as_deref()));
    }
    if let Some(p) = e.selected {
        bind(at, node, p, |k, n, v: &bool| k.set_state(n, State::SELECTED, *v));
    }
    if let Some(p) = e.disabled {
        bind(at, node, p, |k, n, v: &bool| k.set_state(n, State::DISABLED, *v));
    }
    {
        let mut k = u.kernel.borrow_mut();
        if e.focusable {
            k.set_focusable(node, true);
        }
        if e.tab_index != 0 {
            k.set_tab_index(node, e.tab_index);
        }
        if e.focus_group {
            k.set_focus_group(node, true);
        }
        if e.preserve_focus_on_press {
            k.set_preserve_focus_on_press(node, true);
        }
        if e.autofocus {
            k.set_autofocus(node, true);
        }
        if let Some((outside, _)) = &e.modal {
            k.set_modal(node, Some(*outside));
        }
        if let Some(c) = &e.key_context {
            k.set_key_context(node, Some(c));
        }
        if e.group.is_some() {
            k.set_group(node, e.group);
        }
        if e.hover_alone {
            k.set_hover_alone(node, true);
        }
        if e.region {
            k.set_region(node, true);
        }
        if let Some(l) = e.level {
            k.set_level(node, l);
        }
        if let Some(f) = e.copy_markdown {
            k.set_copy_markdown(node, Some(f));
        }
        if let Some(c) = e.cursor {
            k.set_cursor(node, c);
        }
        if e.follow_tail {
            k.set_follow_tail(node, true);
        }
        if let Some(l) = e.live {
            k.set_live(node, l);
        }
        if e.select_none {
            k.set_select_none(node, true);
        }
    }
    for (command, f) in e.commands {
        let id = u.register(Handler::Press(f));
        u.kernel.borrow_mut().add_command(node, &command, HandlerId(id));
    }
    if let Some((_, dismiss)) = e.modal {
        e.handlers.set(HandlerKind::Dismiss, Handler::Press(dismiss));
    }
    for (kind, handler) in e.handlers.compose(e.behaviors) {
        let id = u.register(handler);
        u.kernel.borrow_mut().set_handler(node, kind, Some(HandlerId(id)));
    }
    let p = Parent { node, root: Slots::default() };
    for c in children {
        mount(c, &p, &p.root);
    }
    for o in e.overlays {
        mount_overlay(node, o);
    }
    node
}

/// A canvas's drawing: a binding over the content size (kept in step after each layout) and
/// whatever `draw` reads.
fn mount_canvas(at: &'static Location<'static>, node: NodeId, draw: Draw) {
    let u = crate::app::window();
    let size = graph::Signal::detached((0.0f32, 0.0f32));
    {
        let u = u.clone();
        u.clone().after_layout(Rc::new(move || {
            let k = u.kernel.borrow();
            if !k.is_alive(node) {
                return false;
            }
            let s = k.content_size(node);
            drop(k);
            if untrack(|| size.get()) == s {
                return false;
            }
            size.set(s);
            true
        }));
    }
    rt(|rt| rt.on_cleanup(Box::new(move || size.dispose())));
    graph::binding(at, move || {
        let (w, h) = size.get();
        let mut c = crate::canvas::Canvas::new(w, h);
        if w > 0.0 && h > 0.0 {
            draw(&mut c);
        }
        u.kernel.borrow_mut().set_canvas(node, c.into_ops());
    });
}

/// Mounts an overlay's `show` into its own region of the overlay layer; the region is removed
/// with the anchor's scope.
fn mount_overlay(anchor: NodeId, o: Overlay) {
    let u = crate::app::window();
    let region = Slots::default();
    u.overlay_slots.borrow_mut().push(Slot::Region(region.clone()));
    {
        let (u, region) = (u.clone(), region.clone());
        rt(|rt| {
            rt.on_cleanup(Box::new(move || {
                u.overlay_slots.borrow_mut().retain(|s| !matches!(s, Slot::Region(r) if Rc::ptr_eq(r, &region)));
            }))
        });
    }
    let layer = Parent { node: u.kernel.borrow().overlay_layer(), root: u.overlay_slots.clone() };
    let (placement, content, at, by) = (o.placement, o.content, o.at, o.by);
    let view = super::as_part_of(o.made, || {
        show(o.open, move || {
            let mut b = boxed(content(), at);
            b.overlay_owner = Some(anchor);
            b.anchor = placement.map(|placement| (by.unwrap_or(anchor), placement));
            if placement.is_none() {
                b = b.absolute().inset_0().size_full();
            }
            b.into()
        })
    });
    mount(view, &layer, &region);
}
