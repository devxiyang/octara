//! The retained node tree.
//!
//! Nodes live in a `SlotMap` (generational keys). Each node has a [`Kind`] (which decides how
//! it is measured), a semantic role (`accesskit::Role`), content, semantics and style. Style is a
//! list of base [`StyleProp`]s plus per-[`State`] overrides; the kernel resolves the effective
//! layout (`taffy::Style`) and [`Visual`] whenever a property, a state or the [`Theme`] changes,
//! and marks only the [`Dirty`] categories that actually changed. Colors are [`Paint`]: a theme
//! [`Token`] or an explicit color, so switching themes needs no bindings.
//!
//! Both the semantic dump and the test-id index cover attached nodes only (reachable from root).

pub mod a11y;
mod anim;
mod edit;
mod focus;
mod issues;
mod paint;
mod pick;
pub mod rich;
pub mod select;
pub mod style;
pub mod text;
pub mod theme;

use rustc_hash::FxHashMap;
use slotmap::{SlotMap, new_key_type};
use std::cell::{Cell, RefCell};
use std::fmt::Write as _;
use std::rc::Rc;
use style::{Inherited, Unresolved};
use text::{TextEngine, TextMetrics, TextStyle, Wrap};

pub use accesskit::Role;
pub use edit::floor_boundary;
pub use focus::Arrow;
pub use issues::LayoutIssue;
pub use select::Address;
pub use style::{BoxShadow, Dirty, FontSize, GridPos, Length, LineHeight, Motion, Radius, Shadows, State, StyleProp, Visual, When};
pub use taffy::{AlignItems, Dimension, Display, FlexDirection, FlexWrap, JustifyContent, LengthPercentageAuto, Overflow, Position};
pub use theme::{
    Breakpoint, Color, ColorStop, ContainerStep, Family, Fill, LinearGradient, MISSING, Paint, RadiusStep, ShadowStep, TextStep, Theme,
    Token, contrast, linear_color_stop, linear_gradient, rgb,
};

new_key_type! {
    /// A node handle; a destroyed node's handle never aliases a new node.
    pub struct NodeId;
}

fn hex(c: Color) -> String {
    let c = c.to_rgba8();
    if c.a == 255 { format!("#{:02x}{:02x}{:02x}", c.r, c.g, c.b) } else { format!("#{:02x}{:02x}{:02x}{:02x}", c.r, c.g, c.b, c.a) }
}

/// Which side of its anchor an overlay opens on.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Side {
    Top,
    Bottom,
    Left,
    Right,
}

impl Side {
    fn opposite(self) -> Side {
        match self {
            Side::Top => Side::Bottom,
            Side::Bottom => Side::Top,
            Side::Left => Side::Right,
            Side::Right => Side::Left,
        }
    }
}

/// How an overlay lines up with its anchor along the other axis.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Align {
    Start,
    Center,
    End,
}

/// Where an overlay goes relative to its anchor (as Rune's `anchored`). It opens on `side`,
/// flips to the opposite one when it does not fit there, and stays `margin` inside the window.
/// With `fit`, it is also limited to the room on its side and scrolls the rest (Rune's
/// `anchored_fit`, Floating UI's `size`), instead of being pushed over its anchor.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Placement {
    pub side: Side,
    pub align: Align,
    /// Distance from the anchor.
    pub gap: f32,
    /// Minimum distance from the window's edges (8, as in Rune).
    pub margin: f32,
    /// Limit the size to the room available and scroll the content.
    pub fit: bool,
}

/// Where the last layout put an overlay.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Placed {
    pub x: f32,
    pub y: f32,
    pub side: Side,
    /// The size limit along the side's axis (with `fit`).
    pub limit: Option<f32>,
    /// It fits neither side and was pushed into the window (without `fit`).
    pub clamped: bool,
}

impl Placement {
    /// Below the anchor, left edges aligned, limited to the room below or above (menus, selects).
    pub const BELOW: Placement = Placement { side: Side::Bottom, align: Align::Start, gap: 4.0, margin: 8.0, fit: true };
    /// Above the anchor, centered (tooltips).
    pub const ABOVE: Placement = Placement { side: Side::Top, align: Align::Center, gap: 4.0, margin: 8.0, fit: false };
    /// To the right, top edges aligned (submenus).
    pub const RIGHT: Placement = Placement { side: Side::Right, align: Align::Start, gap: 0.0, margin: 8.0, fit: true };

    pub const fn side(self, side: Side) -> Self {
        Placement { side, ..self }
    }
    pub const fn align(self, align: Align) -> Self {
        Placement { align, ..self }
    }
    pub const fn gap(self, gap: f32) -> Self {
        Placement { gap, ..self }
    }
    pub const fn margin(self, margin: f32) -> Self {
        Placement { margin, ..self }
    }
    pub const fn fit(self, fit: bool) -> Self {
        Placement { fit, ..self }
    }

    /// Room for the overlay on `side` of anchor `a` in viewport `vp`, and the overlay's extent
    /// along that axis.
    fn room(self, side: Side, a: (f32, f32, f32, f32), size: (f32, f32), vp: (f32, f32)) -> (f32, f32) {
        let (ax, ay, aw, ah) = a;
        let m = self.margin;
        match side {
            Side::Bottom => (vp.1 - m - (ay + ah + self.gap), size.1),
            Side::Top => (ay - self.gap - m, size.1),
            Side::Right => (vp.0 - m - (ax + aw + self.gap), size.0),
            Side::Left => (ax - self.gap - m, size.0),
        }
    }

    /// Places an overlay of natural `size` next to anchor rect `a` in viewport `vp`.
    pub fn place(self, a: (f32, f32, f32, f32), size: (f32, f32), vp: (f32, f32)) -> Placed {
        let (ax, ay, aw, ah) = a;
        let opp = self.side.opposite();
        let (room, extent) = self.room(self.side, a, size, vp);
        let (room_opp, _) = self.room(opp, a, size, vp);
        let fits = |r: f32| r >= extent;
        // The requested side if it fits, else the opposite if that fits, else the roomier one.
        let side = if fits(room) || (!fits(room_opp) && room >= room_opp) { self.side } else { opp };
        let room_side = if side == self.side { room } else { room_opp };
        let neither = !fits(room) && !fits(room_opp);
        let limit = (self.fit && neither).then_some(room_side.max(0.0));
        let (mut w, mut h) = size;
        if let Some(l) = limit {
            match side {
                Side::Top | Side::Bottom => h = l,
                Side::Left | Side::Right => w = l,
            }
        }
        let main = match side {
            Side::Bottom => ay + ah + self.gap,
            Side::Top => ay - self.gap - h,
            Side::Right => ax + aw + self.gap,
            Side::Left => ax - self.gap - w,
        };
        let m = self.margin;
        let clamp = |v: f32, len: f32, max: f32| v.min(max - m - len).max(m);
        let cross = |start: f32, alen: f32, len: f32| match self.align {
            Align::Start => start,
            Align::Center => start + (alen - len) / 2.0,
            Align::End => start + alen - len,
        };
        let (x, y) = match side {
            Side::Top | Side::Bottom => (clamp(cross(ax, aw, w), w, vp.0), clamp(main, h, vp.1)),
            Side::Left | Side::Right => (clamp(main, w, vp.0), clamp(cross(ay, ah, h), h, vp.1)),
        };
        Placed { x, y, side, limit, clamped: neither && !self.fit }
    }
}

// ---------------------------------------------------------------------------------------------
// Nodes
// ---------------------------------------------------------------------------------------------

/// What a node is, which decides how it is measured. Its semantic role is separate.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Root,
    /// A box laid out by its children (and its explicit size).
    Box,
    /// Text measured from its content and font size.
    Text,
    /// A text input (a fixed default size; the platform owns editing).
    TextInput,
    /// A link inside a text: a range of its parent text's runs, not laid out on its own (its
    /// frame is the lines the range covers).
    Link,
}

impl Kind {
    fn default_role(self) -> Role {
        match self {
            Kind::Root => Role::Window,
            Kind::Box => Role::GenericContainer,
            Kind::Text => Role::Label,
            Kind::TextInput => Role::TextInput,
            Kind::Link => Role::Link,
        }
    }
}

/// An icon as a node draws it.
#[derive(Clone, PartialEq, Debug)]
pub struct NodeIcon {
    pub name: String,
    pub view_box: crate::paint::Rect,
    pub shapes: Vec<crate::icon::IconShape>,
}

#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub enum HandlerKind {
    Press,
    Input,
    Toggle,
    DoublePress,
    SecondaryPress,
    LongPress,
    Drag,
    /// A modal's dismissal (Escape, a press outside).
    Dismiss,
    /// Keys while focus is on the element or inside it (`on_key`).
    Key,
    /// A text field's submission (Enter).
    Submit,
    /// An event from a native view (`on_native`).
    Native,
    /// A link inside the element's texts was pressed (`on_link`).
    Link,
    /// Files dragged from the system were dropped on it (`on_drop`).
    Drop,
    /// It got keyboard focus (`on_focus`).
    Focus,
    /// It lost keyboard focus (`on_blur`).
    Blur,
}

impl HandlerKind {
    pub const COUNT: usize = 15;
    /// Handlers that make an element a pointer target.
    pub const POINTER: [HandlerKind; 6] = [
        HandlerKind::Press,
        HandlerKind::Toggle,
        HandlerKind::DoublePress,
        HandlerKind::SecondaryPress,
        HandlerKind::LongPress,
        HandlerKind::Drag,
    ];
}

/// Opaque id of a handler closure owned by the view layer.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub struct HandlerId(pub u64);

#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct Counters {
    pub nodes_created: u64,
    pub nodes_destroyed: u64,
    /// Property writes that changed something (content, semantics or effective style).
    pub prop_changes: u64,
    /// Property writes that changed nothing.
    pub prop_noops: u64,
    pub dirty_layout: u64,
    pub dirty_paint: u64,
    pub dirty_text: u64,
    pub dirty_semantics: u64,
    pub structural_ops: u64,
    pub handler_sets: u64,
    pub layout_runs: u64,
    /// Bytes of text written: the whole text for `set_text`, only the new part for edits.
    pub text_bytes: u64,
}

#[derive(Clone, Debug, PartialEq)]
pub enum OpKind {
    Create(Kind),
    Destroy,
    Insert { parent: NodeId, index: usize },
    Remove { parent: NodeId },
    Move { index: usize },
    Set { prop: &'static str, value: String },
}

/// One kernel mutation, attributed to the cause (event id) set when it ran.
#[derive(Clone, Debug)]
pub struct Op {
    pub cause: Option<u64>,
    pub node: NodeId,
    pub test_id: Option<String>,
    pub kind: OpKind,
}

#[derive(Clone, Copy, Debug)]
enum Measure {
    Text(NodeId),
    Input(NodeId),
    Native(NodeId),
}

struct Node {
    /// What the node shows inside its box; its `Kind` follows from it.
    content: Content,
    /// How far the node's drawing, its subtree's included where nothing clips it, reaches beyond
    /// its box (left, top, right, bottom): shadows, overflowing text and children. Painting and
    /// hit testing skip subtrees whose reach misses the clip.
    ink: Cell<[f32; 4]>,
    /// Its own drawing as last built, in its own coordinates, and what it was built from.
    drawn: RefCell<Option<paint::Drawn>>,
    /// Counts changes to what it draws (its own drawing is built again after one).
    paint_ver: u64,
    /// The place last sent to assistive technology (`None`: not sent, or dropped since).
    a11y_sent: Cell<Option<[f32; 6]>>,
    role: Option<Role>,
    parent: Option<NodeId>,
    children: Vec<NodeId>,
    /// The value assistive technology announces for a node that is not a text input (a
    /// combo box's choice, a slider's level); a text input's value is its content.
    value: Option<String>,
    checked: Option<bool>,
    label: Option<String>,
    description: Option<String>,
    /// What shows when the mouse rests on it (`tooltip`); its description where it has none.
    tooltip: Option<String>,
    expanded: Option<bool>,
    hidden: bool,
    focusable: bool,
    /// Tab order: positive values come first (in order), 0 in tree order, negative not at all.
    tab_index: i32,
    /// Arrow keys move focus among the focusable elements inside; Tab stops once.
    focus_group: bool,
    /// A focus group's member focused last (where Tab comes back to).
    group_current: Option<NodeId>,
    /// Focused when the modal around it opens.
    autofocus: bool,
    /// A modal: `Some(click_outside)` (a press outside dismisses it).
    modal: Option<bool>,
    /// The key context this element adds for the bindings of its subtree (`key_context`).
    key_context: Option<String>,
    /// Command handlers (`on_command`), by command id.
    commands: Vec<(String, HandlerId)>,
    /// Properties moving toward their resolved values (`motion`).
    anim: Option<Box<anim::Anim>>,
    /// The layout pass the node was created in: changes before the next layout (its bindings'
    /// first values) do not animate.
    born: u64,
    /// Leaving: out of the tree, fading out, destroyed when it has (an exit's root).
    exiting: bool,
    /// A hover group's name (`group`): descendants' `group_hover` styles follow its hover.
    group: Option<&'static str>,
    /// While hovered, the elements around it are not (`hover_alone`).
    hover_alone: bool,
    /// Its texts can be selected together, as one document (`.selectable()`).
    region: bool,
    /// Its texts are never selected (`.select_none()`).
    select_none: bool,
    /// A list row's key (`each`, `virtual_each`): selections address rows by it.
    row_key: Option<std::rc::Rc<str>>,
    /// The range of its text a document selection covers now.
    selected: Option<std::ops::Range<usize>>,
    /// A region's selection as assistive technology gets it: anchor and focus, each a mounted
    /// text and a byte offset in it.
    doc_selection: Option<[(NodeId, usize); 2]>,
    /// Copies the selected part of it as Markdown (`copy_markdown`).
    copy_markdown: Option<select::CopyMarkdown>,
    /// How the pointer looks over it (else as its kind and ancestors say).
    cursor: Option<crate::platform::CursorStyle>,
    /// Its level (a heading's, a list item's nesting), for assistive technology.
    level: Option<u8>,
    /// How assistive technology announces its changes (a live region).
    live: Option<accesskit::Live>,
    /// An overlay's anchor node and placement; the node is positioned absolutely in the overlay layer.
    anchor: Option<(NodeId, Placement)>,
    /// Where the last layout put an anchored node.
    placed: Option<Placed>,
    /// Scroll offset of a scroll container.
    scroll: (f32, f32),
    /// A scroll container keeping the end of its content in view (`follow_tail`): whether it
    /// follows now (`None`: it does not follow at all).
    follow: Option<bool>,
    /// Text properties passed down to children.
    inherited: Inherited,
    /// What the parent passed down when this node was last resolved.
    inherited_from: Inherited,
    /// Where the element was created (`file:line`), for diagnostics.
    origin: Option<&'static std::panic::Location<'static>>,
    /// The component function that built it.
    component: Option<Component>,
    test_id: Option<String>,
    handlers: [Option<HandlerId>; HandlerKind::COUNT],
    taffy: taffy::NodeId,
    attached: bool,
    base: Vec<StyleProp>,
    overrides: Vec<(When, Vec<StyleProp>)>,
    state: State,
    layout: taffy::Style,
    visual: Visual,
}

/// The component function that built an element: a `#[component]` function's name and module,
/// and where it was called.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Component {
    pub name: &'static str,
    pub module: &'static str,
    pub at: &'static std::panic::Location<'static>,
}

/// A text input's default width (CSS gives `<input>` about 20 characters).
/// The last display list built at rest: the paint count and viewport it was built at.
type LastFrame = (u64, (f32, f32), std::rc::Rc<crate::paint::DisplayList>);

/// What a node shows inside its box: at most one kind of content.
#[derive(Debug)]
enum Content {
    /// A plain box (or the root).
    None,
    Text(Box<TextContent>),
    Input(Box<InputContent>),
    Image(crate::image::Image, crate::image::ObjectFit),
    Icon(std::rc::Rc<NodeIcon>),
    Canvas(std::rc::Rc<Vec<crate::canvas::CanvasOp>>),
    Native(Box<NativeContent>),
    /// A link: its range of the parent text and its target.
    Link(Box<rich::LinkContent>),
}

/// A native view's element: its kind, slot, props (and their version, so the host hears of a
/// change once) and the size the view reported.
#[derive(Debug)]
pub struct NativeContent {
    pub kind: String,
    pub slot: crate::native::Slot,
    pub props: serde_json::Value,
    pub version: u64,
    pub intrinsic: Option<(f32, f32)>,
    /// What the view last showed, from its host: drawn in its place instead of the placeholder
    /// (captures and screenshots).
    pub snapshot: Option<crate::image::Image>,
    /// The platform shows it (its host said so): no placeholder; an underlay view gets a
    /// cutout.
    pub live: bool,
}

/// A native view's size until it reports one (HTML's for an iframe).
const NATIVE_DEFAULT: (f32, f32) = (300.0, 150.0);

impl std::fmt::Debug for TextContent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TextContent")
            .field("text", &self.text)
            .field("runs", &self.runs)
            .field("version", &self.version)
            .finish_non_exhaustive()
    }
}

/// A text's content.
#[derive(Default)]
struct TextContent {
    text: String,
    /// Bytes known unchanged since the text engine last measured them.
    clean: Cell<usize>,
    /// Styled ranges (rich text), sorted; plain text has none.
    runs: Vec<(std::ops::Range<usize>, rich::Span)>,
    /// The runs as the text engine shapes them (families resolved by the theme).
    shape: Vec<text::StyleRun>,
    /// Bumped by every change of the text or its runs: what kept layouts and glyphs are keyed by.
    version: u64,
    /// The glyphs painted last, for the same version, width and style.
    painted: RefCell<Option<rich::Painted>>,
    /// The lines laid out last, for the same version, width and style (links, selection).
    lines: RefCell<Option<rich::Lines>>,
    /// Its layout (see `node_layout`): the one place its shaped paragraphs are held.
    layout: RefCell<Option<(LayoutKey, text::TextLayout)>>,
}

/// A text input's content: its value, the input method's composition, the selection and how
/// far the line is scrolled. A secure field's value never shows in `Debug` and is zeroed when
/// it is replaced or dropped.
#[derive(Default)]
struct InputContent {
    value: String,
    preedit: String,
    selection: (usize, usize),
    /// How far the text is scrolled: x for a single line, y for a multi-line field.
    scroll: Cell<f32>,
    /// Bytes of the shown text (value with composition) known unchanged since last measured.
    clean: Cell<usize>,
    /// The layout of what it shows last (its value, a composition spliced in), of what, in
    /// what, with which fonts.
    layout: RefCell<Option<(String, TextStyle, u64, text::TextLayout)>>,
    /// Wraps at the field's width; Enter breaks lines.
    multiline: bool,
    /// The caret's x kept across moves by lines (Up, Down, Page Up, Page Down).
    goal: Cell<Option<f32>>,
    /// Lines shown (multi-line): the height follows the text between these.
    rows: (u16, u16),
    /// Enter submits and Shift+Enter breaks lines (a chat composer), instead of the reverse.
    submit_on_enter: bool,
    /// The caret (and text length) the scroll last kept in view: the text scrolls back to the
    /// caret only when it moves, not after a wheel moved the text away from it.
    kept: Cell<Option<(usize, usize)>>,
    /// A password: shown masked, kept out of copies, history, input methods and every report.
    secure: bool,
    /// A secure field showing its text (a "show password" toggle); the rest still applies.
    revealed: bool,
    /// The last typed grapheme of a secure field shown until a time (touch platforms): its
    /// value byte offset and the kernel time it hides at.
    reveal: Cell<Option<(usize, f64)>>,
}

impl InputContent {
    /// Shown as bullets.
    fn masked(&self) -> bool {
        self.secure && !self.revealed
    }
}

/// Overwrites a string's bytes before letting them go (a secure field's value).
fn wipe(s: &mut String) {
    // SAFETY: zero bytes are valid UTF-8, and the string is cleared right after.
    for b in unsafe { s.as_bytes_mut() } {
        unsafe { std::ptr::write_volatile(b, 0) };
    }
    s.clear();
}

impl Drop for InputContent {
    fn drop(&mut self) {
        if self.secure {
            wipe(&mut self.value);
            wipe(&mut self.preedit);
        }
    }
}

impl std::fmt::Debug for InputContent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut d = f.debug_struct("InputContent");
        if self.secure {
            d.field("value", &"[redacted]");
        } else {
            d.field("value", &self.value).field("preedit", &self.preedit).field("selection", &self.selection);
        }
        d.field("multiline", &self.multiline).field("secure", &self.secure).finish_non_exhaustive()
    }
}

impl Node {
    fn kind(&self) -> Kind {
        match self.content {
            Content::Text(_) => Kind::Text,
            Content::Input(_) => Kind::TextInput,
            Content::Link(_) => Kind::Link,
            _ => Kind::Box,
        }
    }
    /// A text's own text (empty for other nodes).
    fn text(&self) -> &str {
        match &self.content {
            Content::Text(t) => &t.text,
            _ => "",
        }
    }
    fn text_content(&self) -> Option<&TextContent> {
        if let Content::Text(t) = &self.content { Some(t) } else { None }
    }
    fn input(&self) -> Option<&InputContent> {
        if let Content::Input(i) = &self.content { Some(i) } else { None }
    }
    fn input_mut(&mut self) -> Option<&mut InputContent> {
        if let Content::Input(i) = &mut self.content { Some(i) } else { None }
    }
}

const INPUT_WIDTH: f32 = 160.0;

/// The width limit a text breaks at: the known width, else the available width, with
/// min-content as "break everywhere".
fn width_limit(known: Option<f32>, avail: taffy::AvailableSpace) -> Option<f32> {
    known.or(match avail {
        taffy::AvailableSpace::Definite(w) => Some(w),
        taffy::AvailableSpace::MinContent => Some(0.0),
        taffy::AvailableSpace::MaxContent => None,
    })
}

/// What a text node's layout was made for: its version, its shape style and the fonts.
type LayoutKey = (u64, &'static str, u32, u16, u32, u64);

/// A text node's layout, made again when its version, shape style or the fonts changed (its
/// unchanged paragraphs reused).
fn node_layout(engine: &mut dyn TextEngine, n: &Node, fonts: u64) -> Option<text::TextLayout> {
    let t = n.text_content()?;
    let style = n.visual.text_style();
    let key: LayoutKey = (t.version, style.family, style.font_size.to_bits(), style.weight, style.line_height.to_bits(), fonts);
    let mut kept = t.layout.borrow_mut();
    if let Some((k, l)) = kept.as_ref()
        && *k == key
    {
        return Some(l.clone());
    }
    let clean = t.clean.replace(t.text.len());
    let layout = engine.layout(&t.text, &style, &t.shape, kept.as_ref().map(|(_, l)| l), clean);
    *kept = Some((key, layout.clone()));
    Some(layout)
}

fn text_size(engine: &mut dyn TextEngine, n: &Node, fonts: u64, known: Option<f32>, avail: taffy::AvailableSpace) -> (f32, f32) {
    let Some(t) = n.text_content() else { return (0.0, 0.0) };
    if t.text.is_empty() {
        return (0.0, 0.0);
    }
    let Some(layout) = node_layout(engine, n, fonts) else { return (0.0, 0.0) };
    let m = text::metrics(&layout, &n.visual.text_style(), width_limit(known, avail));
    // Whole pixels, rounded up (as Rune): fractional line heights would make rows alternate.
    (m.width.ceil(), m.height.ceil())
}

/// A text input's natural size: a fixed width; one line high, or (multi-line) as many lines as
/// its value wraps into at its width.
fn input_size(engine: &mut dyn TextEngine, n: &Node, known: Option<f32>, avail: taffy::AvailableSpace) -> (f32, f32) {
    let lh = n.visual.line_height;
    let Some(i) = n.input().filter(|i| i.multiline) else { return (INPUT_WIDTH, lh) };
    let width = known.unwrap_or(INPUT_WIDTH);
    if i.value.is_empty() {
        return (width, f32::from(i.rows.0.max(1)) * lh);
    }
    let style = edit::input_style(n.visual.text_style(), true);
    // The value alone: a composition in progress does not resize the field.
    let layout = engine.layout(&i.value, &style, &[], None, 0);
    let m = text::metrics(&layout, &style, width_limit(Some(width), avail));
    let lines = m.lines.clamp(u32::from(i.rows.0.max(1)), u32::from(i.rows.1.max(1)));
    (width, (lines as f32 * lh).ceil())
}

fn anchor_layout(l: &mut taffy::Style, placed: Option<Placed>) {
    l.position = Position::Absolute;
    let (x, y) = placed.map_or((0.0, 0.0), |p| (p.x, p.y));
    l.inset = taffy::Rect {
        left: LengthPercentageAuto::length(x),
        top: LengthPercentageAuto::length(y),
        right: LengthPercentageAuto::auto(),
        bottom: LengthPercentageAuto::auto(),
    };
    // Limited to the room on its side: the content scrolls.
    if let Some(Placed { limit: Some(limit), side, .. }) = placed {
        match side {
            Side::Top | Side::Bottom => {
                l.max_size.height = LengthPercentageAuto::length(limit);
                l.overflow.y = taffy::Overflow::Scroll;
            }
            Side::Left | Side::Right => {
                l.max_size.width = LengthPercentageAuto::length(limit);
                l.overflow.x = taffy::Overflow::Scroll;
            }
        }
    }
}

/// The starting layout of a node. Every node is a flex container (like React Native, Rune and
/// SwiftUI stacks). Boxes do not shrink below their content (`flex-shrink: 0`, as in Yoga and
/// Rune; `.flex_shrink()` opts in); text does, so a long label gives way and wraps in a row.
/// Every node starts as a flex container that does not shrink (text does), positioned
/// `relative`: an `absolute` child is placed against its parent, as in Rune, GPUI and Yoga
/// (Taffy's own default is CSS's `static`, which would place it against the window).
fn default_layout(kind: Kind) -> taffy::Style {
    let flex_shrink = if kind == Kind::Text { 1.0 } else { 0.0 };
    taffy::Style { display: taffy::Display::Flex, flex_shrink, position: taffy::Position::Relative, ..Default::default() }
}

pub struct Kernel {
    nodes: SlotMap<NodeId, Node>,
    root: NodeId,
    overlay_layer: NodeId,
    /// Anchored nodes in the order they were anchored (parents before nested overlays).
    anchored: Vec<NodeId>,
    viewport: (f32, f32),
    screen: Breakpoint,
    missing_tokens: FxHashMap<NodeId, Vec<&'static str>>,
    /// What every window's kernel shares: the theme, the text system, input conventions.
    res: Rc<Resources>,
    taffy: taffy::TaffyTree<Measure>,
    needs_layout: bool,
    layout_enabled: bool,
    counters: Counters,
    /// Text glyphs taken from the engine rather than reused (`glyph_extractions`).
    glyphs_taken: Cell<u64>,
    /// Bumped when the shared text system's fonts change: kept layouts and glyphs are stale.
    fonts_generation: u64,
    cause: Option<u64>,
    log: Option<Vec<Op>>,
    /// The time shown (seconds on the app's clock).
    now: f64,
    /// Changes in this frame land at once.
    instant: bool,
    /// Nodes with properties in motion.
    moving: rustc_hash::FxHashSet<NodeId>,
    /// Ink reaches must be computed again (after a layout, or a shadow changed).
    ink_dirty: Cell<bool>,
    culling: bool,
    /// A secure field showing its last typed character, and when it hides.
    revealing: Option<(NodeId, f64)>,
    /// The focused text field and when its caret last moved or its text changed: the caret
    /// blinks from then on (shown for half a second, hidden for half).
    caret: Option<(NodeId, f64)>,
    /// The window is the active one (key on macOS): its focused field shows a caret.
    window_active: bool,
    /// Nodes made modals (shown or not, attached or not): `modals` filters them.
    modal_nodes: rustc_hash::FxHashSet<NodeId>,
    /// Native views (mounted or not): placements and the host follow them.
    native_nodes: rustc_hash::FxHashSet<NodeId>,
    /// Bumped by every change that can alter the display list (paint, layout and text changes,
    /// layouts, theme): an unchanged count with nothing moving means the last frame still holds.
    paint_gen: u64,
    /// The last display list built while nothing moved, with the count and viewport it was
    /// built at.
    last_frame: RefCell<Option<LastFrame>>,
    /// Display lists built (not reused), for tests and benchmarks.
    frames_built: Cell<u64>,
    /// Own drawings built and kept ones used (incremental paint's counters).
    drawings_built: Cell<u64>,
    drawings_reused: Cell<u64>,
    glyph_runs_built: Cell<u64>,
    /// Counts theme changes (a drawing depends on the theme).
    theme_gen: u64,
    /// Nodes moving now and their ancestors: their reach is not their resting one.
    in_motion: RefCell<rustc_hash::FxHashSet<NodeId>>,
    /// Nodes whose semantics or children changed since the last accessibility update.
    a11y_dirty: RefCell<rustc_hash::FxHashSet<NodeId>>,
    /// A layout ran since the last accessibility update (places may have changed).
    a11y_geometry: Cell<bool>,
    /// Nodes whose layout changed since the last accessibility update (what may have moved is
    /// under them and their ancestors), the nodes that were moving then, and the viewport then.
    a11y_relaid: RefCell<rustc_hash::FxHashSet<NodeId>>,
    a11y_moving: RefCell<Vec<NodeId>>,
    a11y_viewport: Cell<(f32, f32)>,
    /// Places compared by accessibility updates (their work).
    a11y_compared: Cell<u64>,
    /// Assistive technology has the tree (updates can be incremental), at this scale.
    a11y_live: Cell<bool>,
    a11y_scale: Cell<f64>,
    /// Debug builds: what assistive technology was sent, to check updates against the tree.
    #[cfg(debug_assertions)]
    a11y_mirror: RefCell<rustc_hash::FxHashMap<accesskit::NodeId, accesskit::Node>>,
    /// Layout passes run (a node created since the last one is not shown yet).
    layouts: u64,
    /// Nodes whose position and size move (`layout_motion`).
    framed: rustc_hash::FxHashSet<NodeId>,
    /// Roots of subtrees on their way out (drawn by their former parent until they have faded).
    ghosts: Vec<NodeId>,
    /// Structural changes now neither enter nor exit (a hot reload, a virtual list's window).
    quiet: bool,
    /// Scroll containers gliding to a requested offset: x and y tracks.
    glides: rustc_hash::FxHashMap<NodeId, [crate::motion::Track; 2]>,
    index: FxHashMap<String, Vec<NodeId>>,
}

/// What every window's kernel shares ([design](https://github.com/octoii-works/octara/blob/main/docs/design/core.md)):
/// the theme, the text system (fonts, fallback, shaping and layout caches), whether input is
/// touch, and the reduced-motion preference. The app changes them and tells each kernel.
pub struct Resources {
    pub(crate) theme: RefCell<Theme>,
    pub(crate) text: RefCell<Box<dyn TextEngine>>,
    pub(crate) touch: Cell<bool>,
    pub(crate) reduced_motion: Cell<bool>,
}

impl Default for Resources {
    fn default() -> Self {
        Resources {
            theme: RefCell::new(Theme::default()),
            text: RefCell::new(Box::new(text::ParleyEngine::default())),
            touch: Cell::new(false),
            reduced_motion: Cell::new(false),
        }
    }
}

impl Resources {
    /// The text system's engine (e.g. `MonoEngine` for benchmarks); kernels sharing it then
    /// measure again (`fonts_changed`).
    pub fn set_text_engine(&self, engine: Box<dyn TextEngine>) {
        *self.text.borrow_mut() = engine;
    }
}

impl Default for Kernel {
    fn default() -> Self {
        Self::new(Rc::default())
    }
}

impl Kernel {
    /// A kernel reading `res` (shared by an app's windows).
    pub fn new(res: Rc<Resources>) -> Self {
        let mut k = Kernel {
            nodes: SlotMap::with_key(),
            root: NodeId::default(),
            overlay_layer: NodeId::default(),
            anchored: Vec::new(),
            viewport: (0.0, 0.0),
            screen: Breakpoint::Compact,
            missing_tokens: Default::default(),
            res,
            taffy: taffy::TaffyTree::new(),
            needs_layout: true,
            layout_enabled: true,
            counters: Counters::default(),
            glyphs_taken: Cell::new(0),
            fonts_generation: 0,
            cause: None,
            log: None,
            now: 0.0,
            instant: false,
            moving: rustc_hash::FxHashSet::default(),
            ink_dirty: Cell::new(true),
            culling: true,
            revealing: None,
            caret: None,
            window_active: true,
            modal_nodes: rustc_hash::FxHashSet::default(),
            native_nodes: rustc_hash::FxHashSet::default(),
            paint_gen: 0,
            last_frame: RefCell::new(None),
            frames_built: Cell::new(0),
            drawings_built: Cell::new(0),
            drawings_reused: Cell::new(0),
            glyph_runs_built: Cell::new(0),
            theme_gen: 0,
            in_motion: RefCell::new(rustc_hash::FxHashSet::default()),
            a11y_dirty: RefCell::default(),
            a11y_geometry: Cell::new(true),
            a11y_relaid: RefCell::new(rustc_hash::FxHashSet::default()),
            a11y_moving: RefCell::new(Vec::new()),
            a11y_viewport: Cell::new((0.0, 0.0)),
            a11y_compared: Cell::new(0),
            a11y_live: Cell::new(false),
            a11y_scale: Cell::new(1.0),
            #[cfg(debug_assertions)]
            a11y_mirror: RefCell::default(),
            layouts: 0,
            framed: rustc_hash::FxHashSet::default(),
            ghosts: Vec::new(),
            quiet: false,
            glides: rustc_hash::FxHashMap::default(),
            index: FxHashMap::default(),
        };
        k.root = k.create(Kind::Root);
        k.nodes[k.root].attached = true;
        // The root is the window, a column (as in React Native): content fills its width and
        // wraps to it; an app root takes the full height with `size_full()` or `flex_1()`.
        k.set_style(k.root, StyleProp::Width(Length::Pct(1.0)));
        k.set_style(k.root, StyleProp::Height(Length::Pct(1.0)));
        k.set_style(k.root, StyleProp::Direction(FlexDirection::Column));
        // The overlay layer: the root's last child, covering the window, so overlays paint on top.
        let layer = k.create(Kind::Box);
        for p in [
            StyleProp::Position(Position::Absolute),
            StyleProp::Top(Length::Px(0.0)),
            StyleProp::Left(Length::Px(0.0)),
            StyleProp::Width(Length::Pct(1.0)),
            StyleProp::Height(Length::Pct(1.0)),
        ] {
            k.set_style(layer, p);
        }
        k.insert_child(k.root, layer, 0);
        k.overlay_layer = layer;
        k.counters = Counters::default();
        k.log = None;
        k
    }

    pub fn root(&self) -> NodeId {
        self.root
    }
    /// Bumped by every change that can alter what is drawn: unchanged and with nothing moving,
    /// the last frame still holds.
    pub fn paint_generation(&self) -> u64 {
        self.paint_gen
    }
    /// The resources this kernel reads.
    pub fn resources(&self) -> &Rc<Resources> {
        &self.res
    }
    /// Replaces the text engine (e.g. `MonoEngine` for benchmarks) and remeasures every text.
    /// Replaces the text engine (fonts registered with the old one are not carried over).
    pub fn set_text_engine(&mut self, engine: Box<dyn TextEngine>) {
        self.res.set_text_engine(engine);
        self.fonts_changed();
    }
    /// How a text node's content is laid out at its current width: lines shown and whether
    /// any of it is cut off.
    pub fn text_metrics(&self, id: NodeId) -> TextMetrics {
        let n = self.node(id);
        let l = self.taffy.layout(n.taffy).expect("taffy");
        let inner = l.size.width - l.padding.left - l.padding.right - l.border.left - l.border.right;
        if n.text_content().is_none_or(|t| t.text.is_empty()) {
            return TextMetrics::default();
        }
        let Some(layout) = node_layout(&mut **self.res.text.borrow_mut(), n, self.fonts_generation) else { return TextMetrics::default() };
        text::metrics(&layout, &n.visual.text_style(), Some(inner.max(0.0)))
    }
    /// Parent of every overlay; always the root's last child.
    pub fn overlay_layer(&self) -> NodeId {
        self.overlay_layer
    }

    // ---- counters, causes, log ----

    pub fn counters(&self) -> Counters {
        self.counters
    }
    pub fn reset_counters(&mut self) {
        self.counters = Counters::default();
    }
    /// The cause attributed to subsequent ops (an event id).
    pub fn set_cause(&mut self, cause: Option<u64>) {
        self.cause = cause;
    }
    pub fn enable_log(&mut self, on: bool) {
        self.log = if on { Some(Vec::new()) } else { None };
    }
    pub fn take_log(&mut self) -> Vec<Op> {
        self.log.as_mut().map(std::mem::take).unwrap_or_default()
    }

    fn record(&mut self, node: NodeId, kind: OpKind) {
        if let Some(log) = self.log.as_mut() {
            let test_id = self.nodes[node].test_id.clone();
            log.push(Op { cause: self.cause, node, test_id, kind });
        }
    }

    fn mark(&mut self, d: Dirty) {
        if d.intersects(Dirty::PAINT | Dirty::LAYOUT | Dirty::TEXT) {
            self.paint_gen += 1;
        }
        let c = &mut self.counters;
        if d.contains(Dirty::LAYOUT) {
            c.dirty_layout += 1;
            self.needs_layout = true;
        }
        if d.contains(Dirty::PAINT) {
            c.dirty_paint += 1;
        }
        if d.contains(Dirty::TEXT) {
            c.dirty_text += 1;
        }
        if d.contains(Dirty::SEMANTICS) {
            c.dirty_semantics += 1;
        }
    }

    /// `id`'s semantics or children changed: the next accessibility update sends it.
    fn a11y_touch(&self, id: NodeId) {
        // Nobody has the tree yet: the next update sends it whole anyway.
        if self.a11y_live.get() {
            self.a11y_dirty.borrow_mut().insert(id);
        }
    }
    /// `parent`'s shown children changed (the overlay layer, shown only with children, changes
    /// the root's).
    fn a11y_children_changed(&self, parent: NodeId) {
        self.a11y_touch(parent);
        // The children after a change move.
        if self.a11y_live.get() {
            self.a11y_relaid.borrow_mut().insert(parent);
        }
        if parent == self.overlay_layer {
            self.a11y_touch(self.root);
        }
    }
    /// `id`'s subtree left the accessibility tree: if it comes back, it is sent whole.
    fn a11y_forget(&self, id: NodeId) {
        if !self.a11y_live.get() {
            return;
        }
        let mut stack = vec![id];
        while let Some(n) = stack.pop() {
            let node = self.node(n);
            node.a11y_sent.set(None);
            stack.extend(node.children.iter().copied());
        }
    }

    fn changed(&mut self, id: NodeId, d: Dirty, prop: &'static str, value: impl FnOnce() -> String) {
        if d.contains(Dirty::SEMANTICS) {
            self.a11y_touch(id);
        }
        if d.intersects(Dirty::PAINT | Dirty::TEXT | Dirty::LAYOUT) {
            self.node_mut(id).paint_ver += 1;
        }
        // An edit or a move of the focused field's caret: it shows, and blinks from now.
        if let Some((field, since)) = &mut self.caret
            && *field == id
        {
            *since = self.now;
        }
        if d.contains(Dirty::LAYOUT) && self.a11y_live.get() {
            self.a11y_relaid.borrow_mut().insert(id);
        }
        self.counters.prop_changes += 1;
        self.mark(d);
        if self.log.is_some() {
            let v = value();
            self.record(id, OpKind::Set { prop, value: v });
        }
    }

    fn node(&self, id: NodeId) -> &Node {
        self.nodes.get(id).expect("kernel: stale NodeId")
    }
    fn node_mut(&mut self, id: NodeId) -> &mut Node {
        self.nodes.get_mut(id).expect("kernel: stale NodeId")
    }

    // ---- reads ----

    pub fn is_alive(&self, id: NodeId) -> bool {
        self.nodes.contains_key(id)
    }
    pub fn kind(&self, id: NodeId) -> Kind {
        if id == self.root { Kind::Root } else { self.node(id).kind() }
    }
    pub fn role(&self, id: NodeId) -> Role {
        let n = self.node(id);
        if let (None, Content::Native(v)) = (n.role, &n.content) {
            return if v.kind == "web" { Role::WebView } else { Role::GenericContainer };
        }
        // A region is a document unless it is part of an outer one.
        if n.region && n.role.is_none() && self.region_of(id) == Some(id) {
            return Role::Document;
        }
        n.role.unwrap_or(match n.input() {
            Some(i) if i.masked() => Role::PasswordInput,
            Some(i) if i.multiline => Role::MultilineTextInput,
            _ => self.kind(id).default_role(),
        })
    }
    /// A text's own text (empty for other nodes).
    pub fn text(&self, id: NodeId) -> &str {
        self.node(id).text()
    }
    /// A text input's value, else the value set for assistive technology (empty when none).
    pub fn value(&self, id: NodeId) -> &str {
        let n = self.node(id);
        match n.input() {
            Some(i) => &i.value,
            None => n.value.as_deref().unwrap_or(""),
        }
    }
    pub fn checked(&self, id: NodeId) -> Option<bool> {
        self.node(id).checked
    }
    pub fn label(&self, id: NodeId) -> Option<&str> {
        self.node(id).label.as_deref()
    }
    pub fn expanded(&self, id: NodeId) -> Option<bool> {
        self.node(id).expanded
    }
    pub fn state(&self, id: NodeId) -> State {
        self.node(id).state
    }
    pub fn visual(&self, id: NodeId) -> Visual {
        self.node(id).visual
    }
    pub fn test_id(&self, id: NodeId) -> Option<&str> {
        self.node(id).test_id.as_deref()
    }
    pub fn children(&self, id: NodeId) -> &[NodeId] {
        &self.node(id).children
    }
    pub fn parent(&self, id: NodeId) -> Option<NodeId> {
        self.node(id).parent
    }
    pub fn handler(&self, id: NodeId, kind: HandlerKind) -> Option<HandlerId> {
        self.node(id).handlers[kind as usize]
    }
    pub fn live_nodes(&self) -> usize {
        self.nodes.len()
    }
    /// The window size of the last layout (logical pixels).
    pub fn viewport(&self) -> (f32, f32) {
        self.viewport
    }
    /// The shared theme.
    pub fn theme(&self) -> std::cell::Ref<'_, Theme> {
        self.res.theme.borrow()
    }

    // ---- structure ----

    pub fn create(&mut self, kind: Kind) -> NodeId {
        let layout = default_layout(kind);
        let t = self.taffy.new_leaf(layout.clone()).expect("taffy");
        let root_inh = Inherited::root(&self.res.theme.borrow());
        let visual = Unresolved::new(&self.res.theme.borrow(), &root_inh).resolve(&self.res.theme.borrow(), &mut Vec::new());
        let content = match kind {
            Kind::Text => Content::Text(Box::default()),
            Kind::Link => Content::Link(Box::default()),
            Kind::TextInput => Content::Input({
                let mut i = Box::<InputContent>::default();
                i.rows = (1, u16::MAX);
                i
            }),
            Kind::Root | Kind::Box => Content::None,
        };
        let id = self.nodes.insert(Node {
            content,
            ink: Cell::new([0.0; 4]),
            drawn: RefCell::new(None),
            paint_ver: 0,
            a11y_sent: Cell::new(None),
            role: None,
            parent: None,
            children: Vec::new(),
            value: None,
            checked: None,
            label: None,
            description: None,
            tooltip: None,
            expanded: None,
            hidden: false,
            focusable: false,
            tab_index: 0,
            focus_group: false,
            group_current: None,
            autofocus: false,
            modal: None,
            key_context: None,
            commands: Vec::new(),
            anim: None,
            born: 0,
            exiting: false,
            group: None,
            hover_alone: false,
            region: false,
            select_none: false,
            row_key: None,
            selected: None,
            doc_selection: None,
            copy_markdown: None,
            cursor: None,
            level: None,
            live: None,
            anchor: None,
            placed: None,
            scroll: (0.0, 0.0),
            follow: None,
            inherited: root_inh,
            inherited_from: root_inh,
            origin: None,
            component: None,
            test_id: None,
            handlers: [None; HandlerKind::COUNT],
            taffy: t,
            attached: false,
            base: Vec::new(),
            overrides: Vec::new(),
            state: State::empty(),
            layout,
            visual,
        });
        let measure = match kind {
            Kind::Text => Some(Measure::Text(id)),
            Kind::TextInput => Some(Measure::Input(id)),
            Kind::Root | Kind::Box | Kind::Link => None,
        };
        if measure.is_some() {
            self.taffy.set_node_context(t, measure).expect("taffy");
        }
        self.counters.nodes_created += 1;
        let layouts = self.layouts;
        self.node_mut(id).born = layouts;
        self.record(id, OpKind::Create(kind));
        id
    }

    /// Inserts a detached `child` under `parent` at `index` (clamped to the end).
    pub fn insert_child(&mut self, parent: NodeId, child: NodeId, index: usize) {
        assert!(self.node(child).parent.is_none(), "kernel: child already attached");
        let index = index.min(self.child_limit(parent));
        self.node_mut(parent).children.insert(index, child);
        self.node_mut(child).parent = Some(parent);
        if self.node(parent).attached {
            self.set_attached(child, true);
        }
        let (pt, ct) = (self.node(parent).taffy, self.node(child).taffy);
        self.taffy.insert_child_at_index(pt, index, ct).expect("taffy");
        if self.node(parent).inherited != self.node(child).inherited_from {
            self.resolve(child, "inherit");
        }
        self.begin_enter(child);
        self.counters.structural_ops += 1;
        self.a11y_children_changed(parent);
        self.a11y_forget(child);
        self.mark(Dirty::LAYOUT | Dirty::SEMANTICS);
        self.record(child, OpKind::Insert { parent, index });
    }

    /// Taffy 0.15.1 lays out a childless node as a leaf, which skips clearing the list of
    /// absolutely positioned children it hoisted earlier; after its last such child is removed,
    /// rounding then reads the removed node. A fresh taffy node has no such list.
    fn refresh_taffy_node(&mut self, id: NodeId) {
        let old = self.node(id).taffy;
        let new = self.taffy.new_leaf(self.node(id).layout.clone()).expect("taffy");
        if let Some(m) = self.taffy.get_node_context(old).copied() {
            self.taffy.set_node_context(new, Some(m)).expect("taffy");
        }
        if let Some(p) = self.taffy.parent(old) {
            let index = self.taffy.children(p).expect("taffy").iter().position(|c| *c == old).expect("taffy child");
            self.taffy.replace_child_at_index(p, index, new).expect("taffy");
        }
        self.taffy.remove(old).expect("taffy");
        self.node_mut(id).taffy = new;
    }

    /// The highest index a new child can take: the overlay layer stays the root's last child.
    fn child_limit(&self, parent: NodeId) -> usize {
        let kids = &self.node(parent).children;
        kids.len() - usize::from(parent == self.root && kids.last() == Some(&self.overlay_layer))
    }

    pub fn append_child(&mut self, parent: NodeId, child: NodeId) {
        let n = self.node(parent).children.len();
        self.insert_child(parent, child, n);
    }

    /// Detaches `child` from its parent without destroying it.
    pub fn remove_child(&mut self, child: NodeId) {
        let parent = self.node(child).parent.expect("kernel: child not attached");
        let pos = self.node(parent).children.iter().position(|c| *c == child).expect("child of parent");
        self.node_mut(parent).children.remove(pos);
        self.node_mut(child).parent = None;
        if self.node(child).attached {
            self.set_attached(child, false);
        }
        let pt = self.node(parent).taffy;
        self.taffy.remove_child_at_index(pt, pos).expect("taffy");
        if self.node(parent).children.is_empty() && self.node(child).layout.position == Position::Absolute {
            self.refresh_taffy_node(parent);
        }
        self.counters.structural_ops += 1;
        self.a11y_children_changed(parent);
        self.a11y_forget(child);
        self.mark(Dirty::LAYOUT | Dirty::SEMANTICS);
        self.record(child, OpKind::Remove { parent });
    }

    /// Moves an attached `child` to `index` among its siblings.
    pub fn move_child(&mut self, child: NodeId, index: usize) {
        let parent = self.node(child).parent.expect("kernel: child not attached");
        let pos = self.node(parent).children.iter().position(|c| *c == child).expect("child of parent");
        if pos == index {
            return;
        }
        self.node_mut(parent).children.remove(pos);
        let index = index.min(self.child_limit(parent));
        self.node_mut(parent).children.insert(index, child);
        let (pt, ct) = (self.node(parent).taffy, self.node(child).taffy);
        self.taffy.remove_child_at_index(pt, pos).expect("taffy");
        self.taffy.insert_child_at_index(pt, index, ct).expect("taffy");
        self.counters.structural_ops += 1;
        self.a11y_children_changed(parent);
        self.mark(Dirty::LAYOUT | Dirty::SEMANTICS);
        self.record(child, OpKind::Move { index });
    }

    /// Destroys `id` and its subtree, detaching it first.
    pub fn destroy(&mut self, id: NodeId) {
        // Part of a subtree on its way out: it goes with that subtree once it has faded.
        if self.is_exiting(id) {
            return;
        }
        if self.node(id).parent.is_some() {
            self.remove_child(id);
        }
        self.destroy_rec(id);
    }

    fn destroy_rec(&mut self, id: NodeId) {
        let mut children = std::mem::take(&mut self.node_mut(id).children);
        // Subtrees leaving this node go with it.
        let leaving: Vec<NodeId> = self.ghosts.iter().copied().filter(|g| self.node(*g).parent == Some(id)).collect();
        self.ghosts.retain(|g| !leaving.contains(g));
        children.extend(leaving);
        for c in children {
            self.node_mut(c).parent = None;
            self.destroy_rec(c);
        }
        self.record(id, OpKind::Destroy);
        let n = self.nodes.remove(id).expect("node");
        self.missing_tokens.remove(&id);
        self.modal_nodes.remove(&id);
        self.native_nodes.remove(&id);
        if n.anchor.is_some() {
            self.anchored.retain(|a| *a != id);
        }
        self.taffy.remove(n.taffy).expect("taffy");
        self.counters.nodes_destroyed += 1;
    }

    fn set_attached(&mut self, id: NodeId, on: bool) {
        let mut stack = vec![id];
        while let Some(n) = stack.pop() {
            let node = self.node_mut(n);
            node.attached = on;
            stack.extend(node.children.iter().copied());
            if let Some(t) = node.test_id.clone() {
                self.index_set(&t, n, on);
            }
        }
    }

    fn index_set(&mut self, test_id: &str, n: NodeId, on: bool) {
        if on {
            self.index.entry(test_id.to_owned()).or_default().push(n);
        } else if let Some(v) = self.index.get_mut(test_id) {
            v.retain(|x| *x != n);
            if v.is_empty() {
                self.index.remove(test_id);
            }
        }
    }

    // ---- content and semantics ----

    pub fn set_cursor(&mut self, id: NodeId, cursor: crate::platform::CursorStyle) {
        self.node_mut(id).cursor = Some(cursor);
    }

    /// How the pointer looks over `id` (the node it is on): the nearest style set on it or an
    /// ancestor; else a text field's or selectable text's I-beam, a link's hand, an arrow.
    pub fn cursor_over(&self, id: NodeId) -> crate::platform::CursorStyle {
        use crate::platform::CursorStyle;
        let mut n = Some(id);
        while let Some(a) = n {
            if let Some(c) = self.node(a).cursor {
                return c;
            }
            n = self.node(a).parent;
        }
        match self.kind(id) {
            Kind::TextInput => CursorStyle::IBeam,
            Kind::Link => CursorStyle::PointingHand,
            Kind::Text if self.region_of(id).is_some_and(|r| self.selectable_in(r, id)) => CursorStyle::IBeam,
            _ => CursorStyle::Arrow,
        }
    }

    pub fn set_level(&mut self, id: NodeId, level: u8) -> bool {
        if self.node(id).level == Some(level) {
            self.counters.prop_noops += 1;
            return false;
        }
        self.node_mut(id).level = Some(level);
        self.changed(id, Dirty::SEMANTICS, "level", || level.to_string());
        true
    }
    pub fn set_live(&mut self, id: NodeId, live: accesskit::Live) -> bool {
        if self.node(id).live == Some(live) {
            self.counters.prop_noops += 1;
            return false;
        }
        self.node_mut(id).live = Some(live);
        self.changed(id, Dirty::SEMANTICS, "live", || format!("{live:?}"));
        true
    }

    pub fn set_role(&mut self, id: NodeId, role: Role) -> bool {
        if self.node(id).role == Some(role) {
            self.counters.prop_noops += 1;
            return false;
        }
        self.node_mut(id).role = Some(role);
        self.changed(id, Dirty::SEMANTICS, "role", || format!("{role:?}"));
        true
    }

    /// Sets a text's text (other nodes have none: ignored).
    pub fn set_text(&mut self, id: NodeId, text: &str) -> bool {
        let Content::Text(t) = &mut self.node_mut(id).content else { return false };
        if t.text == text {
            self.counters.prop_noops += 1;
            return false;
        }
        let same = t.text.bytes().zip(text.bytes()).take_while(|(a, b)| a == b).count();
        t.clean.set(t.clean.get().min(same));
        t.text.clear();
        t.text.push_str(text);
        self.counters.text_bytes += text.len() as u64;
        self.text_changed(id, "text", text);
        true
    }

    /// Appends to a node's text: O(appended).
    pub fn append_text(&mut self, id: NodeId, more: &str) -> bool {
        if more.is_empty() {
            self.counters.prop_noops += 1;
            return false;
        }
        let Content::Text(t) = &mut self.node_mut(id).content else { return false };
        t.clean.set(t.clean.get().min(t.text.len()));
        t.text.push_str(more);
        self.counters.text_bytes += more.len() as u64;
        self.text_changed(id, "text+", more);
        true
    }

    /// Replaces the byte range `start..end` of a node's text with `insert`.
    pub fn splice_text(&mut self, id: NodeId, start: usize, end: usize, insert: &str) -> bool {
        let Content::Text(t) = &mut self.node_mut(id).content else { return false };
        t.clean.set(t.clean.get().min(start));
        t.text.replace_range(start..end, insert);
        self.counters.text_bytes += insert.len() as u64;
        self.text_changed(id, "text~", insert);
        true
    }

    fn text_changed(&mut self, id: NodeId, prop: &'static str, value: &str) {
        if let Content::Text(t) = &mut self.node_mut(id).content {
            t.version += 1;
        }
        self.update_measure(id);
        self.changed(id, Dirty::TEXT | Dirty::LAYOUT | Dirty::SEMANTICS, prop, || value.to_owned());
    }

    /// The node's measured content changed: Taffy must measure it again.
    fn update_measure(&mut self, id: NodeId) {
        let n = self.node(id);
        if matches!(n.content, Content::Text(_) | Content::Input(_)) {
            self.taffy.mark_dirty(n.taffy).expect("taffy");
        }
    }

    /// The input method's composition in a text input (empty when none).
    pub fn set_preedit(&mut self, id: NodeId, text: &str) -> bool {
        if self.node(id).input().is_none_or(|i| i.preedit == text) {
            return false;
        }
        // The composition sits at the caret: what is shown changes from there on.
        let caret = self.selection(id).1;
        let i = self.node_mut(id).input_mut().expect("a text input");
        i.clean.set(i.clean.get().min(caret));
        i.preedit.clear();
        i.preedit.push_str(text);
        self.changed(id, Dirty::PAINT, "preedit", || text.to_owned());
        true
    }
    pub fn preedit(&self, id: NodeId) -> &str {
        self.node(id).input().map_or("", |i| &i.preedit)
    }

    /// A text input's value; for other nodes, the value assistive technology announces.
    pub fn set_value(&mut self, id: NodeId, value: &str) -> bool {
        if self.value(id) == value && (self.node(id).input().is_some() || self.node(id).value.is_some()) {
            self.counters.prop_noops += 1;
            return false;
        }
        let n = self.node_mut(id);
        match n.input_mut() {
            Some(i) => {
                // What a text input shows is unchanged only up to the first byte that differs
                // (and up to its composition, which follows the caret).
                let same = i.value.bytes().zip(value.bytes()).take_while(|(a, b)| a == b).count();
                i.clean.set(i.clean.get().min(same));
                if i.secure {
                    // A masked field's shown text changes with every edit (the bullets).
                    i.clean.set(0);
                    wipe(&mut i.value);
                } else {
                    i.value.clear();
                }
                i.value.push_str(value);
                // A multi-line field's height follows its text.
                let grows = i.multiline;
                if grows {
                    self.update_measure(id);
                }
                let d = if grows { Dirty::TEXT | Dirty::LAYOUT | Dirty::SEMANTICS } else { Dirty::TEXT | Dirty::SEMANTICS };
                self.changed(id, d, "value", || value.to_owned());
            }
            None => {
                n.value = Some(value.to_owned());
                self.changed(id, Dirty::SEMANTICS, "value", || value.to_owned());
            }
        }
        true
    }

    pub fn set_checked(&mut self, id: NodeId, checked: Option<bool>) -> bool {
        if self.node(id).checked == checked {
            self.counters.prop_noops += 1;
            return false;
        }
        let n = self.node_mut(id);
        n.checked = checked;
        n.state.set(State::CHECKED, checked == Some(true));
        self.resolve(id, "checked");
        self.changed(id, Dirty::PAINT | Dirty::SEMANTICS, "checked", || format!("{checked:?}"));
        true
    }

    pub fn set_label(&mut self, id: NodeId, label: Option<&str>) -> bool {
        self.set_string(id, label, |n| &mut n.label, "label")
    }

    pub fn set_description(&mut self, id: NodeId, description: Option<&str>) -> bool {
        self.set_string(id, description, |n| &mut n.description, "description")
    }

    pub fn set_tooltip(&mut self, id: NodeId, tooltip: Option<&str>) -> bool {
        self.set_string(id, tooltip, |n| &mut n.tooltip, "tooltip")
    }
    pub fn tooltip(&self, id: NodeId) -> Option<&str> {
        self.node(id).tooltip.as_deref()
    }
    /// The nearest element from `id` up with a tooltip.
    pub fn tooltip_above(&self, id: NodeId) -> Option<NodeId> {
        let mut n = Some(id);
        while let Some(x) = n {
            if self.node(x).tooltip.is_some() {
                return Some(x);
            }
            n = self.node(x).parent;
        }
        None
    }

    pub fn set_test_id(&mut self, id: NodeId, test_id: Option<&str>) -> bool {
        if self.node(id).test_id.as_deref() == test_id {
            self.counters.prop_noops += 1;
            return false;
        }
        if self.node(id).attached {
            if let Some(old) = self.node(id).test_id.clone() {
                self.index_set(&old, id, false);
            }
            if let Some(new) = test_id {
                self.index_set(new, id, true);
            }
        }
        self.node_mut(id).test_id = test_id.map(str::to_owned);
        self.changed(id, Dirty::SEMANTICS, "test_id", || test_id.unwrap_or("").to_owned());
        true
    }

    fn set_string(&mut self, id: NodeId, v: Option<&str>, field: fn(&mut Node) -> &mut Option<String>, prop: &'static str) -> bool {
        let slot = field(self.node_mut(id));
        if slot.as_deref() == v {
            self.counters.prop_noops += 1;
            return false;
        }
        *slot = v.map(str::to_owned);
        self.changed(id, Dirty::SEMANTICS, prop, || v.unwrap_or("").to_owned());
        true
    }

    pub fn set_expanded(&mut self, id: NodeId, expanded: Option<bool>) -> bool {
        if self.node(id).expanded == expanded {
            self.counters.prop_noops += 1;
            return false;
        }
        let n = self.node_mut(id);
        n.expanded = expanded;
        n.state.set(State::EXPANDED, expanded == Some(true));
        self.resolve(id, "expanded");
        self.changed(id, Dirty::SEMANTICS, "expanded", || format!("{expanded:?}"));
        true
    }

    /// Hidden from assistive technology and from `child_ids` (e.g. list spacers, decorative icons).
    pub fn set_hidden(&mut self, id: NodeId, hidden: bool) -> bool {
        if self.node(id).hidden == hidden {
            self.counters.prop_noops += 1;
            return false;
        }
        self.node_mut(id).hidden = hidden;
        if let Some(p) = self.node(id).parent {
            self.a11y_children_changed(p);
        }
        self.a11y_forget(id);
        self.changed(id, Dirty::SEMANTICS, "hidden", || hidden.to_string());
        true
    }

    /// Makes `id` an overlay positioned next to `anchor` at each layout (or a normal node again).
    pub fn set_anchor(&mut self, id: NodeId, anchor: Option<(NodeId, Placement)>) {
        let n = self.node_mut(id);
        if n.anchor == anchor {
            self.counters.prop_noops += 1;
            return;
        }
        let was = std::mem::replace(&mut n.anchor, anchor);
        n.placed = None;
        match (was, anchor) {
            (None, Some(_)) => self.anchored.push(id),
            (Some(_), None) => self.anchored.retain(|a| *a != id),
            _ => {}
        }
        self.resolve(id, "anchor");
        self.needs_layout = true;
    }
    /// The side the overlay holding `id` was placed on by the last layout (after flipping).
    pub fn placed_side(&self, id: NodeId) -> Option<Side> {
        let mut cur = Some(id);
        while let Some(n) = cur {
            if let Some(p) = self.node(n).placed {
                return Some(p.side);
            }
            cur = self.node(n).parent;
        }
        None
    }

    /// Replaces a box's content (texts and text inputs keep theirs: ignored).
    fn set_content(&mut self, id: NodeId, content: Content) {
        let n = self.node_mut(id);
        if matches!(n.content, Content::Text(_) | Content::Input(_)) {
            return;
        }
        n.content = content;
    }

    /// Draws `icon` in the box (`None`: no icon), its strokes `stroke_width` wide on its view box
    /// (`None`: the icon's own).
    pub fn set_icon(&mut self, id: NodeId, icon: Option<(&crate::icon::Icon, Option<f32>)>) {
        let name = icon.map(|(i, _)| i.name().to_owned());
        let icon = icon.map(|(i, w)| NodeIcon { name: i.name().to_owned(), view_box: i.bounds(), shapes: i.shapes(w) });
        if self.icon(id) == icon.as_ref() {
            return;
        }
        self.set_content(id, icon.map_or(Content::None, |i| Content::Icon(std::rc::Rc::new(i))));
        self.changed(id, Dirty::PAINT, "icon", || name.unwrap_or_default());
    }
    pub fn icon(&self, id: NodeId) -> Option<&NodeIcon> {
        if let Content::Icon(i) = &self.node(id).content { Some(i) } else { None }
    }

    /// Makes the box a canvas drawing `ops` (in its content box's coordinates).
    pub fn set_canvas(&mut self, id: NodeId, ops: Vec<crate::canvas::CanvasOp>) {
        if self.canvas(id) == Some(ops.as_slice()) {
            return;
        }
        let count = ops.len();
        self.set_content(id, Content::Canvas(std::rc::Rc::new(ops)));
        self.changed(id, Dirty::PAINT, "canvas", || format!("{count} ops"));
    }
    pub fn canvas(&self, id: NodeId) -> Option<&[crate::canvas::CanvasOp]> {
        if let Content::Canvas(ops) = &self.node(id).content { Some(ops) } else { None }
    }
    /// Makes the box a native view of `kind` in `slot`, sized by the size it reports until an
    /// authored one.
    pub fn set_native(&mut self, id: NodeId, kind: &str, slot: crate::native::Slot) {
        let content = NativeContent {
            kind: kind.to_owned(),
            slot,
            props: serde_json::Value::Null,
            version: 0,
            intrinsic: None,
            snapshot: None,
            live: false,
        };
        self.set_content(id, Content::Native(Box::new(content)));
        let t = self.node(id).taffy;
        self.taffy.set_node_context(t, Some(Measure::Native(id))).expect("taffy");
        self.taffy.mark_dirty(t).expect("taffy");
        self.native_nodes.insert(id);
        self.changed(id, Dirty::LAYOUT | Dirty::PAINT | Dirty::SEMANTICS, "native", || kind.to_owned());
    }
    /// A native view's props: handed to its host whole when they change.
    pub fn set_native_props(&mut self, id: NodeId, props: &serde_json::Value) -> bool {
        let Content::Native(v) = &mut self.node_mut(id).content else { return false };
        if v.props == *props {
            return false;
        }
        v.props = props.clone();
        v.version += 1;
        self.changed(id, Dirty::SEMANTICS, "props", || props.to_string());
        true
    }
    /// The size a native view reports (its natural size), used where none is authored.
    pub fn set_native_size(&mut self, id: NodeId, size: (f32, f32)) {
        let Content::Native(v) = &mut self.node_mut(id).content else { return };
        if v.intrinsic == Some(size) {
            return;
        }
        v.intrinsic = Some(size);
        self.update_measure(id);
        let t = self.node(id).taffy;
        self.taffy.mark_dirty(t).expect("taffy");
        self.changed(id, Dirty::LAYOUT, "native size", || format!("{size:?}"));
    }
    /// Whether the platform shows a native view (its host's answer to `create`).
    pub fn set_native_live(&mut self, id: NodeId, live: bool) {
        let Content::Native(v) = &mut self.node_mut(id).content else { return };
        if v.live != live {
            v.live = live;
            self.changed(id, Dirty::PAINT, "native live", || live.to_string());
        }
    }
    /// An image of what a native view shows (or none, back to the placeholder).
    pub fn set_native_snapshot(&mut self, id: NodeId, snapshot: Option<crate::image::Image>) {
        let Content::Native(v) = &mut self.node_mut(id).content else { return };
        v.snapshot = snapshot;
        self.changed(id, Dirty::PAINT, "native snapshot", String::new);
    }
    pub fn native(&self, id: NodeId) -> Option<&NativeContent> {
        if let Content::Native(v) = &self.node(id).content { Some(v) } else { None }
    }
    /// Every native view, mounted or not.
    pub fn native_nodes(&self) -> Vec<NodeId> {
        let mut v: Vec<NodeId> = self.native_nodes.iter().copied().collect();
        v.sort();
        v
    }

    /// The size of the node's content box (inside border and padding) in the last layout.
    pub fn content_size(&self, id: NodeId) -> (f32, f32) {
        let n = self.node(id);
        let l = self.taffy.layout(n.taffy).expect("taffy");
        let w = l.size.width - l.border.left - l.border.right - l.padding.left - l.padding.right;
        let h = l.size.height - l.border.top - l.border.bottom - l.padding.top - l.padding.bottom;
        (w.max(0.0), h.max(0.0))
    }

    /// Shows `image` in the box (`None`: no image), fitted by `fit`.
    pub fn set_image(&mut self, id: NodeId, image: Option<(crate::image::Image, crate::image::ObjectFit)>) {
        if self.image(id) == image.as_ref().map(|(i, f)| (i, *f)) {
            return;
        }
        self.set_content(id, image.map_or(Content::None, |(i, f)| Content::Image(i, f)));
        self.resolve(id, "image");
        self.mark(Dirty::PAINT);
    }
    pub fn image(&self, id: NodeId) -> Option<(&crate::image::Image, crate::image::ObjectFit)> {
        if let Content::Image(i, f) = &self.node(id).content { Some((i, *f)) } else { None }
    }
    /// Can take keyboard focus (and so receive `FOCUS_VISIBLE`).
    pub fn set_focusable(&mut self, id: NodeId, focusable: bool) -> bool {
        if self.node(id).focusable == focusable {
            self.counters.prop_noops += 1;
            return false;
        }
        self.node_mut(id).focusable = focusable;
        self.changed(id, Dirty::SEMANTICS, "focusable", || focusable.to_string());
        true
    }
    pub fn is_focusable(&self, id: NodeId) -> bool {
        self.node(id).focusable
    }

    /// Handlers are not visual properties: no invalidation, counted separately.
    pub fn set_handler(&mut self, id: NodeId, kind: HandlerKind, handler: Option<HandlerId>) {
        let slot = &mut self.node_mut(id).handlers[kind as usize];
        if *slot != handler {
            *slot = handler;
            self.counters.handler_sets += 1;
            // What it can do is part of what assistive technology reads.
            self.a11y_touch(id);
        }
    }

    // ---- style ----

    /// Sets a base style property (replacing the previous value of the same property).
    pub fn set_style(&mut self, id: NodeId, prop: StyleProp) -> bool {
        let n = self.node_mut(id);
        // A property replaces the earlier one of its kind and moves last, so `gap(8).gap_x(2)`
        // and `gap_x(2).gap(8)` mean what CSS would.
        match n.base.iter().position(|p| p.same_slot(&prop)) {
            Some(i) if n.base[i] == prop => {
                self.counters.prop_noops += 1;
                return false;
            }
            Some(i) => {
                n.base.remove(i);
                n.base.push(prop);
            }
            None => n.base.push(prop),
        }
        self.resolve(id, "style")
    }

    /// Sets several base properties and state overrides with one resolve (a new element's
    /// fixed styles).
    pub fn set_styles(&mut self, id: NodeId, props: impl IntoIterator<Item = StyleProp>, overrides: Vec<(When, Vec<StyleProp>)>) -> bool {
        let n = self.node_mut(id);
        for prop in props {
            n.base.retain(|p| !p.same_slot(&prop));
            n.base.push(prop);
        }
        for (when, props) in overrides {
            match n.overrides.iter_mut().find(|(w, _)| *w == when) {
                Some((_, p)) => *p = props,
                None => n.overrides.push((when, props)),
            }
        }
        self.resolve(id, "style")
    }

    /// The properties that apply while `state` is active.
    pub fn set_overrides(&mut self, id: NodeId, when: impl Into<When>, props: Vec<StyleProp>) -> bool {
        let when = when.into();
        let n = self.node_mut(id);
        match n.overrides.iter_mut().find(|(w, _)| *w == when) {
            Some((_, p)) => *p = props,
            None => n.overrides.push((when, props)),
        }
        self.resolve(id, "overrides")
    }

    /// Turns an interaction state on or off.
    pub fn set_state(&mut self, id: NodeId, state: State, on: bool) -> bool {
        let n = self.node_mut(id);
        let before = n.state;
        n.state.set(state, on);
        if n.state == before {
            self.counters.prop_noops += 1;
            return false;
        }
        let semantic = state.intersects(State::SELECTED | State::DISABLED);
        let changed = self.resolve(id, "state");
        // A hover group's members follow its hover.
        if state.contains(State::HOVER)
            && let Some(g) = self.node(id).group
        {
            self.resolve_group_members(id, g);
        }
        if semantic {
            self.changed(id, Dirty::SEMANTICS, "state", || format!("{state:?}={on}"));
        }
        // What the kernel draws itself follows states too: a field's caret and selection its
        // focus, a link (drawn by its text) its hover and press.
        // A field gaining focus shows its caret, blinking from now; losing it, none.
        if matches!(self.node(id).content, Content::Input(_)) && state.contains(State::FOCUSED) {
            if on {
                self.caret = Some((id, self.now));
            } else if self.caret.is_some_and(|(f, _)| f == id) {
                self.caret = None;
            }
        }
        let drawn = match &self.node(id).content {
            Content::Input(_) if state.intersects(State::FOCUSED | State::FOCUS_VISIBLE) => Some(id),
            Content::Link(_) if state.intersects(State::HOVER | State::PRESSED) => self.node(id).parent,
            _ => None,
        };
        if let Some(n) = drawn {
            self.changed(n, Dirty::PAINT, "drawn state", || format!("{state:?}={on}"));
        }
        changed || semantic || drawn.is_some()
    }

    /// Sets the shared theme and restyles this kernel (other kernels sharing it are told with
    /// [`theme_changed`](Self::theme_changed)).
    pub fn set_theme(&mut self, theme: Theme) {
        if theme == *self.res.theme.borrow() {
            return;
        }
        *self.res.theme.borrow_mut() = theme;
        self.theme_changed();
    }
    /// The shared theme changed: every node resolves against it again.
    pub fn theme_changed(&mut self) {
        self.paint_gen += 1;
        self.theme_gen += 1;
        self.missing_tokens.clear();
        // Breakpoint widths may have changed too.
        self.screen = self.res.theme.borrow().breakpoint_of(self.viewport.0);
        // Parents before children, so inherited text styles are current.
        let mut stack: Vec<NodeId> = self.nodes.iter().filter(|(_, n)| n.parent.is_none()).map(|(id, _)| id).collect();
        while let Some(id) = stack.pop() {
            self.resolve(id, "theme");
            stack.extend(self.node(id).children.iter().copied());
        }
        self.restyle_runs();
    }
    /// Custom tokens styles use but the theme does not define (drawn as [`MISSING`]).
    pub fn missing_tokens(&self) -> Vec<&'static str> {
        let mut v: Vec<_> = self.missing_tokens.values().flatten().copied().collect();
        v.sort();
        v.dedup();
        v
    }
    /// Adds font data; its families become available to `font_family` and the theme.
    pub fn register_font(&mut self, data: &'static [u8]) {
        self.res.text.borrow_mut().register_font(data);
        self.fonts_changed();
    }
    /// Adds a font text falls back to for `scripts` (see [`TextEngine::register_fallback_font`]).
    pub fn register_fallback_font(&mut self, data: &'static [u8], scripts: &[[u8; 4]]) {
        self.res.text.borrow_mut().register_fallback_font(data, scripts);
        self.fonts_changed();
    }
    /// Every text measures again: shapes may now resolve to other faces (after the shared text
    /// system's fonts changed).
    pub fn fonts_changed(&mut self) {
        self.fonts_generation += 1;
        let ids: Vec<NodeId> =
            self.nodes.iter().filter(|(_, n)| matches!(n.kind(), Kind::Text | Kind::TextInput)).map(|(id, _)| id).collect();
        for id in ids {
            match &self.node(id).content {
                Content::Text(t) => t.clean.set(0),
                Content::Input(i) => i.clean.set(0),
                _ => {}
            }
            self.update_measure(id);
        }
        self.needs_layout = true;
    }

    /// Recomputes the effective style; marks what changed.
    fn resolve(&mut self, id: NodeId, why: &'static str) -> bool {
        let n = self.node(id);
        let mut layout = default_layout(n.kind());
        let from = n.parent.map_or_else(|| Inherited::root(&self.res.theme.borrow()), |p| self.node(p).inherited);
        let mut v = Unresolved::new(&self.res.theme.borrow(), &from);
        let n = {
            self.node_mut(id).inherited_from = from;
            self.node(id)
        };
        for p in &n.base {
            p.apply(&mut layout, &mut v, &self.res.theme.borrow());
        }
        for bp in Breakpoint::ALL.into_iter().filter(|b| *b <= self.screen) {
            for (_, props) in n.overrides.iter().filter(|(w, _)| *w == When::Screen(bp)) {
                for p in props {
                    p.apply(&mut layout, &mut v, &self.res.theme.borrow());
                }
            }
        }
        if self.res.touch.get() {
            for (_, props) in n.overrides.iter().filter(|(w, _)| *w == When::Touch) {
                for p in props {
                    p.apply(&mut layout, &mut v, &self.res.theme.borrow());
                }
            }
        }
        for flag in State::all().iter() {
            if n.state.contains(flag) {
                for (_, props) in n.overrides.iter().filter(|(w, _)| *w == When::State(flag)) {
                    for p in props {
                        p.apply(&mut layout, &mut v, &self.res.theme.borrow());
                    }
                }
            }
        }
        for (w, props) in &n.overrides {
            if let When::GroupHover(g) = w
                && self.group_hovered(id, g)
            {
                for p in props {
                    p.apply(&mut layout, &mut v, &self.res.theme.borrow());
                }
            }
        }
        if n.anchor.is_some() {
            anchor_layout(&mut layout, n.placed);
        }
        // An image sizes its box like a replaced element: one side set, the other follows its
        // aspect ratio; neither set, its own size (one image pixel per logical pixel).
        if let Content::Image(img, _) = &n.content {
            let (w, h) = img.size();
            if w > 0 && h > 0 {
                if layout.aspect_ratio.is_none() {
                    layout.aspect_ratio = Some(w as f32 / h as f32);
                }
                if layout.size.width == taffy::Dimension::auto() && layout.size.height == taffy::Dimension::auto() {
                    layout.size.width = taffy::Dimension::length(w as f32);
                }
            }
        }
        let mut missing = Vec::new();
        let visual = v.resolve(&self.res.theme.borrow(), &mut missing);
        let inherited = v.inherited();
        let (layout_changed, old_visual, t) = (layout != n.layout, n.visual, n.taffy);
        // What the children inherit changed: they resolve again (and so on, while it changes).
        if inherited != n.inherited {
            self.node_mut(id).inherited = inherited;
            let children = self.node(id).children.clone();
            for c in children {
                self.resolve(c, "inherit");
            }
        }
        if missing.is_empty() {
            self.missing_tokens.remove(&id);
        } else {
            self.missing_tokens.insert(id, missing);
        }
        let mut dirty = Dirty::empty();
        if layout_changed {
            dirty |= Dirty::LAYOUT;
            // Shown or not changes what assistive technology sees under the parent.
            if (layout.display == taffy::Display::None) != (self.node(id).layout.display == taffy::Display::None) {
                if let Some(p) = self.node(id).parent {
                    self.a11y_children_changed(p);
                }
                self.a11y_forget(id);
            }
            self.taffy.set_style(t, layout.clone()).expect("taffy");
        }
        if visual != old_visual {
            dirty |= Dirty::PAINT;
            if visual.shadow != old_visual.shadow {
                self.ink_dirty.set(true);
            }
            if visual.text_style() != old_visual.text_style() {
                dirty |= Dirty::LAYOUT | Dirty::TEXT;
            }
        }
        if dirty.is_empty() {
            self.counters.prop_noops += 1;
            return false;
        }
        // Not shown yet (no layout since it was created): it starts at its values.
        let first = self.node(id).born == self.layouts;
        if first || visual != old_visual {
            self.animate(id, &old_visual, &visual, first);
        }
        let n = self.node_mut(id);
        n.layout = layout;
        n.visual = visual;
        if dirty.contains(Dirty::TEXT) {
            self.update_measure(id);
        }
        self.changed(id, dirty, why, || format!("{visual:?}"));
        true
    }

    // ---- layout ----

    /// Benchmarks turn layout off to separate its cost from state updates.
    pub fn set_layout_enabled(&mut self, on: bool) {
        self.layout_enabled = on;
    }

    /// Runs Taffy if anything invalidated layout since the last run. Returns whether it ran.
    pub fn layout(&mut self, width: f32, height: f32) -> bool {
        if (!self.needs_layout && self.viewport == (width, height)) || !self.layout_enabled {
            return false;
        }
        self.viewport = (width, height);
        let screen = self.res.theme.borrow().breakpoint_of(width);
        self.set_screen(screen);
        self.compute_layout(width, height);
        // Overlays are placed from the first pass; being absolute, moving them affects nothing
        // else, so one more (mostly cached) pass settles them.
        if self.place_overlays() {
            self.compute_layout(width, height);
        }
        self.needs_layout = false;
        self.ink_dirty.set(true);
        self.a11y_geometry.set(true);
        self.paint_gen += 1;
        self.layouts += 1;
        self.clamp_scrolls();
        self.track_frames();
        self.counters.layout_runs += 1;
        true
    }

    fn compute_layout(&mut self, width: f32, height: f32) {
        let root = self.node(self.root).taffy;
        let space = taffy::Size { width: taffy::AvailableSpace::Definite(width), height: taffy::AvailableSpace::Definite(height) };
        let Kernel { taffy, nodes, res, fonts_generation, .. } = self;
        let fonts = *fonts_generation;
        let mut engine = res.text.borrow_mut();
        taffy
            .compute_layout_with_measure(root, space, |inputs, _node, ctx, style| {
                let m = ctx.map(|c| *c);
                taffy::compute_leaf_layout(
                    inputs,
                    style,
                    |_, _| 0.0,
                    |known, avail| {
                        let (w, h) = match m {
                            Some(Measure::Text(id)) => text_size(&mut **engine, &nodes[id], fonts, known.width, avail.width),
                            Some(Measure::Input(id)) => input_size(&mut **engine, &nodes[id], known.width, avail.width),
                            Some(Measure::Native(id)) => match &nodes[id].content {
                                Content::Native(v) => v.intrinsic.unwrap_or(NATIVE_DEFAULT),
                                _ => (0.0, 0.0),
                            },
                            None => (0.0, 0.0),
                        };
                        taffy::Size { width: known.width.unwrap_or(w), height: known.height.unwrap_or(h) }
                    },
                )
            })
            .expect("taffy");
    }

    /// The breakpoint of the last layout's width.
    pub fn screen(&self) -> Breakpoint {
        self.screen
    }

    /// Whether input is touch (`When::Touch` overrides apply).
    pub fn touch(&self) -> bool {
        self.res.touch.get()
    }

    /// Switching between touch and pointer input re-resolves the nodes with touch overrides (no
    /// closure runs).
    pub fn set_touch(&mut self, on: bool) {
        if on == self.res.touch.get() {
            return;
        }
        self.res.touch.set(on);
        self.touch_changed();
    }
    /// The shared touch setting changed: the nodes with touch overrides resolve again.
    pub fn touch_changed(&mut self) {
        let ids: Vec<NodeId> =
            self.nodes.iter().filter(|(_, n)| n.overrides.iter().any(|(w, _)| *w == When::Touch)).map(|(id, _)| id).collect();
        for id in ids {
            self.resolve(id, "touch");
        }
    }

    /// Crossing a breakpoint re-resolves the nodes with screen overrides (no closure runs).
    fn set_screen(&mut self, screen: Breakpoint) {
        if screen == self.screen {
            return;
        }
        self.screen = screen;
        let ids: Vec<NodeId> =
            self.nodes.iter().filter(|(_, n)| n.overrides.iter().any(|(w, _)| matches!(w, When::Screen(_)))).map(|(id, _)| id).collect();
        for id in ids {
            self.resolve(id, "screen");
        }
    }

    /// Positions anchored nodes from the current layout; true if any moved.
    fn place_overlays(&mut self) -> bool {
        let mut moved = false;
        for i in 0..self.anchored.len() {
            let id = self.anchored[i];
            let Some((anchor, placement)) = self.node(id).anchor else { continue };
            if !self.is_alive(anchor) || !self.node(anchor).attached {
                continue;
            }
            // The natural size: a limited overlay still reports its whole content.
            let (_, _, w, h) = self.rect(id);
            let (sw, sh) = self.scroll_size(id);
            let a = self.rect(anchor);
            let placed = placement.place(a, (w.max(sw), h.max(sh)), self.viewport);
            let n = self.node_mut(id);
            if n.placed == Some(placed) {
                continue;
            }
            n.placed = Some(placed);
            anchor_layout(&mut n.layout, n.placed);
            let (t, l) = (n.taffy, n.layout.clone());
            self.taffy.set_style(t, l).expect("taffy");
            moved = true;
        }
        moved
    }

    /// Absolute rect `(x, y, w, h)` from the last layout.
    pub fn rect(&self, id: NodeId) -> (f32, f32, f32, f32) {
        if matches!(self.node(id).content, Content::Link(_))
            && let Some(text) = self.node(id).parent
        {
            let (tx, ty, _, _) = self.rect(text);
            let (x, y, w, h) = self.link_frame(id);
            return (tx + x, ty + y, w, h);
        }
        let l = self.taffy.layout(self.node(id).taffy).expect("taffy");
        let (mut x, mut y) = (l.location.x, l.location.y);
        let mut p = self.node(id).parent;
        while let Some(pid) = p {
            let pn = self.node(pid);
            let pl = self.taffy.layout(pn.taffy).expect("taffy");
            // A scroll container moves its content by its scroll offset.
            x += pl.location.x - pn.scroll.0;
            y += pl.location.y - pn.scroll.1;
            p = pn.parent;
        }
        (x, y, l.size.width, l.size.height)
    }

    // ---- scrolling (CSS `overflow`) ----

    /// Whether the node scrolls on (x, y): `overflow_*_auto` / `_scroll`, or a multi-line text
    /// input whose text is taller than it.
    pub fn scrollable(&self, id: NodeId) -> (bool, bool) {
        if self.is_multiline(id) {
            return (false, self.field(id).is_some_and(|f| f.text_height() > f.height + 0.5));
        }
        let o = self.node(id).layout.overflow;
        (o.x == taffy::Overflow::Scroll, o.y == taffy::Overflow::Scroll)
    }
    /// The visible size inside the border (CSS `clientWidth` / `clientHeight`).
    pub fn client_size(&self, id: NodeId) -> (f32, f32) {
        let l = self.taffy.layout(self.node(id).taffy).expect("taffy");
        (
            (l.size.width - l.border.left - l.border.right - l.scrollbar_size.width).max(0.0),
            (l.size.height - l.border.top - l.border.bottom - l.scrollbar_size.height).max(0.0),
        )
    }
    /// The size of the content that can be scrolled through (CSS `scrollWidth` / `scrollHeight`).
    pub fn scroll_size(&self, id: NodeId) -> (f32, f32) {
        let l = self.taffy.layout(self.node(id).taffy).expect("taffy");
        let (cw, ch) = self.client_size(id);
        (l.scrollable_overflow_rect.right.max(cw), l.scrollable_overflow_rect.bottom.max(ch))
    }
    /// The current scroll offset (CSS `scrollLeft` / `scrollTop`); a multi-line text input's
    /// is how far its text is scrolled.
    pub fn scroll_offset(&self, id: NodeId) -> (f32, f32) {
        match self.node(id).input() {
            Some(i) if i.multiline => (0.0, i.scroll.get()),
            _ => self.node(id).scroll,
        }
    }
    /// Scrolls to `(x, y)`, clamped to the scrollable range: someone put it there (the person, a
    /// reveal, the app), so a container following its tail follows from here on only if that is
    /// the end. Returns whether it moved.
    pub fn scroll_to(&mut self, id: NodeId, x: f32, y: f32) -> bool {
        let moved = self.move_scroll(id, x, y);
        self.note_scrolled(id);
        moved
    }
    /// Moves the offset, clamped, without anyone having put it there (clamping after a layout,
    /// a glide's steps, following the tail).
    fn move_scroll(&mut self, id: NodeId, x: f32, y: f32) -> bool {
        // A multi-line text input scrolls its text (the caret may leave the view).
        if self.is_multiline(id) {
            let Some(f) = self.field(id) else { return false };
            let to = y.clamp(0.0, (f.text_height() - f.height).max(0.0));
            let i = self.node(id).input().expect("a text input");
            if i.scroll.get() == to {
                return false;
            }
            i.scroll.set(to);
            self.a11y_touch(id);
            self.changed(id, Dirty::PAINT, "scroll", || format!("{to:?}"));
            return true;
        }
        let (cw, ch) = self.client_size(id);
        let (sw, sh) = self.scroll_size(id);
        let to = (x.clamp(0.0, (sw - cw).max(0.0)), y.clamp(0.0, (sh - ch).max(0.0)));
        if self.node(id).scroll == to {
            return false;
        }
        self.node_mut(id).scroll = to;
        self.a11y_touch(id);
        if !self.anchored.is_empty() {
            // Overlays anchored inside may need to move.
            self.needs_layout = true;
        }
        self.changed(id, Dirty::PAINT, "scroll", || format!("{to:?}"));
        true
    }
    /// Scrolls the containers around `id`, innermost first and each as little as possible, so
    /// that `rect` (window coordinates) shows. Returns whether anything moved.
    pub fn reveal(&mut self, id: NodeId, mut rect: crate::paint::Rect) -> bool {
        let mut moved = false;
        let mut p = self.node(id).parent;
        while let Some(c) = p {
            p = self.node(c).parent;
            let (sx, sy) = self.scrollable(c);
            if !(sx || sy) || self.is_multiline(c) {
                continue;
            }
            let (x, y, _, _) = self.shown_rect(c);
            let l = self.taffy.layout(self.node(c).taffy).expect("taffy");
            let (cx, cy) = (x + l.border.left, y + l.border.top);
            let (cw, ch) = self.client_size(c);
            let need = |at: f32, size: f32, start: f32, room: f32| {
                if at + size > start + room {
                    (at + size) - (start + room)
                } else if at < start {
                    at - start
                } else {
                    0.0
                }
            };
            let dx = if sx { need(rect.x, rect.w, cx, cw) } else { 0.0 };
            let dy = if sy { need(rect.y, rect.h, cy, ch) } else { 0.0 };
            if dx == 0.0 && dy == 0.0 {
                continue;
            }
            let (ox, oy) = self.scroll_offset(c);
            if self.scroll_to(c, ox + dx, oy + dy) {
                let (nx, ny) = self.scroll_offset(c);
                rect.x -= nx - ox;
                rect.y -= ny - oy;
                moved = true;
            }
        }
        moved
    }

    /// Every scrolled node with a test id and its offset.
    pub fn scroll_offsets(&self) -> Vec<(String, (f32, f32))> {
        self.nodes
            .values()
            .filter(|n| n.scroll != (0.0, 0.0) && n.attached)
            .filter_map(|n| n.test_id.clone().map(|t| (t, n.scroll)))
            .collect()
    }
    /// Sets the scroll offset without clamping (when the content is about to change size).
    pub fn scroll_to_unclamped(&mut self, id: NodeId, x: f32, y: f32) {
        if self.node(id).scroll != (x, y) {
            self.node_mut(id).scroll = (x, y);
            self.a11y_touch(id);
            self.changed(id, Dirty::PAINT, "scroll", || format!("{:?}", (x, y)));
        }
    }
    /// The absolute y of the top of the node's content box (inside border and padding).
    /// Where the element was created (`file:line`), if known.
    pub fn origin(&self, id: NodeId) -> Option<&'static std::panic::Location<'static>> {
        self.node(id).origin
    }
    /// The component function that built the element, if one was marked `#[component]`.
    pub fn component(&self, id: NodeId) -> Option<Component> {
        self.node(id).component
    }
    pub fn set_component(&mut self, id: NodeId, component: Option<Component>) {
        self.node_mut(id).component = component;
    }
    pub fn content_origin_y(&self, id: NodeId) -> f32 {
        let l = self.taffy.layout(self.node(id).taffy).expect("taffy");
        self.rect(id).1 + l.border.top + l.padding.top
    }
    /// After a layout, keeps every scroll offset inside its (possibly smaller) range, and every
    /// container following its tail at its end (gliding there, if a glide is on its way).
    fn clamp_scrolls(&mut self) {
        let ids: Vec<NodeId> =
            self.nodes.iter().filter(|(_, n)| n.scroll != (0.0, 0.0) || n.follow == Some(true)).map(|(id, _)| id).collect();
        for id in ids {
            let (x, y) = self.node(id).scroll;
            if self.node(id).follow != Some(true) {
                self.move_scroll(id, x, y);
            } else if self.glides.contains_key(&id) {
                self.glide_scroll(id, x, f32::MAX);
            } else {
                self.move_scroll(id, x, f32::MAX);
            }
        }
    }

    // ---- following the tail ----

    /// Makes `id` keep the end of its content in view (GPUI's `FollowMode::Tail`), following
    /// from now; `false` stops it.
    pub fn set_follow_tail(&mut self, id: NodeId, on: bool) {
        let n = self.node_mut(id);
        match (on, n.follow) {
            (true, None) => n.follow = Some(true),
            (false, _) => n.follow = None,
            _ => {}
        }
        if on {
            self.needs_layout = true;
        }
    }
    /// Whether `id` follows its tail now: it is set to (`follow_tail`), and nobody has scrolled
    /// it away from the end since.
    pub fn is_following_tail(&self, id: NodeId) -> bool {
        self.node(id).follow == Some(true)
    }
    /// Lets go of the tail until `id` is next scrolled to its end (a hot reload restoring a
    /// container the person had scrolled away).
    pub fn release_tail(&mut self, id: NodeId) {
        if let Some(f) = &mut self.node_mut(id).follow {
            *f = false;
        }
    }
    /// Containers following their tail that have let go of it, by test id.
    pub fn released_tails(&self) -> Vec<String> {
        self.nodes.values().filter(|n| n.follow == Some(false) && n.attached).filter_map(|n| n.test_id.clone()).collect()
    }
    /// Someone put `id`'s offset where it is: a container with a tail follows it again at the
    /// end, and lets go of it anywhere else.
    fn note_scrolled(&mut self, id: NodeId) {
        if self.node(id).follow.is_some() {
            let end = self.at_end(id);
            self.node_mut(id).follow = Some(end);
        }
    }
    /// Whether `id` is scrolled to the end of its content (within a pixel).
    fn at_end(&self, id: NodeId) -> bool {
        let (_, ch) = self.client_size(id);
        let (_, sh) = self.scroll_size(id);
        self.node(id).scroll.1 >= sh - ch - 1.0
    }

    // ---- queries by test id ----

    /// An attached node with this test id, O(1).
    pub fn find(&self, test_id: &str) -> Option<NodeId> {
        self.index.get(test_id).and_then(|v| v.first().copied())
    }
    pub fn find_all(&self, test_id: &str) -> Vec<NodeId> {
        self.index.get(test_id).cloned().unwrap_or_default()
    }
    pub fn exists(&self, test_id: &str) -> bool {
        self.find(test_id).is_some()
    }
    pub fn text_of(&self, test_id: &str) -> Option<String> {
        self.find(test_id).map(|id| self.text(id).to_owned())
    }
    pub fn value_of(&self, test_id: &str) -> Option<String> {
        self.find(test_id).map(|id| self.value(id).to_owned())
    }
    pub fn checked_of(&self, test_id: &str) -> Option<bool> {
        self.find(test_id).and_then(|id| self.checked(id))
    }
    pub fn expanded_of(&self, test_id: &str) -> Option<bool> {
        self.find(test_id).and_then(|id| self.node(id).expanded)
    }
    pub fn state_of(&self, test_id: &str) -> Option<State> {
        self.find(test_id).map(|id| self.state(id))
    }
    pub fn visual_of(&self, test_id: &str) -> Option<Visual> {
        self.find(test_id).map(|id| self.visual(id))
    }
    pub fn role_of(&self, test_id: &str) -> Option<Role> {
        self.find(test_id).map(|id| self.role(id))
    }

    /// Test ids of the direct, non-hidden children of the node with `test_id`, in order.
    pub fn child_ids(&self, test_id: &str) -> Vec<String> {
        let Some(id) = self.find(test_id) else { return Vec::new() };
        self.children(id).iter().filter(|c| !self.node(**c).hidden).filter_map(|c| self.test_id(*c).map(str::to_owned)).collect()
    }

    /// Shown nodes whose accessible name is `name`: an explicit label, else a text's own text.
    /// In tree order.
    pub fn find_by_name(&self, name: &str) -> Vec<NodeId> {
        let mut out = Vec::new();
        let mut stack = vec![self.root];
        while let Some(id) = stack.pop() {
            let n = self.node(id);
            if n.layout.display == taffy::Display::None {
                continue;
            }
            let own = n.label.as_deref().or((n.kind() == Kind::Text).then_some(n.text()));
            if own == Some(name) {
                out.push(id);
            }
            stack.extend(n.children.iter().rev().copied());
        }
        out
    }
    /// How to name a node in a message: its test id, else its accessible name, else its role.
    pub fn describe(&self, id: NodeId) -> String {
        let n = self.node(id);
        if let Some(t) = &n.test_id {
            format!("`{t}`")
        } else if let Some(l) = n.label.as_deref().or((n.kind() == Kind::Text).then_some(n.text())).filter(|l| !l.is_empty()) {
            format!("{:?} \"{l}\"", self.role(id))
        } else {
            format!("a {:?}", self.role(id))
        }
    }

    /// Deterministic semantic dump of the attached tree.
    pub fn tree_dump(&self) -> String {
        let mut out = String::new();
        self.dump_rec(self.root, 0, &mut out);
        out
    }

    fn dump_rec(&self, id: NodeId, depth: usize, out: &mut String) {
        let n = self.node(id);
        let _ = write!(out, "{:indent$}{:?}", "", self.role(id), indent = depth * 2);
        if let Some(t) = &n.test_id {
            let _ = write!(out, "#{t}");
        }
        if !n.text().is_empty() {
            let _ = write!(out, " {:?}", n.text());
        }
        let value = self.reported_value(id);
        if !value.is_empty() {
            let _ = write!(out, " value={value:?}");
        }
        if let Some(c) = n.checked {
            let _ = write!(out, " checked={c}");
        }
        if let Some(l) = &n.label {
            let _ = write!(out, " label={l:?}");
        }
        if let Some(e) = n.expanded {
            let _ = write!(out, " expanded={e}");
        }
        if n.hidden {
            out.push_str(" hidden");
        }
        // CHECKED and EXPANDED mirror `checked=` / `expanded=` above.
        let state = n.state - (State::CHECKED | State::EXPANDED);
        if !state.is_empty() {
            let _ = write!(out, " state={state:?}");
        }
        let default = Unresolved::new(&self.res.theme.borrow(), &Inherited::root(&self.res.theme.borrow()))
            .resolve(&self.res.theme.borrow(), &mut Vec::new());
        if let Some(bg) = n.visual.background {
            let _ = write!(out, " bg={}", hex(bg));
        }
        if n.visual.text != default.text {
            let _ = write!(out, " fg={}", hex(n.visual.text));
        }
        out.push('\n');
        for c in &n.children {
            if *c == self.overlay_layer && self.node(*c).children.is_empty() {
                continue;
            }
            self.dump_rec(*c, depth + 1, out);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree() -> (Kernel, NodeId, NodeId) {
        let mut k = Kernel::default();
        let col = k.create(Kind::Box);
        k.set_test_id(col, Some("col"));
        k.set_style(col, StyleProp::Direction(FlexDirection::Column));
        let root = k.root();
        k.append_child(root, col);
        let t = k.create(Kind::Text);
        k.set_test_id(t, Some("label"));
        k.set_text(t, "hello");
        k.append_child(col, t);
        (k, col, t)
    }

    #[test]
    fn layout_measures_text_from_its_font() {
        let (mut k, col, t) = tree();
        k.set_text_engine(Box::new(text::MonoEngine));
        assert!(k.layout(400.0, 300.0));
        assert_eq!((k.text_metrics(t).width, k.rect(t).3), (5.0 * 8.0, 20.0), "half an em per char; lines 1.25 × 16 px");
        assert!(!k.layout(400.0, 300.0), "nothing changed");
        k.set_style(t, StyleProp::FontSize(FontSize::Len(Length::Px(20.0))));
        assert!(k.layout(400.0, 300.0), "font size invalidates layout");
        assert_eq!(k.text_metrics(t).width, 5.0 * 10.0);
        // Text wraps to the width it gets (CSS default) and reports what it shows.
        k.set_text(t, "hello hello hello");
        k.set_style(col, StyleProp::Width(Length::Px(120.0)));
        k.layout(400.0, 300.0);
        assert_eq!(k.text_metrics(t).lines, 2);
        k.set_style(t, StyleProp::TextWrap(Wrap::Truncate));
        k.layout(400.0, 300.0);
        let m = k.text_metrics(t);
        assert!(m.truncated && m.lines == 1, "{m:?}");
    }

    #[test]
    fn parley_text_follows_the_viewport() {
        let mut k = Kernel::default();
        let t = k.create(Kind::Text);
        k.set_text(t, &"word ".repeat(60));
        k.append_child(k.root(), t);
        k.layout(800.0, 600.0);
        let wide = k.text_metrics(t);
        k.layout(200.0, 600.0);
        let narrow = k.text_metrics(t);
        assert_eq!(k.rect(t).2, 200.0);
        assert!(narrow.lines > wide.lines && wide.lines > 1, "rewraps when the window narrows: {wide:?} -> {narrow:?}");
        assert!(!narrow.truncated);
    }

    #[test]
    fn style_invalidates_only_what_changed() {
        let (mut k, col, t) = tree();
        k.layout(400.0, 300.0);
        k.reset_counters();
        assert!(!k.set_text(t, "hello"));
        assert!(k.set_style(t, StyleProp::TextColor(Token::Accent.into())));
        let c = k.counters();
        assert_eq!((c.prop_noops, c.prop_changes, c.dirty_paint, c.dirty_layout), (1, 1, 1, 0));
        assert!(!k.set_style(t, StyleProp::TextColor(Token::Accent.into())), "same value is a no-op");
        assert!(k.set_style(col, StyleProp::Gap(Length::Px(4.0))));
        assert_eq!(k.counters().dirty_layout, 1);
        assert_eq!(k.visual_of("label").unwrap().text, Theme::light().get(Token::Accent));
    }

    #[test]
    fn overrides_follow_states_and_themes() {
        let (mut k, col, _) = tree();
        k.set_style(col, StyleProp::Background(Some(Token::Surface.into())));
        k.set_overrides(col, State::HOVER, vec![StyleProp::Background(Some(Token::SurfaceHover.into()))]);
        k.set_overrides(col, State::DISABLED, vec![StyleProp::Opacity(0.5)]);
        let bg = |k: &Kernel| k.visual_of("col").unwrap().background.unwrap();
        assert_eq!(bg(&k), Theme::light().get(Token::Surface));
        k.set_state(col, State::HOVER, true);
        assert_eq!(bg(&k), Theme::light().get(Token::SurfaceHover));
        k.set_state(col, State::DISABLED, true);
        assert_eq!(k.visual_of("col").unwrap().opacity, 0.5);
        k.set_theme(Theme::dark());
        assert_eq!(bg(&k), Theme::dark().get(Token::SurfaceHover), "tokens follow the theme");
        k.set_state(col, State::HOVER, false);
        assert_eq!(bg(&k), Theme::dark().get(Token::Surface));
    }

    #[test]
    fn semantics_index_and_dump() {
        let (mut k, col, t) = tree();
        let b = k.create(Kind::Box);
        k.set_role(b, Role::Button);
        k.set_label(b, Some("Go"));
        k.set_test_id(b, Some("go"));
        k.append_child(col, b);
        let s = k.create(Kind::Box);
        k.set_hidden(s, true);
        k.set_test_id(s, Some("spacer"));
        k.append_child(col, s);
        assert_eq!(k.role_of("go"), Some(Role::Button));
        assert_eq!(k.child_ids("col"), ["label", "go"], "hidden nodes are skipped");
        assert_eq!(
            k.tree_dump(),
            "Window\n  GenericContainer#col\n    Label#label \"hello\"\n    Button#go label=\"Go\"\n    GenericContainer#spacer hidden\n"
        );
        k.destroy(col);
        assert!(!k.is_alive(t) && !k.exists("label"));
    }

    #[test]
    fn removing_the_last_absolute_child_keeps_layout_valid() {
        let mut k = Kernel::default();
        let root = k.root();
        let host = k.create(Kind::Box);
        k.set_style(host, StyleProp::Position(Position::Absolute));
        k.append_child(root, host);
        let child = k.create(Kind::Box);
        k.set_style(child, StyleProp::Position(Position::Absolute));
        k.set_style(child, StyleProp::Width(Length::Px(10.0)));
        k.append_child(host, child);
        k.layout(100.0, 100.0);
        k.destroy(child);
        k.layout(100.0, 100.0);
        assert_eq!(k.rect(host).0, 0.0);
    }

    #[test]
    fn moves_and_log() {
        let mut k = Kernel::default();
        k.enable_log(true);
        k.set_cause(Some(7));
        let root = k.root();
        let ids: Vec<_> = (0..3)
            .map(|i| {
                let n = k.create(Kind::Text);
                k.set_test_id(n, Some(&format!("n{i}")));
                k.append_child(root, n);
                n
            })
            .collect();
        k.move_child(ids[2], 0);
        let order: Vec<_> = k.children(root).iter().filter_map(|c| k.test_id(*c)).collect();
        assert_eq!(order, ["n2", "n0", "n1"]);
        let log = k.take_log();
        assert!(log.iter().all(|op| op.cause == Some(7)));
        assert!(matches!(log.last().unwrap().kind, OpKind::Move { index: 0 }));
    }
}
