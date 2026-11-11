//! Views: elements, property bindings, structure (`show`, components, lists).
//!
//! A `View` is a one-shot description consumed when mounted. Components run once; each dynamic
//! property is one binding that writes one kernel property. Lists: [`each`], [`virtual_each`].

mod element;
mod list;
mod style;

pub(crate) use element::build_el;
pub use element::{El, IntoProp, IntoText, Prop, TextContent, col, el, row, spacer, text, text_input};
pub use list::{each, virtual_each};
pub use style::{Length, Patch, Styled, px, relative, rems, sp};

use crate::diagnostics as codes;
use crate::kernel::{Breakpoint, NodeId, Theme};
use crate::reactive::graph::{self, Ctx, ReadSignal, ScopeId, Signal, rt};
use rustc_hash::FxHashMap;
use std::cell::{Cell, RefCell};
use std::fmt::Display;
use std::panic::Location;
use std::rc::Rc;

// ---- UI context ----

#[derive(Clone)]
pub(crate) enum Handler {
    Press(Rc<dyn Fn()>),
    Input(Rc<dyn Fn(String)>),
    Toggle(Rc<dyn Fn()>),
    Drag(Rc<dyn Fn(&crate::app::Drag)>),
    Key(Rc<dyn Fn(&crate::app::KeyEvent) -> bool>),
}

#[derive(Default)]
pub(crate) struct Ui {
    pub kernel: RefCell<crate::kernel::Kernel>,
    pub handlers: RefCell<FxHashMap<u64, Handler>>,
    /// The app's command handlers (`on_command`), in registration order.
    pub commands: RefCell<Vec<(String, u64)>>,
    /// Regions of the kernel's overlay layer, one per `overlay` of a mounted element.
    pub overlay_slots: Slots,
    pub viewport: Cell<Option<Signal<(f32, f32)>>>,
    pub breakpoint: Cell<Option<Signal<Breakpoint>>>,
    /// Scroll containers someone follows (virtual lists), with their state as a signal.
    pub scrolls: RefCell<FxHashMap<NodeId, Signal<ScrollState>>>,
    /// Element sizes someone follows (`size_of`), by test id.
    pub sizes: RefCell<FxHashMap<String, Signal<(f32, f32)>>>,
    /// Run after each layout; return whether they changed anything (then the app lays out again).
    pub after_layout: RefCell<Vec<(u64, AfterLayout)>>,
    /// Tasks, timers and channels, and the app's clock.
    pub sched: Rc<crate::task::Scheduler>,
    next: Cell<u64>,
}

/// A hook run after each layout; returns whether it changed anything.
pub(crate) type AfterLayout = Rc<dyn Fn() -> bool>;

/// A scroll container's offset and visible size.
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub(crate) struct ScrollState {
    pub offset: (f32, f32),
    pub client: (f32, f32),
}

thread_local! {
    static UI: RefCell<Option<Rc<Ui>>> = const { RefCell::new(None) };
}

pub(crate) fn ui() -> Rc<Ui> {
    UI.with(|u| u.borrow().clone()).expect("no UI context")
}

pub(crate) fn enter_ui<R>(u: &Rc<Ui>, f: impl FnOnce() -> R) -> R {
    let prev = UI.with(|c| c.replace(Some(u.clone())));
    let r = f();
    UI.with(|c| *c.borrow_mut() = prev);
    r
}

impl Ui {
    /// The (shared) scroll state signal of `node`, kept in step after every layout and scroll.
    pub(crate) fn scroll_state(&self, node: NodeId) -> Signal<ScrollState> {
        *self.scrolls.borrow_mut().entry(node).or_insert_with(|| Signal::detached(ScrollState::default()))
    }
    pub(crate) fn after_layout(&self, f: Rc<dyn Fn() -> bool>) -> u64 {
        let id = self.next.get() + 1;
        self.next.set(id);
        self.after_layout.borrow_mut().push((id, f));
        id
    }
    pub(crate) fn remove_after_layout(&self, id: u64) {
        self.after_layout.borrow_mut().retain(|(i, _)| *i != id);
    }
    /// Runs the after-layout hooks and refreshes scroll signals; true if anything changed.
    pub(crate) fn sync_after_layout(&self) -> bool {
        let hooks: Vec<_> = self.after_layout.borrow().iter().map(|(_, f)| f.clone()).collect();
        let mut changed = false;
        for f in hooks {
            changed |= f();
        }
        let k = self.kernel.borrow();
        for (id, sig) in self.sizes.borrow().iter() {
            let size = k.find(id).map_or((0.0, 0.0), |n| {
                let r = k.rect(n);
                (r.2, r.3)
            });
            if sig.get_untracked() != size {
                sig.set(size);
                changed = true;
            }
        }
        let mut dead = Vec::new();
        for (&node, sig) in self.scrolls.borrow().iter() {
            if !k.is_alive(node) {
                dead.push(node);
                continue;
            }
            let s = ScrollState { offset: k.scroll_offset(node), client: k.client_size(node) };
            if sig.get_untracked() != s {
                sig.set(s);
                changed = true;
            }
        }
        drop(k);
        for node in dead {
            if let Some(sig) = self.scrolls.borrow_mut().remove(&node) {
                sig.dispose();
            }
        }
        changed
    }
    pub(crate) fn register(self: &Rc<Self>, h: Handler) -> u64 {
        let id = self.next.get() + 1;
        self.next.set(id);
        self.handlers.borrow_mut().insert(id, h);
        let me = self.clone();
        rt(|rt| {
            rt.on_cleanup(Box::new(move || {
                me.handlers.borrow_mut().remove(&id);
            }))
        });
        id
    }
}

/// A one-shot description of UI, consumed when mounted.
pub enum View {
    El(Box<El>),
    Fragment(Vec<View>),
    Empty,
    /// Structure (show, components, lists): mounts itself.
    Dyn(Mount),
}

/// How a structural view mounts itself (opaque).
pub struct Mount(pub(crate) MountFn);

pub(crate) type MountFn = Box<dyn FnOnce(&Parent, &Slots)>;

/// What `.child(..)` takes (as GPUI's `IntoElement`): an element, a view, or a string, which is
/// shown as text.
pub trait IntoElement {
    fn into_view(self, at: &'static Location<'static>) -> View;
}
impl IntoElement for El {
    fn into_view(self, _: &'static Location<'static>) -> View {
        self.into()
    }
}
impl IntoElement for View {
    fn into_view(self, _: &'static Location<'static>) -> View {
        self
    }
}
impl IntoElement for &str {
    fn into_view(self, at: &'static Location<'static>) -> View {
        element::text_at(self.to_owned(), at).into()
    }
}
impl IntoElement for String {
    fn into_view(self, at: &'static Location<'static>) -> View {
        element::text_at(self, at).into()
    }
}

/// Several views at once (internal: overlay content).
pub(crate) trait Children {
    fn into_views(self) -> Vec<View>;
}
impl Children for Vec<View> {
    fn into_views(self) -> Vec<View> {
        self
    }
}
impl Children for View {
    fn into_views(self) -> Vec<View> {
        vec![self]
    }
}
impl Children for El {
    fn into_views(self) -> Vec<View> {
        vec![View::El(Box::new(self))]
    }
}
impl Children for () {
    fn into_views(self) -> Vec<View> {
        Vec::new()
    }
}
macro_rules! tuple_children {
    ($($t:ident),+) => {
        impl<$($t: Into<View>),+> Children for ($($t,)+) {
            #[allow(non_snake_case)]
            fn into_views(self) -> Vec<View> {
                let ($($t,)+) = self;
                vec![$($t.into()),+]
            }
        }
    };
}
tuple_children!(A);
tuple_children!(A, B);
tuple_children!(A, B, C);
tuple_children!(A, B, C, D);
tuple_children!(A, B, C, D, E);
tuple_children!(A, B, C, D, E, F);
tuple_children!(A, B, C, D, E, F, G);
tuple_children!(A, B, C, D, E, F, G, H);
tuple_children!(A, B, C, D, E, F, G, H, I);
tuple_children!(A, B, C, D, E, F, G, H, I, J);
tuple_children!(A, B, C, D, E, F, G, H, I, J, K);
tuple_children!(A, B, C, D, E, F, G, H, I, J, K, L);

// ---- mounting ----

/// A parent's children as slots: plain nodes, nested regions (`show`) and counted regions
/// (lists, which track only how many nodes they hold).
pub(crate) type Slots = Rc<RefCell<Vec<Slot>>>;

pub(crate) enum Slot {
    Node,
    Region(Slots),
    Count(Rc<Cell<usize>>),
}

#[derive(Clone)]
pub(crate) struct Parent {
    pub(crate) node: NodeId,
    pub(crate) root: Slots,
}

fn flat_len(slots: &Slots) -> usize {
    slots
        .borrow()
        .iter()
        .map(|s| match s {
            Slot::Node => 1,
            Slot::Region(r) => flat_len(r),
            Slot::Count(c) => c.get(),
        })
        .sum()
}

enum Target<'a> {
    Slots(&'a Slots),
    Count(&'a Rc<Cell<usize>>),
}

/// Kernel children before `target` within `root`.
fn offset(root: &Slots, target: &Target<'_>) -> Option<usize> {
    if let Target::Slots(t) = target
        && Rc::ptr_eq(root, t)
    {
        return Some(0);
    }
    let mut before = 0;
    for s in root.borrow().iter() {
        match s {
            Slot::Node => before += 1,
            Slot::Count(c) => {
                if let Target::Count(t) = target
                    && Rc::ptr_eq(c, t)
                {
                    return Some(before);
                }
                before += c.get();
            }
            Slot::Region(r) => {
                if let Some(o) = offset(r, target) {
                    return Some(before + o);
                }
                before += flat_len(r);
            }
        }
    }
    None
}

pub(crate) fn count_offset(parent: &Parent, count: &Rc<Cell<usize>>) -> usize {
    offset(&parent.root, &Target::Count(count)).expect("list region in parent")
}

pub(crate) fn mount(view: View, parent: &Parent, target: &Slots) {
    match view {
        View::El(e) => {
            let node = build_el(*e);
            let index = offset(&parent.root, &Target::Slots(target)).expect("region in parent") + flat_len(target);
            ui().kernel.borrow_mut().insert_child(parent.node, node, index);
            target.borrow_mut().push(Slot::Node);
        }
        View::Fragment(vs) => vs.into_iter().for_each(|v| mount(v, parent, target)),
        View::Empty => {}
        View::Dyn(Mount(f)) => f(parent, target),
    }
}

// ---- components and show ----

/// A component identified by an explicit name, `Name[key]`: its identity and state do not depend
/// on sibling order. Two instances with the same key are an error.
#[track_caller]
pub fn component_keyed(name: &'static str, key: impl Display, f: impl FnOnce() -> View + 'static) -> View {
    component_at(Location::caller(), name, Some(key.to_string()), f)
}

/// A component named by order among same-named siblings (`Name`, `Name#2`); a second unkeyed
/// sibling is reported because its state then follows its position.
#[track_caller]
pub fn component(name: &'static str, f: impl FnOnce() -> View + 'static) -> View {
    component_at(Location::caller(), name, None, f)
}

fn component_at(at: &'static Location<'static>, name: &'static str, key: Option<String>, f: impl FnOnce() -> View + 'static) -> View {
    View::Dyn(Mount(Box::new(move |parent, target| {
        let s = rt(|rt| {
            let cur = rt.current_scope().expect("component outside a scope");
            let path = rt.scope_path(cur);
            let seg = match &key {
                None => {
                    let seg = rt.child_name(cur, name);
                    if seg != name {
                        rt.diag(
                            &codes::AMBIGUOUS_COMPONENT,
                            format!("several unkeyed `{name}` in {path}; their state follows their order. Use `component_keyed`"),
                            at,
                        );
                    }
                    seg
                }
                Some(k) => {
                    let want = format!("{name}[{k}]");
                    let seg = rt.child_name(cur, &want);
                    if seg != want {
                        rt.diag(
                            &codes::DUPLICATE_COMPONENT_KEY,
                            format!("two `{name}` with key `{k}` in {path}; they would share state"),
                            at,
                        );
                    }
                    seg
                }
            };
            rt.create_scope(Some(cur), &seg)
        });
        rt(|rt| rt.with_frame(Ctx::Build, None, Some(Some(s)), || mount(f(), parent, target)));
    })))
}

/// Shows `then` while `cond` holds; the branch is a scope, so its state lives while it is shown.
#[track_caller]
pub fn show(cond: impl Fn() -> bool + 'static, then: impl Fn() -> View + 'static) -> View {
    show_else_at(Location::caller(), cond, then, || View::Empty)
}

#[track_caller]
pub fn show_else(cond: impl Fn() -> bool + 'static, then: impl Fn() -> View + 'static, els: impl Fn() -> View + 'static) -> View {
    show_else_at(Location::caller(), cond, then, els)
}

fn show_else_at(
    at: &'static Location<'static>,
    cond: impl Fn() -> bool + 'static,
    then: impl Fn() -> View + 'static,
    els: impl Fn() -> View + 'static,
) -> View {
    View::Dyn(Mount(Box::new(move |parent, target| {
        let region = Slots::default();
        target.borrow_mut().push(Slot::Region(region.clone()));
        let (owner, base) = rt(|rt| {
            let owner = rt.current_scope().expect("show outside a scope");
            (owner, rt.child_name(owner, "show"))
        });
        let parent = parent.clone();
        let last: Rc<Cell<Option<bool>>> = Rc::default();
        let branch: Rc<Cell<Option<ScopeId>>> = Rc::default();
        graph::binding(at, move || {
            let c = cond();
            if last.replace(Some(c)) == Some(c) {
                return;
            }
            rt(|rt| {
                if let Some(old) = branch.take() {
                    // The branch's top nodes leave (fading out where motion is set) before their
                    // scope is disposed.
                    let u = ui();
                    let start = offset(&parent.root, &Target::Slots(&region)).expect("region in parent");
                    let len = flat_len(&region);
                    let tops: Vec<NodeId> = u.kernel.borrow().children(parent.node).iter().skip(start).take(len).copied().collect();
                    for n in tops {
                        u.kernel.borrow_mut().begin_exit(n);
                    }
                    rt.dispose_scope(old);
                }
                region.borrow_mut().clear();
                let s = rt.create_scope(Some(owner), &format!("{base}:{}", if c { "then" } else { "else" }));
                branch.set(Some(s));
                rt.with_frame(Ctx::Build, None, Some(Some(s)), || mount(if c { then() } else { els() }, &parent, &region));
            });
        });
    })))
}

/// The window size `(width, height)`, tracked: a binding that reads it reruns on resize.
pub fn viewport() -> ReadSignal<(f32, f32)> {
    ui().viewport.get().expect("viewport() outside an app").read_only()
}

/// The window's breakpoint, tracked: a binding that reads it reruns only when the window
/// crosses a size class boundary. For styles alone prefer `.when_medium(..)` and friends, which
/// rerun nothing.
pub fn breakpoint() -> ReadSignal<Breakpoint> {
    ui().breakpoint.get().expect("breakpoint() outside an app").read_only()
}

/// The laid-out size `(width, height)` of the element with test id `id` ((0, 0) while it does not
/// exist), tracked: a binding that reads it reruns when the size changes. Read it through a memo
/// to rerun only when a condition flips (Rune's `layout_with`):
/// `let narrow = memo(|| size_of("panel").get().0 < 320.0);`. A size that feeds back into
/// itself is cut after a few rounds and reported as `LAYOUT_LOOP`.
pub fn size_of(id: &str) -> ReadSignal<(f32, f32)> {
    let u = ui();
    let mut sizes = u.sizes.borrow_mut();
    if let Some(s) = sizes.get(id) {
        return s.read_only();
    }
    let size = {
        let k = u.kernel.borrow();
        k.find(id).map_or((0.0, 0.0), |n| {
            let r = k.rect(n);
            (r.2, r.3)
        })
    };
    let s = Signal::detached(size);
    sizes.insert(id.to_owned(), s);
    s.read_only()
}

/// Switches the theme. Token colors resolve in the kernel, so no closure reruns; only nodes
/// whose resolved colors change are repainted. Callable from handlers.
pub fn set_theme(theme: Theme) {
    let u = ui();
    let bp = {
        let mut k = u.kernel.borrow_mut();
        k.set_theme(theme);
        let width = u.viewport.get().map_or(0.0, |v| v.get_untracked().0);
        k.theme().breakpoint_of(width)
    };
    // The theme may move the breakpoints.
    if let Some(sig) = u.breakpoint.get()
        && sig.get_untracked() != bp
    {
        sig.set(bp);
    }
}

/// Runs `f`; what it changes lands at once this frame, even where `motion` is set (Rune's
/// `tx.instant`): a value jumping to where the user put it.
pub fn instant<R>(f: impl FnOnce() -> R) -> R {
    ui().kernel.borrow_mut().set_instant(true);
    f()
}

/// Scrolls the scroll container with test id `id` to `(x, y)`: it glides there where its layout
/// moves (`layout_motion` / `motion`), else jumps. Returns whether there is such a container.
pub fn scroll_to(id: &str, x: f32, y: f32) -> bool {
    let u = ui();
    let mut k = u.kernel.borrow_mut();
    let Some(n) = k.find(id) else { return false };
    k.glide_scroll(n, x, y);
    true
}

/// Lands everything in motion on its target now (Rune's `tx.settle`).
pub fn settle_motion() {
    let u = ui();
    let mut k = u.kernel.borrow_mut();
    k.land_motion();
    k.set_instant(true);
}

/// Handles `command` for the whole app (when no focused element handles it): the keymap's
/// chords, `App::invoke` and menus run it. Lives as long as the current scope.
pub fn on_command(command: &str, f: impl Fn() + 'static) {
    let u = ui();
    let id = u.register(Handler::Press(Rc::new(f)));
    u.commands.borrow_mut().push((command.to_owned(), id));
    let me = u.clone();
    rt(|rt| rt.on_cleanup(Box::new(move || me.commands.borrow_mut().retain(|(_, h)| *h != id))));
}
