//! Views: elements, property bindings, structure (`show`, content scopes, lists).
//!
//! A `View` is a one-shot description consumed when mounted. Ordinary UI functions compose
//! descriptions; explicit scopes own content. Each dynamic property is one binding that writes
//! one kernel property. Lists: [`each`], [`virtual_each`].

mod behavior;
mod element;
mod list;
mod pace;
mod structure;
mod style;

pub use crate::kernel::Component;
pub use crate::kernel::rich::{Rich, Span, rich};
pub use behavior::Behavior;
pub(crate) use element::build_el;
pub use element::{El, IntoProp, IntoText, Prop, TextContent, canvas, col, el, icon, native_view, row, spacer, text, text_input, web_view};
pub use list::{IntoRows, each, virtual_each};
pub use octara_macros::ui;
pub(crate) use pace::Paced;
pub use pace::paced;
pub use style::{Length, Patch, Styled, px, relative, rems, sp};

use crate::diagnostics as codes;
use crate::kernel::{Breakpoint, NodeId, Theme};
use crate::reactive::graph::{self, Ctx, ReadSignal, ScopeId, Signal, rt};
use std::cell::{Cell, RefCell};
use std::fmt::Display;
use std::panic::Location;
use std::rc::Rc;

// ---- UI context ----

#[derive(Clone)]
pub(crate) enum Handler {
    Press(Rc<dyn Fn()>),
    Input(Rc<dyn Fn(String)>),
    NumericInput(Rc<dyn Fn(f64)>),
    Toggle(Rc<dyn Fn()>),
    Drag(Rc<dyn Fn(&crate::app::Drag)>),
    Key(Rc<dyn Fn(&crate::app::KeyEvent) -> bool>),
    Native(Rc<dyn Fn(&crate::native::NativeEvent)>),
    Link(Rc<dyn Fn(&str)>),
    Drop(OnDrop),
}

/// Input runs under the scope and window that mounted its callback.
#[derive(Clone)]
pub(crate) struct ScopedHandler {
    pub(crate) handler: Handler,
    execution: graph::ExecutionOwner,
}

impl ScopedHandler {
    pub(crate) fn capture(handler: Handler) -> Self {
        rt(|rt| Self { handler, execution: rt.capture_owner() })
    }

    pub(crate) fn run<R>(self, rt: &graph::Runtime, f: impl FnOnce(Handler) -> R) -> Option<R> {
        if !rt.admits(self.execution) {
            return None;
        }
        Some(rt.with_execution(self.execution, Ctx::Handler, None, || f(self.handler)))
    }
}

/// What an element does with files dropped on it.
pub(crate) type OnDrop = Rc<dyn Fn(&[std::path::PathBuf])>;

/// A one-shot description of UI, consumed when mounted.
/// Use element conversion, [`View::empty`] or [`View::fragment`] to compose descriptions.
/// The representation and mounting callbacks are private.
///
/// ```compile_fail
/// use octara::view::View;
/// let view = View::empty();
/// let representation = view.0;
/// ```
///
/// ```compile_fail
/// use octara::view::Mount;
/// ```
pub struct View(ViewKind);

enum ViewKind {
    El(Box<El>),
    Fragment(Vec<View>),
    Empty,
    Dyn(MountFn),
}

type MountFn = Box<dyn FnOnce(&Parent, &Slots)>;

impl View {
    /// An empty UI description.
    pub fn empty() -> Self {
        Self(ViewKind::Empty)
    }

    /// Sibling views without an extra container or layout box.
    pub fn fragment(views: impl IntoIterator<Item = View>) -> Self {
        Self(ViewKind::Fragment(views.into_iter().collect()))
    }

    fn element(element: Box<El>) -> Self {
        Self(ViewKind::El(element))
    }

    fn dynamic(mount: MountFn) -> Self {
        Self(ViewKind::Dyn(mount))
    }
}

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
        vec![View::element(Box::new(self))]
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
    match view.0 {
        ViewKind::El(e) => {
            let node = build_el(*e);
            let index = offset(&parent.root, &Target::Slots(target)).expect("region in parent") + flat_len(target);
            structure::place(&mut crate::app::window().kernel.borrow_mut(), parent.node, index, [node]);
            target.borrow_mut().push(Slot::Node);
        }
        ViewKind::Fragment(vs) => vs.into_iter().for_each(|v| mount(v, parent, target)),
        ViewKind::Empty => {}
        ViewKind::Dyn(f) => f(parent, target),
    }
}

// ---- content ownership and show ----

/// Builds content once when mounted, in an explicitly keyed owner without an extra node.
/// Named state, context, watches, tasks and cleanup created by the factory belong to this scope.
/// Ordinary UI functions, including those marked [`macro@ui`], do not create owners.
///
/// The key must be unique within the current owner and stable across hot reloads. Sibling order
/// and function names do not identify scopes. A duplicate key reports `DUPLICATE_SCOPE_KEY` and
/// leaves the duplicate content unmounted without running its factory.
///
/// The factory resolves context where the scope is mounted. Content built beforehand is not
/// rebuilt, and resources created beforehand keep their original owner. Pass state handles to
/// ordinary functions when they need no separate lifetime or context boundary.
///
/// State paths use `scope[key]` segments (`%` and `/` in keys are escaped). These paths replace
/// the former component name/key paths; existing state is not recovered through old path aliases.
#[track_caller]
pub fn scope<C: IntoElement>(key: impl Display, content: impl FnOnce() -> C + 'static) -> View {
    let at = Location::caller();
    let key = key.to_string();
    let segment = format!("scope[{}]", key.replace('%', "%25").replace('/', "%2F"));
    let made = building();
    View::dynamic(Box::new(move |parent, target| {
        let s = rt(|rt| {
            let owner = rt.current_scope().expect("scope outside an owner");
            if rt.child_name(owner, &segment) != segment {
                rt.diag(
                    &codes::DUPLICATE_SCOPE_KEY,
                    format!("scope key `{key}` used twice in {}; duplicate content was not mounted", rt.scope_path(owner)),
                    at,
                );
                return None;
            }
            Some(rt.create_scope(Some(owner), &segment))
        });
        let Some(s) = s else { return };
        rt(|rt| {
            rt.with_frame(Ctx::Build, None, Some(Some(s)), || {
                mount(as_part_of(made, || content().into_view(at)), parent, target);
            });
        });
    }))
}

// ---- UI function provenance (`#[ui]`) ----

thread_local! {
    static BUILDING: Cell<Option<Component>> = const { Cell::new(None) };
}

/// Restores the component that was being built when dropped.
#[doc(hidden)]
pub struct Building(Option<Component>);

impl Drop for Building {
    fn drop(&mut self) {
        BUILDING.set(self.0);
    }
}

/// What a `#[ui]` function runs first: the elements it builds, and those its views build
/// later, record its provenance until the guard drops. A UI function called through another's
/// `#[track_caller]` chain from the same call site (`primary_button` calling `button`) belongs to
/// the outer one: that is the one its caller wrote. This never changes the active owner.
#[doc(hidden)]
pub fn enter_ui(name: &'static str, module: &'static str, at: &'static Location<'static>) -> Building {
    let prev = BUILDING.get();
    let next = match prev {
        Some(p) if p.at == at => p,
        _ => Component { name, module, at },
    };
    BUILDING.set(Some(next));
    Building(prev)
}

/// The UI function whose provenance is being recorded now.
pub(crate) fn building() -> Option<Component> {
    BUILDING.get()
}

/// Restores only provenance for a delayed factory, without restoring its construction owner.
pub(crate) fn as_part_of<R>(component: Option<Component>, f: impl FnOnce() -> R) -> R {
    let prev = BUILDING.replace(component);
    let _restore = Building(prev);
    f()
}

/// Shows `then` while `cond` holds; the branch is a scope, so its state lives while it is shown.
#[track_caller]
pub fn show(cond: impl Fn() -> bool + 'static, then: impl Fn() -> View + 'static) -> View {
    show_else_at(Location::caller(), cond, then, View::empty)
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
    switch_at(at, cond, |c| if *c { "then".into() } else { "else".into() }, move |c| if *c { then() } else { els() })
}

/// Shows `view(key)` for the current `key`; when the key changes, the view is built anew (React's
/// `key`, SwiftUI's `.id`): the open conversation's transcript, a form for the selected record.
/// Each build is a scope named by the key, so its state lives while the key stays (and comes back
/// by name across a hot reload).
#[track_caller]
pub fn keyed<K: PartialEq + Display + 'static>(key: impl Fn() -> K + 'static, view: impl Fn(&K) -> View + 'static) -> View {
    switch_at(Location::caller(), key, |k| k.to_string(), view)
}

/// A view rebuilt whenever `key` changes, in a scope named by `name(key)`.
fn switch_at<K: PartialEq + 'static>(
    at: &'static Location<'static>,
    key: impl Fn() -> K + 'static,
    name: impl Fn(&K) -> String + 'static,
    view: impl Fn(&K) -> View + 'static,
) -> View {
    let made = building();
    View::dynamic(Box::new(move |parent, target| {
        let region = Slots::default();
        target.borrow_mut().push(Slot::Region(region.clone()));
        let (owner, base) = rt(|rt| {
            let owner = rt.current_scope().expect("show outside a scope");
            (owner, rt.child_name(owner, "show"))
        });
        let parent = parent.clone();
        let last: Rc<RefCell<Option<K>>> = Rc::default();
        let branch: Rc<Cell<Option<ScopeId>>> = Rc::default();
        let (name, view) = (Rc::new(name), Rc::new(view));
        graph::structural_binding(at, move || {
            let k = key();
            if last.borrow().as_ref() == Some(&k) {
                return;
            }
            rt(|rt| {
                if let Some(old) = branch.take() {
                    // The branch's top nodes leave (fading out where motion is set) before their
                    // scope is disposed.
                    let u = crate::app::window();
                    let start = offset(&parent.root, &Target::Slots(&region)).expect("region in parent");
                    let len = flat_len(&region);
                    let tops: Vec<NodeId> = u.kernel.borrow().children(parent.node).iter().skip(start).take(len).copied().collect();
                    structure::begin_exit(&mut u.kernel.borrow_mut(), tops);
                    rt.dispose_scope(old);
                }
                region.borrow_mut().clear();
                let s = rt.create_scope(Some(owner), &format!("{base}:{}", name(&k)));
                branch.set(Some(s));
                rt.with_frame(Ctx::Build, None, Some(Some(s)), || mount(as_part_of(made, || view(&k)), &parent, &region));
            });
            *last.borrow_mut() = Some(k);
        });
    }))
}

/// The window size `(width, height)`, tracked: a binding that reads it reruns on resize.
pub fn viewport() -> ReadSignal<(f32, f32)> {
    crate::app::window().viewport.read_only()
}

/// The window's breakpoint, tracked: a binding that reads it reruns only when the window
/// crosses a size class boundary. For styles alone prefer `.when_medium(..)` and friends, which
/// rerun nothing.
pub fn breakpoint() -> ReadSignal<Breakpoint> {
    crate::app::window().breakpoint.read_only()
}

/// The laid-out size `(width, height)` of the element with test id `id` ((0, 0) while it does not
/// exist), tracked: a binding that reads it reruns when the size changes. Read it through a memo
/// to rerun only when a condition flips (Rune's `layout_with`):
/// `let narrow = memo(|| size_of("panel").get().0 < 320.0);`. A size that feeds back into
/// itself is cut after a few rounds and reported as `LAYOUT_LOOP`.
pub fn size_of(id: &str) -> ReadSignal<(f32, f32)> {
    crate::app::window().size_signal(id)
}

/// The current pointer press in an element's local coordinates. Available only inside its
/// pointer `on_press` handler (mouse, touch or pen); keyboard and assistive activation return
/// `None`. This does not move focus or retain the pointer after its handler returns.
pub fn pointer_position_in(id: &str) -> Option<(f32, f32)> {
    crate::app::window().pointer_position_in(id)
}

/// Switches the theme of every window. Token colors resolve in the kernels, so no closure
/// reruns; only nodes whose resolved colors change are repainted. Callable from handlers.
#[track_caller]
pub fn set_theme(theme: Theme) {
    crate::app::App::current().set_theme(theme);
}

/// Opens `url` with the system: the browser, the mail app (GPUI's `open_url`). A link no
/// `on_link` takes opens this way. Callable from handlers.
pub fn open_url(url: &str) {
    crate::app::app_state().open_url(url);
}

/// Puts text or an image on the clipboard (GPUI's `write_to_clipboard`):
/// `write_to_clipboard("text")`, `write_to_clipboard(image)`. Callable from handlers.
pub fn write_to_clipboard(item: impl Into<crate::platform::ClipboardItem>) {
    crate::app::app_state().write_clipboard(&item.into());
}

/// What the clipboard holds (GPUI's `read_from_clipboard`): its text, its image.
pub fn read_from_clipboard() -> Option<crate::platform::ClipboardItem> {
    crate::app::app_state().read_clipboard()
}

/// Runs `f`; what it changes lands at once this frame, even where `motion` is set (Rune's
/// `tx.instant`): a value jumping to where the user put it.
pub fn instant<R>(f: impl FnOnce() -> R) -> R {
    crate::app::window().kernel.borrow_mut().set_instant(true);
    f()
}

/// Scrolls the scroll container with test id `id` to `(x, y)`: it glides there where its layout
/// moves (`layout_motion` / `motion`), else jumps. Returns whether there is such a container.
pub fn scroll_to(id: &str, x: f32, y: f32) -> bool {
    let u = crate::app::window();
    let mut k = u.kernel.borrow_mut();
    let Some(n) = k.find(id) else { return false };
    k.glide_scroll(n, x, y);
    true
}

/// Reveals the element with test id `id` in its scroll containers after the next layout,
/// keeping keyboard focus where it is. The element can be mounted in the same frame; a missing
/// or hidden target then does nothing. The last request in a frame wins.
pub fn scroll_into_view(id: &str) {
    crate::app::window().interaction_request_scroll_into_view(id)
}

/// Scrolls the scroll container with test id `id` to the end of its content, gliding where its
/// layout moves, and follows its tail from there if it is set to (`follow_tail`). Returns whether
/// there is such a container.
pub fn scroll_to_end(id: &str) -> bool {
    let u = crate::app::window();
    let mut k = u.kernel.borrow_mut();
    let Some(n) = k.find(id) else { return false };
    let x = k.scroll_offset(n).0;
    k.glide_scroll(n, x, f32::MAX);
    true
}

/// Whether the scroll container with test id `id` follows its tail now (`follow_tail`; `false`
/// while it does not exist), tracked: a binding that reads it reruns when the person scrolls
/// away from the end or back to it (to show a way back to the end).
pub fn following_tail(id: &str) -> ReadSignal<bool> {
    crate::app::window().tail_signal(id)
}

/// The app's clock, tracked, moving in steps of `period` (SwiftUI's periodic `TimelineView`): a
/// binding reading it reruns every `period` (a spinner's step, "5 minutes ago", a caret). Readers
/// in a window with one period share one timer, which stops when the last of their scopes goes;
/// it does not keep the app busy (`run_until_idle`).
///
/// # Panics
/// A zero period is rejected with INVALID_INTERVAL before allocating a clock.
#[track_caller]
pub fn clock(period: std::time::Duration) -> ReadSignal<std::time::Duration> {
    crate::task::require_period(period);
    let u = crate::app::window();
    let step = period.as_nanos();
    let at = move |t: std::time::Duration| std::time::Duration::from_nanos((t.as_nanos() / step * step) as u64);
    let signal = {
        let mut clocks = u.clocks.borrow_mut();
        let e = clocks.entry(step).or_insert_with(|| {
            let s = Signal::detached(at(crate::task::now()));
            // The readers own it together, not the first one's scope.
            let task = crate::app::App::current().run(|| crate::task::interval(period, move || s.set(at(crate::task::now()))));
            crate::app::Clock { signal: s, task, readers: 0 }
        });
        e.readers += 1;
        e.signal
    };
    let u2 = u.clone();
    rt(|rt| {
        rt.on_cleanup(Box::new(move || {
            let mut clocks = u2.clocks.borrow_mut();
            if let Some(e) = clocks.get_mut(&step) {
                e.readers -= 1;
                if e.readers == 0 {
                    let e = clocks.remove(&step).expect("the clock");
                    e.task.cancel_owned();
                    e.signal.dispose();
                }
            }
        }))
    });
    signal.read_only()
}

/// Adds the app's key bindings (GPUI's `bind_keys`): `(chord, command, context)`, a context
/// naming a `key_context` the focus must be in (`None`: anywhere). A binding added again (a hot
/// reload) replaces itself. `Err` names a chord that does not parse.
pub fn bind_keys<'a>(bindings: impl IntoIterator<Item = (&'a str, &'a str, Option<&'a str>)>) -> Result<(), String> {
    let app = crate::app::app_state();
    app.keymap_changed();
    let mut keymap = app.keymap.borrow_mut();
    for (chord, command, context) in bindings {
        let mut one = crate::app::Keymap::default();
        one.bind(chord, Some(command), context)?;
        let b = one.bindings.remove(0);
        keymap.bindings.retain(|x| x != &b);
        keymap.bindings.push(b);
    }
    Ok(())
}

/// Moves keyboard focus to the element with test id `id` once this frame is built (GPUI's
/// `window.focus`): a field after the view that holds it was rebuilt, the composer after "New
/// chat". It must be focusable and enabled then; otherwise focus stays where it is. Callable
/// from handlers and tasks.
pub fn focus(id: &str) {
    crate::app::window().interaction_request_focus(id);
}

/// Run a command in this window using its current focus path and registered handlers.
/// Callable from handlers and tasks. For a popup that closes before invoking against restored
/// focus, use [`invoke_after_update`].
pub fn invoke(command: &str) -> Result<(), String> {
    crate::app::current().invoke(command)
}

/// Queues a command in this window after the current updates, modal focus restoration and
/// explicit [`focus`] requests settle. Commands run in order through the existing command
/// dispatcher, with each command's updates settled before the next. Callable from handlers
/// and tasks; closing the owning popup does not cancel it, while closing the window does.
///
/// An unhandled command reports `COMMAND_UNHANDLED`. At most 16 commands run in one update;
/// further queued commands are discarded with `COMMAND_QUEUE_LIMIT`, including commands
/// that keep queuing themselves. This does not wait for animation to finish or advance time.
#[track_caller]
pub fn invoke_after_update(command: &str) {
    crate::app::window().interaction_request_command(command, Location::caller());
}

/// Whether the text input currently has marked text. This is a handler-time observation,
/// not a reactive dependency. Suggestions can refuse a pointer choice while the system IME
/// still owns the composition, just as the runtime protects its keyboard navigation.
pub fn is_composing(id: &str) -> bool {
    let window = crate::app::window();
    let kernel = window.kernel.borrow();
    kernel.find(id).is_some_and(|node| !kernel.preedit(node).is_empty())
}

/// Lands everything in motion on its target now (Rune's `tx.settle`).
pub fn settle_motion() {
    let u = crate::app::window();
    {
        let mut k = u.kernel.borrow_mut();
        k.land_motion();
        k.set_instant(true);
    }
    u.land_paced();
}

/// Handles `command` for the whole app (when no focused element handles it): the keymap's
/// chords, `App::invoke` and menus run it. Lives as long as the current scope.
#[track_caller]
pub fn on_command(command: &str, f: impl Fn() + 'static) {
    let at = Location::caller();
    if !rt(|rt| rt.allows(crate::reactive::graph::Operation::Register, at)) {
        return;
    }
    let u = crate::app::window();
    let id = u.register(Handler::Press(Rc::new(f)));
    u.commands.borrow_mut().push((command.to_owned(), id));
    let me = u.clone();
    rt(|rt| rt.on_cleanup(Box::new(move || me.commands.borrow_mut().retain(|(_, h)| *h != id))));
}
