//! Diagnostics: every problem the runtime reports has a stable code registered here, with its
//! severity, cause and where the docs explain it. Reporting takes a [`Code`], so an
//! unregistered code does not compile; `octara explain <CODE>` reads this table.

use std::fmt;

/// How bad a diagnostic is. Fixed per code.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Severity {
    /// Worth knowing, usually intended (e.g. text cut by `truncate`).
    Info,
    /// Probably a mistake; the app keeps working.
    Warning,
    /// A mistake the runtime rejected or could not honor.
    Error,
}

/// Which part of the framework reports a code.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Area {
    /// Signals, state, memos, `watch`, collections.
    Reactive,
    /// Components and list rendering.
    View,
    /// Layout: overflow, collapse, truncation, overlays, tokens.
    Layout,
    /// Accessibility: names, target sizes, contrast (`octara check`).
    Accessibility,
    /// Tools driving the app: Inspector requests, scripts and checks.
    Inspect,
}

/// One registered diagnostic code.
pub struct Code {
    /// The stable code, e.g. `OVERFLOW`.
    pub id: &'static str,
    pub severity: Severity,
    pub area: Area,
    /// What it means, in one line.
    pub summary: &'static str,
    /// Why it happens, with the rule behind it where the rule is Octara's own.
    pub cause: &'static str,
    /// Where the docs explain it: a workspace path and heading anchor.
    pub doc: &'static str,
}

impl fmt::Debug for Code {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.id)
    }
}

impl fmt::Display for Code {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.id)
    }
}

impl PartialEq for Code {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

impl Eq for Code {}

impl PartialEq<str> for Code {
    fn eq(&self, other: &str) -> bool {
        self.id == other
    }
}

impl PartialEq<&str> for Code {
    fn eq(&self, other: &&str) -> bool {
        self.id == *other
    }
}

/// A diagnostic the runtime recorded: once per (code, location); repeats are counted.
#[derive(Clone, Debug)]
pub struct Diagnostic {
    pub code: &'static Code,
    pub message: String,
    /// Where it happened (`file:line:column`).
    pub location: Option<String>,
}

impl Diagnostic {
    pub fn severity(&self) -> Severity {
        self.code.severity
    }
}

/// The registered code `id`, if any.
/// A source file as reports name it: with `/` on every platform, so snapshots, diagnostics
/// and pending work read the same on Windows.
pub(crate) fn source_file(at: &std::panic::Location<'_>) -> String {
    at.file().replace('\\', "/")
}

pub fn code(id: &str) -> Option<&'static Code> {
    CODES.iter().copied().find(|c| c.id == id)
}

macro_rules! codes {
    ($($name:ident: $sev:ident, $area:ident, $doc:literal, $summary:literal, cause: $cause:literal;)*) => {
        $(
            #[doc = $summary]
            pub static $name: Code = Code {
                id: stringify!($name),
                severity: Severity::$sev,
                area: Area::$area,
                summary: $summary,
                cause: $cause,
                doc: $doc,
            };
        )*
        /// Every registered code.
        pub static CODES: &[&Code] = &[$(&$name),*];
    };
}

codes! {
    // ---- reactive ----
    STRICT_READ_UNTRACKED: Warning, Reactive, "docs/design/reactive-core.md#2-layers",
        "A value was read while building a view, outside any binding; that UI will not update.",
        cause: "A component body runs once: `count.get()` there reads the value once and bakes it in. Only closures passed to bindings re-run; `get_untracked()` says a one-shot read is meant.";
    WRITE_IN_BUILD: Error, Reactive, "docs/design/reactive-core.md#2-layers",
        "A signal or collection was written while building a view; the write was rejected.",
        cause: "Building a view only reads state; a write there would rebuild what is being built. State starts from `state(key, || initial)` and is written by handlers and `watch` callbacks.";
    WRITE_IN_DERIVE: Error, Reactive, "docs/design/reactive-core.md#2-layers",
        "A signal or collection was written inside a memo, binding or `watch` source; the write was rejected.",
        cause: "Derivations and observers only read; a write there would make the value depend on evaluation order. A value computed from others is a `memo`; writes belong to handlers and `watch` callbacks.";
    READ_DISPOSED: Error, Reactive, "docs/design/reactive-core.md#4-identity-and-state",
        "A value whose owner was disposed was read (this panics).",
        cause: "State lives as long as its scope: a hidden `show` branch or a deleted list row disposes it. A closure kept the handle past that; a row's item can be looked up by key when needed (`items.item(&key)`).";
    WRITE_DISPOSED: Error, Reactive, "docs/design/reactive-core.md#4-identity-and-state",
        "A value whose owner was disposed was written; the write was dropped.",
        cause: "A handler, task or timer kept a handle to state of a scope that is gone (a closed dialog, a deleted row). Tasks spawned in a component are cancelled with its scope.";
    CYCLE: Error, Reactive, "docs/design/reactive-core.md#3-graph-and-scheduling",
        "A memo read itself while computing.",
        cause: "The memo's closure reads, directly or through other memos, its own value.";
    FLUSH_RUNAWAY: Error, Reactive, "docs/design/reactive-core.md#3-graph-and-scheduling",
        "Updates did not settle; the flush stopped after its round limit.",
        cause: "`watch` callbacks keep writing each other's sources (A sets B, B sets A).";
    NO_OWNER: Warning, Reactive, "docs/design/reactive-core.md#4-identity-and-state",
        "A binding or `watch` was created outside any scope and will never be disposed.",
        cause: "Observers belong to the scope they are created in; outside a component or `App` there is none.";
    DUPLICATE_STATE_KEY: Error, Reactive, "docs/design/reactive-core.md#4-identity-and-state",
        "The same `state` key was used twice in one component; both calls share one signal.",
        cause: "State is identified by its key within the component's name path.";
    STATE_TYPE_CHANGED: Warning, Reactive, "docs/design/reactive-core.md#4-identity-and-state",
        "A named state changed type across a hot reload and was reset.",
        cause: "Hot reload finds state by name; the new code stores a different type under the same key. Expected after changing a state's type.";
    UNKEYED_STATE: Warning, Reactive, "docs/design/reactive-core.md#4-identity-and-state",
        "`Signal::new` was called inside a component; its value is lost on hot reload.",
        cause: "Only named state (`state(key, init)`) is found again after a reload.";
    VIEW_READS_STATE: Warning, Reactive, "docs/design/map-join.md#view-closures",
        "A signal was read inside a view's closure (filter, sort_by, group_by, map, join).",
        cause: "A view runs its closure when an item changes, not when other values change: what it read goes stale silently. What a view needs belongs in the item or comes through `join`.";
    JOIN_FANOUT: Warning, Reactive, "docs/design/map-join.md#diagnostics",
        "One change on the right side of a join updated more than 10 000 rows.",
        cause: "Many rows point at the same right item and their output takes a part of it that changed (one tag on everything, a user on every message).";
    DUPLICATE_KEY: Error, Reactive, "docs/design/reactive-core.md#5-incremental-collections",
        "Two list items share a key; the later one was dropped.",
        cause: "Lists identify items by key: rows, state and moves follow it.";
    // ---- view ----
    AMBIGUOUS_COMPONENT: Warning, View, "docs/design/reactive-core.md#4-identity-and-state",
        "Several unkeyed components of one name share a parent; their state follows their order.",
        cause: "`component(name, ..)` names siblings by position, so inserting one before another hands it the other's state; `component_keyed(name, key, ..)` names them by key.";
    DUPLICATE_COMPONENT_KEY: Error, View, "docs/design/reactive-core.md#4-identity-and-state",
        "Two components of one name have the same key under one parent; they would share state.",
        cause: "A component's identity is its name and key within its parent's name path.";
    EACH_ROW_NOT_ELEMENT: Error, View, "docs/design/reactive-core.md#5-incremental-collections",
        "A list row was not one element; an empty column was used instead.",
        cause: "`each` and `virtual_each` move, insert and remove rows as single nodes.";
    VIRTUAL_LIST_NOT_SCROLLABLE: Error, View, "docs/design/layout.md#5-scrolling-and-overlays",
        "`virtual_each` is not directly inside a scroll container.",
        cause: "A virtual list mounts the rows in its scroll container's viewport: its parent must be `overflow_y_scroll()` with a bounded height.";
    LAYOUT_LOOP: Warning, Layout, "docs/design/layout.md#3-viewport-and-window-size-classes",
        "Layout did not settle in 4 rounds.",
        cause: "Something reads a size (`size_of`, a breakpoint) and changes what that size measures, so each layout changes it again.";
    // ---- layout issues (Window::layout_issues) ----
    OVERFLOW: Warning, Layout, "docs/design/layout.md#4-layout-diagnostics",
        "Children extend past their container's content box.",
        cause: "The children's sizes add up to more than the container has, and nothing shrinks, wraps or scrolls: Octara's boxes do not shrink unless `flex_shrink()` (text: `truncate()`), and do not scroll unless `overflow_y_scroll()`.";
    COLLAPSED: Warning, Layout, "docs/design/layout.md#4-layout-diagnostics",
        "An element with content has zero width or height.",
        cause: "`flex_1` / `h_full` only take space the parent has; a parent sized by its content has none.";
    TEXT_TRUNCATED: Info, Layout, "docs/design/layout.md#4-layout-diagnostics",
        "Text is cut by `truncate()` or `line_clamp`.",
        cause: "The text does not fit the lines it is allowed.";
    NATIVE_COVERED: Warning, Layout, "docs/design/native-island.md#two-slots-no-interleaving-rune",
        "Content is painted over a native view in the overlay, which is drawn above Octara's surface and hides it.",
        cause: "An overlay view (a web view, a picker) sits above everything Octara draws; content after it in paint order that overlaps it ends up underneath. An underlay view (`Slot::Underlay`) sits behind the surface instead.";
    OVERLAY_CLAMPED: Warning, Layout, "docs/design/ui-primitives.md#5-overlays",
        "An overlay fits neither side of its anchor and was pushed into the window.",
        cause: "The overlay is larger than the space on both sides of its anchor.";
    // ---- accessibility (octara check) ----
    UNNAMED_CONTROL: Warning, Accessibility, "docs/design/agent-knowledge.md#inspector-protocol-and-checks",
        "An element can be operated but has no name for assistive technology.",
        cause: "Screen readers announce a control by its label or the text inside it; an icon-only button or an unlabeled text field has neither.";
    UNLABELED_CANVAS: Warning, Accessibility, "docs/design/agent-knowledge.md#inspector-protocol-and-checks",
        "A canvas is shown without a label, so assistive technology cannot say what it shows.",
        cause: "A canvas is an image: its drawing has no text for a screen reader to read. `.a11y_hidden()` marks one that is only decoration.";
    UNLABELED_NATIVE_VIEW: Warning, Accessibility, "docs/design/native-island.md#input-focus-accessibility",
        "A native view is shown without a label, so assistive technology cannot say what it is.",
        cause: "A platform view (a map, a web page) is one element to Octara's semantics; without a label it is announced as nothing.";
    SMALL_TARGET: Warning, Accessibility, "docs/design/agent-knowledge.md#inspector-protocol-and-checks",
        "A control is smaller than the platform's minimum target: 44 × 44 on touch, 24 × 24 with a pointer.",
        cause: "The hit area is the element's box. Fingers need about 44 pt (Apple); with a mouse, WCAG 2.5.8 asks 24 × 24 unless the target is spaced that far from the others. `when_touch(..)` styles apply on touch platforms only.";
    LOW_CONTRAST: Warning, Accessibility, "docs/design/agent-knowledge.md#inspector-protocol-and-checks",
        "Text does not stand out enough from its background.",
        cause: "WCAG asks a contrast of 4.5:1 for text and 3:1 for large text (24 px, or 18.66 px bold). The text tokens are made for their surfaces (`Token::AccentText` on accent fills); muted text on a tinted surface often falls short.";
    NOT_SETTLED: Warning, Inspect, "docs/design/agent-knowledge.md#inspector-protocol-and-checks",
        "Work was still running after the app was left alone for 10 s of app time.",
        cause: "A task, timer or animation keeps going without input (a loop that never ends, a resource that keeps refetching); `pending` lists what runs and where it started.";
    // ---- inspector (scripts and tools) ----
    BAD_REQUEST: Error, Inspect, "docs/design/agent-knowledge.md#inspector-protocol-and-checks",
        "An Inspector request or script line could not be read.",
        cause: "Each line is one JSON object with an `op` the protocol knows (`octara vocab octara::inspect::Request`), and that op's fields.";
    NO_SUCH_ELEMENT: Error, Inspect, "docs/design/agent-knowledge.md#inspector-protocol-and-checks",
        "No element has the test id a request named.",
        cause: "The element is not in the tree now: never given that `.id(..)`, not shown yet, or already removed.";
    STEP_FAILED: Error, Inspect, "docs/design/agent-knowledge.md#inspector-protocol-and-checks",
        "A script step could not be carried out.",
        cause: "The element cannot take that action (nothing handles it, it is disabled), or work did not settle in time.";
    EXPECTATION_FAILED: Error, Inspect, "docs/design/agent-knowledge.md#inspector-protocol-and-checks",
        "A script's `expect` did not hold.",
        cause: "The app is not in the state the script expected at that step; the reply's `actual` holds what was found.";
    CAPTURE_FAILED: Error, Inspect, "docs/design/capture.md#request",
        "A capture could not be made.",
        cause: "Nothing paints captures where the app runs (a bare `inspect::main` has no renderer), or the PNG could not be written.";
    MISSING_TOKEN: Warning, Layout, "docs/design/ui-primitives.md#4-style",
        "An element uses a custom token the theme does not define (drawn magenta).",
        cause: "`Token::custom(name)` is looked up in the theme at paint time; a theme defines it with `Theme::color`.";
}
