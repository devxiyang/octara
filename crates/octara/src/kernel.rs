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
pub mod style;
pub mod text;
pub mod theme;

use rustc_hash::FxHashMap;
use slotmap::{SlotMap, new_key_type};
use std::cell::{Cell, RefCell};
use std::fmt::Write as _;
use style::{Inherited, Unresolved};
use text::{TextEngine, TextMetrics, TextStyle, Wrap};

pub use accesskit::Role;
pub use edit::floor_boundary;
pub use focus::Arrow;
pub use issues::LayoutIssue;
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
}

impl Kind {
    fn default_role(self) -> Role {
        match self {
            Kind::Root => Role::Window,
            Kind::Box => Role::GenericContainer,
            Kind::Text => Role::Label,
            Kind::TextInput => Role::TextInput,
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
}

impl HandlerKind {
    pub const COUNT: usize = 10;
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
}

struct Node {
    /// What the node shows inside its box; its `Kind` follows from it.
    content: Content,
    role: Option<Role>,
    parent: Option<NodeId>,
    children: Vec<NodeId>,
    /// The value assistive technology announces for a node that is not a text input (a
    /// combo box's choice, a slider's level); a text input's value is its content.
    value: Option<String>,
    checked: Option<bool>,
    label: Option<String>,
    description: Option<String>,
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
    /// An overlay's anchor node and placement; the node is positioned absolutely in the overlay layer.
    anchor: Option<(NodeId, Placement)>,
    /// Where the last layout put an anchored node.
    placed: Option<Placed>,
    /// Scroll offset of a scroll container.
    scroll: (f32, f32),
    /// Text properties passed down to children.
    inherited: Inherited,
    /// What the parent passed down when this node was last resolved.
    inherited_from: Inherited,
    /// Where the element was created (`file:line`), for diagnostics.
    origin: Option<&'static std::panic::Location<'static>>,
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

/// A text input's default width (CSS gives `<input>` about 20 characters).
/// What a node shows inside its box: at most one kind of content.
#[derive(Debug)]
enum Content {
    /// A plain box (or the root).
    None,
    Text(TextContent),
    Input(Box<InputContent>),
    Image(crate::image::Image, crate::image::ObjectFit),
    Icon(std::rc::Rc<NodeIcon>),
    Canvas(std::rc::Rc<Vec<crate::canvas::CanvasOp>>),
}

/// A text's content.
#[derive(Debug, Default)]
struct TextContent {
    text: String,
    /// Bytes known unchanged since the text engine last measured them.
    clean: Cell<usize>,
}

/// A text input's content: its value, the input method's composition, the selection and how
/// far the line is scrolled.
#[derive(Debug, Default)]
struct InputContent {
    value: String,
    preedit: String,
    selection: (usize, usize),
    scroll: Cell<f32>,
    /// Bytes of the shown line (value with composition) known unchanged since last measured.
    clean: Cell<usize>,
}

impl Node {
    fn kind(&self) -> Kind {
        match self.content {
            Content::Text(_) => Kind::Text,
            Content::Input(_) => Kind::TextInput,
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

/// Lays out a text node's content for Taffy and for diagnostics.
fn text_metrics(
    engine: &mut dyn TextEngine,
    id: NodeId,
    content: &str,
    clean: &Cell<usize>,
    style: &TextStyle,
    limit: Option<f32>,
) -> TextMetrics {
    if content.is_empty() {
        return TextMetrics::default();
    }
    let c = clean.replace(content.len());
    match style.wrap {
        Wrap::Words => text::measure(engine, id, content, c, style, limit),
        Wrap::None => text::measure(engine, id, content, c, style, None),
        Wrap::Truncate => {
            let natural = text::measure(engine, id, content, c, style, None);
            let width = limit.map_or(natural.width, |l| natural.width.min(l));
            let cut = natural.lines > 1 || width < natural.width;
            TextMetrics { width, height: style.line_height, lines: 1, truncated: cut }
        }
    }
}

fn text_size(engine: &mut dyn TextEngine, n: &Node, id: NodeId, known: Option<f32>, avail: taffy::AvailableSpace) -> (f32, f32) {
    let Some(t) = n.text_content() else { return (0.0, 0.0) };
    let m = text_metrics(engine, id, &t.text, &t.clean, &n.visual.text_style(), width_limit(known, avail));
    // Whole pixels, rounded up (as Rune): fractional line heights would make rows alternate.
    (m.width.ceil(), m.height.ceil())
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
    /// Input is touch: `When::Touch` overrides apply.
    touch: bool,
    missing_tokens: FxHashMap<NodeId, Vec<&'static str>>,
    text: RefCell<Box<dyn TextEngine>>,
    taffy: taffy::TaffyTree<Measure>,
    theme: Theme,
    needs_layout: bool,
    layout_enabled: bool,
    counters: Counters,
    cause: Option<u64>,
    log: Option<Vec<Op>>,
    /// The time shown (seconds on the app's clock).
    now: f64,
    /// Changes in this frame land at once.
    instant: bool,
    reduced_motion: bool,
    /// Nodes with properties in motion.
    moving: rustc_hash::FxHashSet<NodeId>,
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

impl Default for Kernel {
    fn default() -> Self {
        Self::new()
    }
}

impl Kernel {
    pub fn new() -> Self {
        let mut k = Kernel {
            nodes: SlotMap::with_key(),
            root: NodeId::default(),
            overlay_layer: NodeId::default(),
            anchored: Vec::new(),
            viewport: (0.0, 0.0),
            screen: Breakpoint::Compact,
            touch: false,
            missing_tokens: Default::default(),
            text: RefCell::new(Box::new(text::ParleyEngine::default())),
            taffy: taffy::TaffyTree::new(),
            theme: Theme::default(),
            needs_layout: true,
            layout_enabled: true,
            counters: Counters::default(),
            cause: None,
            log: None,
            now: 0.0,
            instant: false,
            reduced_motion: false,
            moving: rustc_hash::FxHashSet::default(),
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
    /// Replaces the text engine (e.g. `MonoEngine` for benchmarks) and remeasures every text.
    /// Replaces the text engine (fonts registered with the old one are not carried over).
    pub fn set_text_engine(&mut self, engine: Box<dyn TextEngine>) {
        self.text = RefCell::new(engine);
        self.fonts_changed();
    }
    /// How a text node's content is laid out at its current width: lines shown and whether
    /// any of it is cut off.
    pub fn text_metrics(&self, id: NodeId) -> TextMetrics {
        let n = self.node(id);
        let l = self.taffy.layout(n.taffy).expect("taffy");
        let inner = l.size.width - l.padding.left - l.padding.right - l.border.left - l.border.right;
        let Some(t) = n.text_content() else { return TextMetrics::default() };
        text_metrics(&mut **self.text.borrow_mut(), id, &t.text, &t.clean, &n.visual.text_style(), Some(inner.max(0.0)))
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

    fn changed(&mut self, id: NodeId, d: Dirty, prop: &'static str, value: impl FnOnce() -> String) {
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
        self.node(id).role.unwrap_or(self.kind(id).default_role())
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
    pub fn theme(&self) -> &Theme {
        &self.theme
    }

    // ---- structure ----

    pub fn create(&mut self, kind: Kind) -> NodeId {
        let layout = default_layout(kind);
        let t = self.taffy.new_leaf(layout.clone()).expect("taffy");
        let root_inh = Inherited::root(&self.theme);
        let visual = Unresolved::new(&self.theme, &root_inh).resolve(&self.theme, &mut Vec::new());
        let content = match kind {
            Kind::Text => Content::Text(TextContent::default()),
            Kind::TextInput => Content::Input(Box::default()),
            Kind::Root | Kind::Box => Content::None,
        };
        let id = self.nodes.insert(Node {
            content,
            role: None,
            parent: None,
            children: Vec::new(),
            value: None,
            checked: None,
            label: None,
            description: None,
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
            anchor: None,
            placed: None,
            scroll: (0.0, 0.0),
            inherited: root_inh,
            inherited_from: root_inh,
            origin: None,
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
            Kind::Root | Kind::Box => None,
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
        // Texts and text inputs are measured by the text engine (keyed by node).
        if matches!(n.content, Content::Text(_) | Content::Input(_)) {
            self.text.get_mut().forget(id);
        }
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
                i.value.clear();
                i.value.push_str(value);
                self.changed(id, Dirty::TEXT | Dirty::SEMANTICS, "value", || value.to_owned());
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
        changed || semantic
    }

    pub fn set_theme(&mut self, theme: Theme) {
        if theme == self.theme {
            return;
        }
        self.theme = theme;
        self.missing_tokens.clear();
        // Breakpoint widths may have changed too.
        self.screen = self.theme.breakpoint_of(self.viewport.0);
        // Parents before children, so inherited text styles are current.
        let mut stack: Vec<NodeId> = self.nodes.iter().filter(|(_, n)| n.parent.is_none()).map(|(id, _)| id).collect();
        while let Some(id) = stack.pop() {
            self.resolve(id, "theme");
            stack.extend(self.node(id).children.iter().copied());
        }
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
        self.text.get_mut().register_font(data);
        self.fonts_changed();
    }
    /// Adds a font text falls back to for `scripts` (see [`TextEngine::register_fallback_font`]).
    pub fn register_fallback_font(&mut self, data: &'static [u8], scripts: &[[u8; 4]]) {
        self.text.get_mut().register_fallback_font(data, scripts);
        self.fonts_changed();
    }
    /// Every text measures again: shapes may now resolve to other faces.
    fn fonts_changed(&mut self) {
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
        let from = n.parent.map_or_else(|| Inherited::root(&self.theme), |p| self.node(p).inherited);
        let mut v = Unresolved::new(&self.theme, &from);
        let n = {
            self.node_mut(id).inherited_from = from;
            self.node(id)
        };
        for p in &n.base {
            p.apply(&mut layout, &mut v, &self.theme);
        }
        for bp in Breakpoint::ALL.into_iter().filter(|b| *b <= self.screen) {
            for (_, props) in n.overrides.iter().filter(|(w, _)| *w == When::Screen(bp)) {
                for p in props {
                    p.apply(&mut layout, &mut v, &self.theme);
                }
            }
        }
        if self.touch {
            for (_, props) in n.overrides.iter().filter(|(w, _)| *w == When::Touch) {
                for p in props {
                    p.apply(&mut layout, &mut v, &self.theme);
                }
            }
        }
        for flag in State::all().iter() {
            if n.state.contains(flag) {
                for (_, props) in n.overrides.iter().filter(|(w, _)| *w == When::State(flag)) {
                    for p in props {
                        p.apply(&mut layout, &mut v, &self.theme);
                    }
                }
            }
        }
        for (w, props) in &n.overrides {
            if let When::GroupHover(g) = w
                && self.group_hovered(id, g)
            {
                for p in props {
                    p.apply(&mut layout, &mut v, &self.theme);
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
        let visual = v.resolve(&self.theme, &mut missing);
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
            self.taffy.set_style(t, layout.clone()).expect("taffy");
        }
        if visual != old_visual {
            dirty |= Dirty::PAINT;
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
        self.set_screen(self.theme.breakpoint_of(width));
        self.compute_layout(width, height);
        // Overlays are placed from the first pass; being absolute, moving them affects nothing
        // else, so one more (mostly cached) pass settles them.
        if self.place_overlays() {
            self.compute_layout(width, height);
        }
        self.needs_layout = false;
        self.layouts += 1;
        self.clamp_scrolls();
        self.track_frames();
        self.counters.layout_runs += 1;
        true
    }

    fn compute_layout(&mut self, width: f32, height: f32) {
        let root = self.node(self.root).taffy;
        let space = taffy::Size { width: taffy::AvailableSpace::Definite(width), height: taffy::AvailableSpace::Definite(height) };
        let Kernel { taffy, nodes, text, .. } = self;
        let engine = text.get_mut();
        taffy
            .compute_layout_with_measure(root, space, |inputs, _node, ctx, style| {
                let m = ctx.map(|c| *c);
                taffy::compute_leaf_layout(
                    inputs,
                    style,
                    |_, _| 0.0,
                    |known, avail| {
                        let (w, h) = match m {
                            Some(Measure::Text(id)) => text_size(&mut **engine, &nodes[id], id, known.width, avail.width),
                            Some(Measure::Input(id)) => (INPUT_WIDTH, nodes[id].visual.line_height),
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
        self.touch
    }

    /// Switching between touch and pointer input re-resolves the nodes with touch overrides (no
    /// closure runs).
    pub fn set_touch(&mut self, on: bool) {
        if on == self.touch {
            return;
        }
        self.touch = on;
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

    /// Whether the node scrolls on (x, y): `overflow_*_auto` / `_scroll`.
    pub fn scrollable(&self, id: NodeId) -> (bool, bool) {
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
    /// The current scroll offset (CSS `scrollLeft` / `scrollTop`).
    pub fn scroll_offset(&self, id: NodeId) -> (f32, f32) {
        self.node(id).scroll
    }
    /// Scrolls to `(x, y)`, clamped to the scrollable range. Returns whether it moved.
    pub fn scroll_to(&mut self, id: NodeId, x: f32, y: f32) -> bool {
        let (cw, ch) = self.client_size(id);
        let (sw, sh) = self.scroll_size(id);
        let to = (x.clamp(0.0, (sw - cw).max(0.0)), y.clamp(0.0, (sh - ch).max(0.0)));
        if self.node(id).scroll == to {
            return false;
        }
        self.node_mut(id).scroll = to;
        if !self.anchored.is_empty() {
            // Overlays anchored inside may need to move.
            self.needs_layout = true;
        }
        self.changed(id, Dirty::PAINT, "scroll", || format!("{to:?}"));
        true
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
            self.changed(id, Dirty::PAINT, "scroll", || format!("{:?}", (x, y)));
        }
    }
    /// The absolute y of the top of the node's content box (inside border and padding).
    /// Where the element was created (`file:line`), if known.
    pub fn origin(&self, id: NodeId) -> Option<&'static std::panic::Location<'static>> {
        self.node(id).origin
    }
    pub fn content_origin_y(&self, id: NodeId) -> f32 {
        let l = self.taffy.layout(self.node(id).taffy).expect("taffy");
        self.rect(id).1 + l.border.top + l.padding.top
    }
    /// After a layout, keeps every scroll offset inside its (possibly smaller) range.
    fn clamp_scrolls(&mut self) {
        let ids: Vec<NodeId> = self.nodes.iter().filter(|(_, n)| n.scroll != (0.0, 0.0)).map(|(id, _)| id).collect();
        for id in ids {
            let (x, y) = self.node(id).scroll;
            self.scroll_to(id, x, y);
        }
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
        let value = self.value(id);
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
        let default = Unresolved::new(&self.theme, &Inherited::root(&self.theme)).resolve(&self.theme, &mut Vec::new());
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
        let mut k = Kernel::new();
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
        let mut k = Kernel::new();
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
        let mut k = Kernel::new();
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
        let mut k = Kernel::new();
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
