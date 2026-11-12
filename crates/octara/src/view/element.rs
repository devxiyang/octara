//! Elements: constructors, properties (fixed or bound), semantics, interaction, and how an
//! element becomes a kernel node.

use super::style::{Patch, Styled};
use super::{Children, Handler, IntoElement, Parent, Slot, Slots, View, mount, show, ui};
use crate::app::{Drag, KeyEvent};

type DragHandler = Rc<dyn Fn(&Drag)>;
type KeyHandler = Rc<dyn Fn(&KeyEvent) -> bool>;
type Draw = Rc<dyn Fn(&mut crate::canvas::Canvas)>;
use crate::kernel::{
    Breakpoint, Color, Fill, HandlerId, HandlerKind, Kernel, Kind, NodeId, Paint, Placement, Role, State, StyleProp, Token, When,
};
use crate::reactive::graph::{self, rt};
use crate::reactive::text::{LiveText, TextChange};
use std::panic::Location;
use std::rc::Rc;

// ---- property values ----

/// A property value: fixed, or a closure that becomes a binding.
#[doc(hidden)]
pub enum Prop<T> {
    Static(T),
    Dyn(Box<dyn Fn() -> T>),
}

/// What a property accepts: a value, or a closure (a binding re-run when what it reads changes).
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

/// What `text` accepts: a fixed string, a closure (re-computed whole) or a [`LiveText`] (its
/// edits reach the kernel as edits: an append costs O(appended)).
#[doc(hidden)]
pub enum TextContent {
    Prop(Prop<String>),
    Live(LiveText),
}

pub trait IntoText {
    fn into_text(self) -> TextContent;
}
impl<T: IntoProp<String>> IntoText for T {
    fn into_text(self) -> TextContent {
        TextContent::Prop(self.into_prop())
    }
}
impl IntoText for LiveText {
    fn into_text(self) -> TextContent {
        TextContent::Live(self)
    }
}
impl IntoText for &LiveText {
    fn into_text(self) -> TextContent {
        TextContent::Live(self.clone())
    }
}

enum StyleValue {
    Static(StyleProp),
    Dyn(Box<dyn Fn() -> StyleProp>),
}

struct Overlay {
    open: Box<dyn Fn() -> bool>,
    placement: Placement,
    content: Rc<dyn Fn() -> View>,
    at: &'static Location<'static>,
}

// ---- elements ----

pub struct El {
    kind: Kind,
    role: Option<Role>,
    at: &'static Location<'static>,
    test_id: Option<Prop<String>>,
    text: Option<TextContent>,
    value: Option<Prop<String>>,
    checked: Option<Prop<bool>>,
    label: Option<Prop<String>>,
    description: Option<Prop<String>>,
    expanded: Option<Prop<bool>>,
    selected: Option<Prop<bool>>,
    disabled: Option<Prop<bool>>,
    hidden: bool,
    focusable: bool,
    styles: Vec<StyleValue>,
    overrides: Vec<(When, Vec<StyleProp>)>,
    press: Option<Rc<dyn Fn()>>,
    input: Option<Rc<dyn Fn(String)>>,
    toggle: Option<Rc<dyn Fn()>>,
    double_press: Option<Rc<dyn Fn()>>,
    secondary_press: Option<Rc<dyn Fn()>>,
    long_press: Option<Rc<dyn Fn()>>,
    drag: Option<DragHandler>,
    submit: Option<Rc<dyn Fn()>>,
    tab_index: i32,
    focus_group: bool,
    autofocus: bool,
    modal: Option<(bool, Rc<dyn Fn()>)>,
    key: Option<KeyHandler>,
    key_context: Option<String>,
    commands: Vec<(String, Rc<dyn Fn()>)>,
    image: Option<crate::image::Image>,
    object_fit: crate::image::ObjectFit,
    icon: Option<crate::icon::Icon>,
    stroke_width: Option<f32>,
    canvas: Option<Draw>,
    multiline: bool,
    group: Option<&'static str>,
    hover_alone: bool,
    overlays: Vec<Overlay>,
    anchor: Option<(NodeId, Placement)>,
    pub(crate) children: Vec<View>,
}

impl From<El> for View {
    fn from(e: El) -> View {
        View::El(Box::new(e))
    }
}

fn new_el(kind: Kind, at: &'static Location<'static>) -> El {
    El {
        kind,
        role: None,
        at,
        test_id: None,
        text: None,
        value: None,
        checked: None,
        label: None,
        description: None,
        expanded: None,
        selected: None,
        disabled: None,
        hidden: false,
        focusable: false,
        styles: Vec::new(),
        overrides: Vec::new(),
        press: None,
        input: None,
        toggle: None,
        double_press: None,
        secondary_press: None,
        long_press: None,
        drag: None,
        submit: None,
        tab_index: 0,
        focus_group: false,
        autofocus: false,
        modal: None,
        key: None,
        key_context: None,
        commands: Vec::new(),
        image: None,
        object_fit: crate::image::ObjectFit::Fill,
        icon: None,
        stroke_width: None,
        canvas: None,
        multiline: false,
        group: None,
        hover_alone: false,
        overlays: Vec::new(),
        anchor: None,
        children: Vec::new(),
    }
}

fn boxed(children: impl Children, at: &'static Location<'static>) -> El {
    let mut e = new_el(Kind::Box, at);
    e.children = children.into_views();
    e
}

/// A box: a flex container laying children out in a row until told otherwise (Rune's `el()`).
/// Add children with `.child(..)` / `.children(..)`.
#[track_caller]
pub fn el() -> El {
    new_el(Kind::Box, Location::caller())
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

/// Text: `text("fixed")`, `text(move || format!(..))`, or `text(live)` for a [`LiveText`].
#[track_caller]
pub fn text(t: impl IntoText) -> El {
    text_at(t, Location::caller())
}

pub(crate) fn text_at(t: impl IntoText, at: &'static Location<'static>) -> El {
    let mut e = new_el(Kind::Text, at);
    e.text = Some(t.into_text());
    e
}

/// An icon (`icon(icons::SEARCH)`, from `octara-icons`, or an [`Icon`](crate::icon::Icon) of
/// your own): a square, 16 px by default (`size_5`, `size_6` to change), drawn in the text color,
/// so it follows its button's color and states. Decorative (hidden from assistive technology)
/// unless it has a `label`, which makes it an image.
#[track_caller]
pub fn icon(icon: crate::icon::Icon) -> El {
    let mut e = new_el(Kind::Box, Location::caller()).size_4();
    e.icon = Some(icon);
    e
}

/// A canvas: `draw` records shapes, images, transforms and clips into a
/// [`Canvas`](crate::canvas::Canvas) sized to the element's content box. It reruns only when what
/// it reads (signals, the element's size) changes. A canvas has no size of its own: size it like
/// a box (`w`, `h`, `flex_1`, `size_full`). It is an image to assistive technology: give it a
/// `label`, or `a11y_hidden()` when it is decoration. See [`crate::canvas`].
#[track_caller]
pub fn canvas(draw: impl Fn(&mut crate::canvas::Canvas) + 'static) -> El {
    let mut e = new_el(Kind::Box, Location::caller());
    e.canvas = Some(Rc::new(draw));
    e
}

/// A text input. Text input needs the platform's input protocol (IME), so it is a core
/// primitive; `octara-kit`'s `TextField` builds on it.
#[track_caller]
pub fn text_input() -> El {
    new_el(Kind::TextInput, Location::caller()).focusable()
}

impl Styled for El {
    fn style(mut self, p: StyleProp) -> Self {
        self.styles.push(StyleValue::Static(p));
        self
    }
}

impl El {
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
    pub fn description(mut self, d: impl IntoProp<String>) -> Self {
        self.description = Some(d.into_prop());
        self
    }
    pub fn expanded(mut self, e: impl IntoProp<bool>) -> Self {
        self.expanded = Some(e.into_prop());
        self
    }
    pub fn checked(mut self, c: impl IntoProp<bool>) -> Self {
        self.checked = Some(c.into_prop());
        self
    }
    pub fn value(mut self, v: impl IntoProp<String>) -> Self {
        self.value = Some(v.into_prop());
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
    /// focused or selected one). Menus, lists and trees move on Up and Down; other groups (tabs,
    /// toolbars, radio groups) on Left and Right too. Home and End go to the ends.
    pub fn focus_group(mut self) -> Self {
        self.focus_group = true;
        self
    }
    /// Keys while focus is on this element or inside it, before the keymap: return `true` to
    /// consume the key. Innermost handlers run first.
    pub fn on_key(mut self, f: impl Fn(&KeyEvent) -> bool + 'static) -> Self {
        self.key = Some(Rc::new(f));
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
        self.press = Some(Rc::new(f));
        self
    }
    /// A text field's submission: Enter while it has focus (and no input method is composing).
    /// Without one, Enter presses the field like any focused element.
    pub fn on_submit(mut self, f: impl Fn() + 'static) -> Self {
        self.submit = Some(Rc::new(f));
        self
    }
    /// The second press of a double click or double tap (after its `on_press`).
    pub fn on_double_press(mut self, f: impl Fn() + 'static) -> Self {
        self.double_press = Some(Rc::new(f));
        self
    }
    /// A context menu request: a right click (Control-click on macOS), a long press on touch
    /// (without `on_long_press`), or assistive technology's "show menu". The nearest element
    /// with one, from the pointer up, gets it.
    pub fn on_secondary_press(mut self, f: impl Fn() + 'static) -> Self {
        self.secondary_press = Some(Rc::new(f));
        self
    }
    /// A touch held still for the long-press duration (500 ms by default); the release then
    /// presses nothing.
    pub fn on_long_press(mut self, f: impl Fn() + 'static) -> Self {
        self.long_press = Some(Rc::new(f));
        self
    }
    /// A drag starting on the element: `Start` once the pointer moves past the slop (4 pt with a
    /// mouse, 8 pt with touch), then `Move`, then `End` (or `Cancel`). A drag is not a press.
    pub fn on_drag(mut self, f: impl Fn(&Drag) + 'static) -> Self {
        self.drag = Some(Rc::new(f));
        self
    }
    pub fn on_input(mut self, f: impl Fn(String) + 'static) -> Self {
        self.input = Some(Rc::new(f));
        self
    }
    pub fn on_toggle(mut self, f: impl Fn() + 'static) -> Self {
        self.toggle = Some(Rc::new(f));
        self
    }
    /// While `open` holds, shows `content` above everything else, placed next to this element
    /// (flipped and clamped to stay in the window). Its state lives while it is open.
    #[track_caller]
    pub fn overlay(mut self, open: impl Fn() -> bool + 'static, placement: Placement, content: impl Fn() -> View + 'static) -> Self {
        self.overlays.push(Overlay { open: Box::new(open), placement, content: Rc::new(content), at: Location::caller() });
        self
    }

    /// Shows `image` in the element, fitted by [`object_fit`](Self::object_fit) (stretched by
    /// default) and clipped to its corners. With only a width or a height, the other side follows
    /// the image's aspect ratio; with neither, the element takes the image's size. New pixels set
    /// on the handle draw on the next frame without rerunning the view.
    pub fn img(mut self, image: &crate::image::Image) -> Self {
        self.image = Some(image.clone());
        self
    }
    /// Makes a text input multi-line: the text wraps at its width and the field grows with it,
    /// Enter (and Shift+Enter) breaks the line, Cmd+Enter (Ctrl+Enter) submits; Up, Down, Page
    /// Up, Page Down, Home and End move by visual lines; a paste keeps its line breaks.
    pub fn multiline(mut self) -> Self {
        self.multiline = true;
        self
    }
    /// An icon's stroke width on its grid (2 for the bundled icons; 1.5 is lighter).
    pub fn stroke_width(mut self, width: f32) -> Self {
        self.stroke_width = Some(width);
        self
    }
    /// How the image fills the element (GPUI's `object_fit`).
    pub fn object_fit(mut self, fit: crate::image::ObjectFit) -> Self {
        self.object_fit = fit;
        self
    }

    /// Adds a child: an element, a view (`each(..)`, `show(..)`, a component), or a string,
    /// shown as text (as in GPUI and Rune).
    #[track_caller]
    pub fn child(mut self, v: impl IntoElement) -> Self {
        let v = v.into_view(Location::caller());
        self.children.push(v);
        self
    }
    /// Adds children from any iterator: `.children(items.iter().map(|i| ..))`.
    #[track_caller]
    pub fn children<C: IntoElement>(mut self, c: impl IntoIterator<Item = C>) -> Self {
        let at = Location::caller();
        self.children.extend(c.into_iter().map(|v| v.into_view(at)));
        self
    }
}

// ---- building ----

fn bind<T: 'static>(at: &'static Location<'static>, node: NodeId, prop: Prop<T>, set: impl Fn(&mut Kernel, NodeId, &T) -> bool + 'static) {
    let u = ui();
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
pub(crate) fn build_el(e: El) -> NodeId {
    let u = ui();
    let at = e.at;
    let node = u.kernel.borrow_mut().create(e.kind);
    u.kernel.borrow_mut().set_origin(node, at);
    {
        let u = u.clone();
        rt(|rt| {
            rt.on_cleanup(Box::new(move || {
                let mut k = u.kernel.borrow_mut();
                if k.is_alive(node) {
                    k.destroy(node);
                }
            }))
        });
    }
    {
        let mut k = u.kernel.borrow_mut();
        if let Some(r) = e.role {
            k.set_role(node, r);
        }
        // A canvas is an image.
        if e.canvas.is_some() && e.role.is_none() {
            k.set_role(node, Role::Image);
        }
        // An icon is an image when labeled, else decoration.
        if let Some(i) = &e.icon {
            match (&e.label, e.role) {
                (Some(_), None) => {
                    k.set_role(node, Role::Image);
                }
                (None, None) => {
                    k.set_hidden(node, true);
                }
                _ => {}
            }
            k.set_icon(node, Some((i, e.stroke_width)));
        }
        if e.hidden {
            k.set_hidden(node, true);
        }
        if e.anchor.is_some() {
            k.set_anchor(node, e.anchor);
        }
        // Fixed styles and overrides resolve once; bound styles follow (and win over fixed ones).
        let fixed = e.styles.iter().filter_map(|s| match s {
            StyleValue::Static(p) => Some(*p),
            StyleValue::Dyn(_) => None,
        });
        k.set_styles(node, fixed, e.overrides);
    }
    for s in e.styles {
        match s {
            StyleValue::Static(_) => {}
            StyleValue::Dyn(f) => {
                let u = u.clone();
                graph::binding(at, move || {
                    let p = f();
                    u.kernel.borrow_mut().set_style(node, p);
                })
            }
        }
    }
    if let Some(p) = e.test_id {
        bind(at, node, p, |k, n, v: &String| k.set_test_id(n, Some(v)));
    }
    match e.text {
        Some(TextContent::Prop(p)) => bind(at, node, p, |k, n, v: &String| k.set_text(n, v)),
        Some(TextContent::Live(t)) => {
            let cursor = t.subscribe();
            u.kernel.borrow_mut().set_text(node, &t.get_untracked());
            let u = u.clone();
            graph::binding(at, move || {
                t.track();
                for c in t.take(&cursor) {
                    let mut k = u.kernel.borrow_mut();
                    match c {
                        TextChange::Append(s) => k.append_text(node, &s),
                        TextChange::Splice { start, end, insert } => k.splice_text(node, start, end, &insert),
                    };
                }
            });
        }
        None => {}
    }
    if let Some(p) = e.value {
        bind(at, node, p, |k, n, v: &String| k.set_value(n, v));
    }
    if let Some(p) = e.checked {
        bind(at, node, p, |k, n, v: &bool| k.set_checked(n, Some(*v)));
    }
    if let Some(p) = e.label {
        bind(at, node, p, |k, n, v: &String| k.set_label(n, Some(v)));
    }
    if let Some(p) = e.description {
        bind(at, node, p, |k, n, v: &String| k.set_description(n, Some(v)));
    }
    if let Some(p) = e.expanded {
        bind(at, node, p, |k, n, v: &bool| k.set_expanded(n, Some(*v)));
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
        if e.autofocus {
            k.set_autofocus(node, true);
        }
        if let Some((outside, _)) = &e.modal {
            k.set_modal(node, Some(*outside));
        }
        if let Some(c) = &e.key_context {
            k.set_key_context(node, Some(c));
        }
        if e.multiline {
            k.set_multiline(node, true);
        }
        if let Some(img) = e.image {
            k.set_image(node, Some((img, e.object_fit)));
        }
        if e.group.is_some() {
            k.set_group(node, e.group);
        }
        if e.hover_alone {
            k.set_hover_alone(node, true);
        }
    }
    if let Some(draw) = e.canvas {
        mount_canvas(at, node, draw);
    }
    for (command, f) in e.commands {
        let id = u.register(Handler::Press(f));
        u.kernel.borrow_mut().add_command(node, &command, HandlerId(id));
    }
    for (kind, h) in [
        (HandlerKind::Press, e.press.map(Handler::Press)),
        (HandlerKind::Input, e.input.map(Handler::Input)),
        (HandlerKind::Toggle, e.toggle.map(Handler::Toggle)),
        (HandlerKind::DoublePress, e.double_press.map(Handler::Press)),
        (HandlerKind::SecondaryPress, e.secondary_press.map(Handler::Press)),
        (HandlerKind::LongPress, e.long_press.map(Handler::Press)),
        (HandlerKind::Drag, e.drag.map(Handler::Drag)),
        (HandlerKind::Submit, e.submit.map(Handler::Press)),
        (HandlerKind::Dismiss, e.modal.map(|(_, f)| Handler::Press(f))),
        (HandlerKind::Key, e.key.map(Handler::Key)),
    ] {
        if let Some(h) = h {
            let id = u.register(h);
            u.kernel.borrow_mut().set_handler(node, kind, Some(HandlerId(id)));
        }
    }
    let p = Parent { node, root: Slots::default() };
    for c in e.children {
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
    let u = ui();
    let size = graph::Signal::detached((0.0f32, 0.0f32));
    let hook = {
        let u = u.clone();
        u.clone().after_layout(Rc::new(move || {
            let k = u.kernel.borrow();
            if !k.is_alive(node) {
                return false;
            }
            let s = k.content_size(node);
            drop(k);
            if size.get_untracked() == s {
                return false;
            }
            size.set(s);
            true
        }))
    };
    {
        let u = u.clone();
        rt(|rt| {
            rt.on_cleanup(Box::new(move || {
                u.remove_after_layout(hook);
                size.dispose();
            }))
        });
    }
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
    let u = ui();
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
    let (placement, content, at) = (o.placement, o.content, o.at);
    let view = show(o.open, move || {
        let mut b = boxed(content(), at);
        b.anchor = Some((anchor, placement));
        b.into()
    });
    mount(view, &layer, &region);
}
