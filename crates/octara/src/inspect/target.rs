//! Opaque wire targets and request validation. A request binds one mounted allocation at
//! admission and keeps it through host waits. Operation-specific checks run before delivery.

use super::{Completion, Delivery, PointerStep, Reply, Request, ScrollResult, Window, codes};
use crate::app::{Button, Chord, Event, GuardTarget, InputEvent, Key, Modifiers, Phase, PointerEvent, PointerKind, TargetGuard};
use crate::identity::TreeIdentity;
use crate::kernel::{Kernel, NameError, NodeId};
use slotmap::{Key as _, KeyData};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

// Reserved synthetic ids keep cancellation away from a person's mouse or touch.
static NEXT_CLICK_POINTER: AtomicU64 = AtomicU64::new(1 << 63);

fn token(identity: TreeIdentity, node: Option<NodeId>) -> String {
    let mut token = format!("ot1:{:032x}:{:016x}:{:016x}", identity.namespace, identity.window, identity.generation);
    if let Some(node) = node {
        use std::fmt::Write;
        write!(token, ":{:016x}", node.data().as_ffi()).expect("writing to a string");
    }
    token
}

pub(crate) fn window_target(kernel: &Kernel) -> String {
    token(kernel.identity(), None)
}

pub(crate) fn node_target(kernel: &Kernel, node: NodeId) -> Option<String> {
    kernel.is_mounted(node).then(|| token(kernel.identity(), Some(node)))
}

pub(crate) fn paint_target(kernel: &Kernel, node: NodeId) -> Option<String> {
    kernel.is_alive(node).then(|| token(kernel.identity(), Some(node)))
}

/// A request bound to its routed window and, when applicable, an operation-local target.
/// Hosts retain the guard through input phases and waits, including for name-only requests.
pub struct ParsedRequest {
    pub window: Window,
    pub request: Request,
    pub guard: Option<TargetGuard>,
}

enum Binding<'a> {
    Node(Option<&'a str>),
    Focus,
    Window,
}

fn binding(request: &Request) -> Binding<'_> {
    match request {
        Request::TapAt { .. } => Binding::Node(None),
        Request::Tap { id }
        | Request::DoubleTap { id }
        | Request::LongPress { id }
        | Request::Drag { id, .. }
        | Request::Scroll { id, .. }
        | Request::SecondaryTap { id }
        | Request::Focus { id }
        | Request::Hover { id, .. }
        | Request::SetValue { id, .. } => Binding::Node(id.as_deref()),
        Request::Type { id: Some(id), .. } => Binding::Node(Some(id)),
        Request::Expect(expect) if expect.id.is_some() || expect.exists.is_some() || expect.wants_node() => {
            Binding::Node(expect.id.as_deref())
        }
        Request::Type { id: None, .. } | Request::Key { .. } => Binding::Focus,
        _ => Binding::Window,
    }
}

fn failure(code: &'static crate::diagnostics::Code, message: impl Into<String>, delivery: Delivery) -> Reply {
    Reply { delivery: Some(delivery), ..Reply::fail(code, message) }
}

fn stale(delivery: Delivery) -> Reply {
    failure(&codes::STALE_TARGET, "the target no longer identifies this operation's window, node or focus; inspect again", delivery)
}

fn bad(message: impl Into<String>) -> Box<Reply> {
    Box::new(failure(&codes::BAD_REQUEST, message, Delivery::Refused))
}

pub(super) fn name_failure(id: &str, error: NameError) -> Reply {
    let code = match error {
        NameError::Missing => &codes::NO_SUCH_ELEMENT,
        NameError::Ambiguous(_) => &codes::AMBIGUOUS_TARGET,
    };
    failure(code, error.message(id), Delivery::Refused)
}

struct ObservedTarget {
    tree: TreeIdentity,
    node: Option<NodeId>,
}

fn parse_token(encoded: &str) -> Option<ObservedTarget> {
    let mut fields = encoded.split(':');
    if fields.next()? != "ot1" {
        return None;
    }
    let tree = TreeIdentity {
        namespace: u128::from_str_radix(fields.next()?, 16).ok()?,
        window: u64::from_str_radix(fields.next()?, 16).ok()?,
        generation: u64::from_str_radix(fields.next()?, 16).ok()?,
    };
    let node = match fields.next() {
        Some(field) => Some(NodeId::from(KeyData::from_ffi(u64::from_str_radix(field, 16).ok()?))),
        None => None,
    };
    // Round-trip checks the canonical widths and rules out noncanonical slot generations.
    (fields.next().is_none() && token(tree, node) == encoded).then_some(ObservedTarget { tree, node })
}

fn parse_wire(window: &Window, line: &str) -> Result<(Window, Request, Option<ObservedTarget>), Box<Reply>> {
    let mut value: serde_json::Value = serde_json::from_str(line).map_err(|e| bad(format!("{e}: {line}")))?;
    let object = value.as_object_mut().ok_or_else(|| bad("a request must be a JSON object"))?;
    let observed = object
        .remove("target")
        .map(|target| {
            let encoded = target.as_str().ok_or_else(|| bad("target must be an opaque string"))?;
            parse_token(encoded).ok_or_else(|| bad("target is not a valid opaque target"))
        })
        .transpose()?;
    let named = object.remove("window");
    let request: Request = serde_json::from_value(value).map_err(|e| bad(format!("{e}: {line}")))?;
    let routed = match named {
        None => (!window.is_closed()).then(|| window.clone()),
        Some(number) => {
            let number = number.as_u64().ok_or_else(|| bad("window must be a window number"))?;
            window.app().window(crate::app::WindowId::from_number(number))
        }
    };
    let routed = routed.ok_or_else(|| {
        if observed.is_some() { Box::new(stale(Delivery::Refused)) } else { bad("the requested window is closed or does not exist") }
    })?;
    Ok((routed, request, observed))
}

fn bind_request(window: &Window, request: &Request, observed: Option<ObservedTarget>) -> Result<Option<TargetGuard>, Box<Reply>> {
    let binding = binding(request);
    let target = if let Some(observed) = observed {
        let target = match (binding, observed.node) {
            (Binding::Node(_), Some(node)) => GuardTarget::Node(node),
            (Binding::Focus, Some(node)) => GuardTarget::Focus(Some(node)),
            (Binding::Window, None) => GuardTarget::Window,
            _ => return Err(bad("the target must identify the requested window or node, according to the operation")),
        };
        if window.kernel().identity() != observed.tree {
            return Err(Box::new(stale(Delivery::Refused)));
        }
        target
    } else {
        match binding {
            Binding::Window => return Ok(None),
            Binding::Focus => GuardTarget::Focus(window.focused_node()),
            Binding::Node(None) if matches!(request, Request::TapAt { .. }) => return Err(bad("tap_at requires an observed node target")),
            Binding::Node(None) => return Err(bad("this operation requires id or target")),
            Binding::Node(Some(id)) => match window.kernel().named_node(id) {
                Ok(node) => GuardTarget::Node(node),
                Err(NameError::Missing) if matches!(request, Request::Expect(_)) => return Ok(None),
                Err(error) => return Err(Box::new(name_failure(id, error))),
            },
        }
    };
    let guard = TargetGuard::new(window, target);
    validate_guard(window, request, &guard)?;
    Ok(Some(guard))
}

fn validate_guard(window: &Window, request: &Request, guard: &TargetGuard) -> Result<(), Box<Reply>> {
    let matches = match (binding(request), guard.target) {
        (Binding::Window, GuardTarget::Window) | (Binding::Focus, GuardTarget::Focus(_)) => true,
        (Binding::Node(id), GuardTarget::Node(node)) => id.is_none_or(|id| {
            let k = window.kernel();
            k.is_mounted(node) && k.test_id(node) == Some(id)
        }),
        _ => false,
    };
    if matches && guard.is_current(window) { Ok(()) } else { Err(Box::new(stale(Delivery::Refused))) }
}

/// Parses a request and binds its window and target without executing it. An explicit node
/// token selects the allocation directly. A supplied id asserts that allocation's name.
pub fn parse_guarded_request(window: &Window, line: &str) -> Result<ParsedRequest, Box<Reply>> {
    let (window, request, observed) = parse_wire(window, line)?;
    if let Some(reply) = super::failure_refusal(&window, &request) {
        return Err(Box::new(reply));
    }
    let guard = bind_request(&window, &request, observed)?;
    Ok(ParsedRequest { window, request, guard })
}

/// Parses a name-based request and its optional window for callers that execute it with
/// the Request API. Requests carrying an observed target require the guarded parser.
pub fn parse_request(window: &Window, line: &str) -> Result<(Window, Request), Box<Reply>> {
    let (window, request, observed) = parse_wire(window, line)?;
    if observed.is_some() {
        return Err(bad("this host must retain target guards with parse_guarded_request and handle_guarded"));
    }
    if matches!(request, Request::TapAt { .. }) {
        return Err(bad("tap_at requires an observed node target"));
    }
    if matches!(binding(&request), Binding::Node(None)) {
        return Err(bad("this operation requires id"));
    }
    Ok((window, request))
}

/// Executes one operation while retaining the original target through focus, pointer phases
/// and key-to-text fallback. A failed later phase reports that earlier input was delivered.
pub fn handle_guarded(window: &mut Window, request: Request, guard: Option<&TargetGuard>) -> Reply {
    let mut steps = Vec::new();
    let mut reply = super::request_boundary(window, request, |window, request| {
        let bound;
        let guard = match guard {
            Some(guard) => {
                if let Err(reply) = validate_guard(window, &request, guard) {
                    return *reply;
                }
                Some(guard)
            }
            None => {
                bound = match bind_request(window, &request, None) {
                    Ok(bound) => bound,
                    Err(reply) => return *reply,
                };
                bound.as_ref()
            }
        };
        let inspection = matches!(request, Request::Inspect);
        let input =
            !matches!(request, Request::Inspect | Request::Issues | Request::Pick { .. } | Request::Expect(_) | Request::Capture { .. });
        let action = input && !matches!(request, Request::Settle { .. });
        let mut reply = super::handle_inner(window, request, guard, &mut steps);
        if input
            && (guard.is_some() || action && reply.ok)
            && reply.delivery.is_none()
            && reply.code.as_deref() != Some(codes::APP_FAILED.id)
        {
            reply.delivery =
                Some(if reply.code.as_deref() == Some(codes::BAD_REQUEST.id) { Delivery::Refused } else { Delivery::Delivered });
        }
        if inspection || reply.delivery == Some(Delivery::Delivered) {
            reply.completion = Some(Completion::observe(window));
        }
        reply
    });
    reply.pointer = steps;
    reply
}

pub(super) fn semantic(window: &mut Window, guard: &TargetGuard, event: impl FnOnce(String) -> Event) -> Reply {
    let node = guard.node().expect("node op was bound");
    let id = window.kernel().test_id(node).unwrap_or("element").to_owned();
    match window.dispatch_to(event(id), node) {
        Ok(()) => Reply { delivery: Some(Delivery::Delivered), ..Reply::ok() },
        Err(error) => failure(&codes::STEP_FAILED, error, Delivery::Refused),
    }
}

pub(super) fn scroll(window: &mut Window, guard: &TargetGuard, dx: f32, dy: f32) -> Reply {
    if !dx.is_finite() || !dy.is_finite() {
        return failure(&codes::BAD_REQUEST, "scroll requires finite deltas", Delivery::Refused);
    }
    if !window.frame_is_settled() {
        return failure(&codes::NOT_SETTLED, "the window has no current settled geometry", Delivery::Refused);
    }
    let node = guard.node().expect("scroll was bound to a viewport");
    let (before_x, before_y) = {
        let kernel = window.kernel();
        if let Err(error) = kernel.check_interaction_scope(node, window.top_modal()) {
            return failure(&codes::STEP_FAILED, format!("the scroll target {}", error.message(&kernel)), Delivery::Refused);
        }
        kernel.scroll_offset(node)
    };
    let mut reply = semantic(window, guard, |id| Event::Scroll(id, dx, dy));
    if reply.ok {
        // Layout can retire the viewport. Only sample the allocation we scrolled.
        if !guard.is_current(window) {
            return stale(Delivery::Delivered);
        }
        if !window.frame_is_settled() {
            return failure(&codes::NOT_SETTLED, "the scroll was delivered but layout has not settled", Delivery::Delivered);
        }
        let (x, y) = window.kernel().scroll_offset(node);
        reply.scroll = Some(ScrollResult { offset: [x, y], delta: [x - before_x, y - before_y] });
    }
    reply
}

pub(super) fn type_text(window: &mut Window, guard: &TargetGuard, text: &str) -> Reply {
    let mut delivery = Delivery::Refused;
    if matches!(guard.target, GuardTarget::Node(_)) {
        let reply = semantic(window, guard, Event::Focus);
        if !reply.ok {
            return reply;
        }
        delivery = Delivery::Delivered;
        if !guard.is_focused(window) {
            return stale(delivery);
        }
    }
    if window.text_field().is_none() {
        return failure(&codes::STEP_FAILED, "no text field has focus", delivery);
    }
    window.type_text(text);
    Reply { delivery: Some(Delivery::Delivered), ..Reply::ok() }
}

pub(super) fn key(window: &mut Window, guard: &TargetGuard, chord: &str) -> Reply {
    let event = match Chord::parse(chord) {
        Ok(chord) => chord.event(window.app().target_platform()),
        Err(error) => return failure(&codes::STEP_FAILED, error, Delivery::Refused),
    };
    let handled = window.event_guarded(InputEvent::Key(event), Some(guard));
    if !handled && !guard.is_current(window) {
        return stale(Delivery::Delivered);
    }
    let text = match event.key {
        Key::Char(c) if !handled && !event.modifiers.intersects(Modifiers::CTRL | Modifiers::META) => Some(c.to_string()),
        Key::Space if !handled && window.text_field().is_some() => Some(" ".into()),
        _ => None,
    };
    if let Some(text) = text
        && guard.node().is_some()
    {
        if !guard.is_focused(window) {
            return stale(Delivery::Delivered);
        }
        window.type_text(&text);
    }
    Reply { delivery: Some(Delivery::Delivered), ..Reply::ok() }
}

/// Validates a gesture phase and reads the bound instance's currently reachable centre.
pub fn guarded_point(window: &Window, guard: &TargetGuard, delivery: Delivery) -> Result<(f32, f32), Box<Reply>> {
    if !guard.is_current(window) {
        return Err(Box::new(stale(delivery)));
    }
    if !window.frame_is_settled() {
        return Err(Box::new(failure(&codes::NOT_SETTLED, "the window has no current settled geometry", delivery)));
    }
    let node = guard.node().ok_or_else(|| bad("a pointer gesture requires a node target"))?;
    window.activation_point(node).map_err(|why| Box::new(failure(&codes::STEP_FAILED, why, delivery)))
}

/// Delivers one raw pointer event and collects any evidence from its recognizer phase.
pub fn dispatch_pointer(window: &mut Window, event: PointerEvent, steps: &mut Vec<PointerStep>) {
    window.observed_pointer(event, steps);
}

pub(super) fn tap_at(
    window: &mut Window,
    guard: &TargetGuard,
    (x, y): (f32, f32),
    drawing_revision: Option<u64>,
    steps: &mut Vec<PointerStep>,
) -> Reply {
    let mut delivery = Delivery::Refused;
    if !x.is_finite() || !y.is_finite() {
        return failure(&codes::BAD_REQUEST, "tap_at requires finite window coordinates", delivery);
    }
    let target = guard.node().expect("tap_at bound an observed node");
    let pointer = NEXT_CLICK_POINTER.fetch_add(1, Ordering::Relaxed);
    let held = Some((pointer, PointerKind::Mouse));
    let event = |phase| PointerEvent { id: pointer, ..PointerEvent::mouse(phase, x, y) };
    for phase in [Phase::Move, Phase::Down, Phase::Up] {
        let expected_held = if phase == Phase::Up { held } else { None };
        let refusal = if !guard.is_current(window) {
            Some(stale(delivery))
        } else if !window.frame_is_settled() {
            Some(failure(&codes::NOT_SETTLED, "the window has no current settled geometry", delivery))
        } else if phase == Phase::Move && drawing_revision.is_some_and(|revision| window.published_revision() != Some(revision)) {
            Some(failure(&codes::STEP_FAILED, "the requested drawing is no longer current; inspect again", delivery))
        } else if window.held_pointer() != expected_held {
            Some(failure(&codes::STEP_FAILED, "another pointer interrupted the fixed-coordinate click", delivery))
        } else if !window.tap_at_matches(target, x, y) {
            Some(failure(&codes::STEP_FAILED, "the fixed point does not reach its observed press target", delivery))
        } else if window.observed_pointer_at(event(phase), guard, steps) {
            None
        } else if !guard.is_current(window) {
            Some(stale(delivery))
        } else if !window.frame_is_settled() {
            Some(failure(&codes::NOT_SETTLED, "the window has no current settled geometry", delivery))
        } else {
            Some(failure(&codes::STEP_FAILED, "the pointer phase could not complete for the observed target", delivery))
        };
        if let Some(reply) = refusal {
            if window.held_pointer() == held {
                window.observed_pointer_at(event(Phase::Cancel), guard, steps);
            }
            return reply;
        }
        delivery = Delivery::Delivered;
    }
    Reply { delivery: Some(delivery), ..Reply::ok() }
}

pub(super) fn pointer(
    window: &mut Window,
    guard: &TargetGuard,
    kind: PointerKind,
    button: Button,
    double: bool,
    long: bool,
    steps: &mut Vec<PointerStep>,
) -> Reply {
    if window.held_pointer().is_some() {
        return failure(&codes::STEP_FAILED, "a pointer is already held", Delivery::Refused);
    }
    let pointer = NEXT_CLICK_POINTER.fetch_add(1, Ordering::Relaxed);
    let mut delivered = Delivery::Refused;
    let event = |phase, (x, y)| PointerEvent { id: pointer, kind, button, ..PointerEvent::mouse(phase, x, y) };
    let cancel = |window: &mut Window, at, steps: &mut Vec<PointerStep>| {
        if !window.is_closed() && window.held_pointer() == Some((pointer, kind)) {
            dispatch_pointer(window, event(Phase::Cancel, at), steps);
        }
    };
    for click in 0..if double { 2 } else { 1 } {
        if click != 0 {
            window.app().advance(Duration::from_millis(50));
        }
        let mut at = match guarded_point(window, guard, delivered) {
            Ok(at) => at,
            Err(reply) => return *reply,
        };
        if kind != PointerKind::Touch {
            dispatch_pointer(window, event(Phase::Move, at), steps);
            delivered = Delivery::Delivered;
            at = match guarded_point(window, guard, delivered) {
                Ok(at) => at,
                Err(reply) => return *reply,
            };
        }
        if window.held_pointer().is_some() {
            return failure(&codes::STEP_FAILED, "another pointer interrupted the gesture before its down", delivered);
        }
        dispatch_pointer(window, event(Phase::Down, at), steps);
        delivered = Delivery::Delivered;
        if long {
            window.app().advance(window.tuning().long_press);
            if window.pointer_long_pressed(pointer) {
                cancel(window, at, steps);
                return Reply { delivery: Some(Delivery::Delivered), ..Reply::ok() };
            }
        }
        let at = match guarded_point(window, guard, delivered) {
            Ok(at) => at,
            Err(reply) => {
                cancel(window, at, steps);
                return *reply;
            }
        };
        dispatch_pointer(window, event(Phase::Up, at), steps);
    }
    Reply { delivery: Some(Delivery::Delivered), ..Reply::ok() }
}

/// The headless equivalent of the host's realtime drag job. Even without an explicit token,
/// every phase keeps the original node and cancels only its unique synthetic mouse pointer.
pub(super) fn drag(window: &mut Window, guard: &TargetGuard, dx: f32, dy: f32, steps: &mut Vec<PointerStep>) -> Reply {
    static NEXT_POINTER: AtomicU64 = AtomicU64::new(1 << 62);
    let refused = Delivery::Refused;
    if !dx.is_finite() || !dy.is_finite() {
        return failure(&codes::BAD_REQUEST, "drag requires finite dx and dy", refused);
    }
    if window.held_pointer().is_some() {
        return failure(&codes::STEP_FAILED, "a pointer is already held", refused);
    }
    let hover = match guarded_point(window, guard, refused) {
        Ok(point) => point,
        Err(reply) => return *reply,
    };
    let finite_endpoint = |(x, y): (f32, f32)| (x + dx).is_finite() && (y + dy).is_finite();
    if !finite_endpoint(hover) {
        return failure(&codes::BAD_REQUEST, "drag endpoint must be finite", refused);
    }
    let delivered = Delivery::Delivered;
    let current = |window: &Window| guarded_point(window, guard, delivered);
    let pointer = NEXT_POINTER.fetch_add(1, Ordering::Relaxed);
    let event = |phase, (x, y)| PointerEvent { id: pointer, ..PointerEvent::mouse(phase, x, y) };
    dispatch_pointer(window, event(Phase::Move, hover), steps);
    let start = match current(window) {
        Ok(point) => point,
        Err(reply) => return *reply,
    };
    if !finite_endpoint(start) {
        return failure(&codes::BAD_REQUEST, "drag endpoint must be finite", delivered);
    }
    if window.held_pointer().is_some() {
        return failure(&codes::STEP_FAILED, "another pointer interrupted the drag before its down", delivered);
    }
    let cancel = |window: &mut Window, steps: &mut Vec<PointerStep>| {
        if !window.is_closed() && window.held_pointer() == Some((pointer, PointerKind::Mouse)) {
            dispatch_pointer(window, event(Phase::Cancel, start), steps);
        }
    };
    dispatch_pointer(window, event(Phase::Down, start), steps);
    for step in 1..=4 {
        window.app().advance(Duration::from_millis(16));
        if let Err(reply) = current(window) {
            cancel(window, steps);
            return *reply;
        }
        if window.held_pointer() != Some((pointer, PointerKind::Mouse)) {
            return failure(&codes::STEP_FAILED, "the drag pointer was cancelled or replaced", delivered);
        }
        let fraction = step as f32 / 4.0;
        dispatch_pointer(window, event(Phase::Move, (start.0 + dx * fraction, start.1 + dy * fraction)), steps);
    }
    if let Err(reply) = current(window) {
        cancel(window, steps);
        return *reply;
    }
    if window.held_pointer() != Some((pointer, PointerKind::Mouse)) {
        return failure(&codes::STEP_FAILED, "the drag pointer was cancelled or replaced before release", delivered);
    }
    dispatch_pointer(window, event(Phase::Up, (start.0 + dx, start.1 + dy)), steps);
    Reply { delivery: Some(delivered), ..Reply::ok() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wire_targets_keep_their_canonical_encoding() {
        let mut identity = TreeIdentity::new(0x1234, 7);
        assert_eq!(token(identity, None), "ot1:00000000000000000000000000001234:0000000000000007:0000000000000000");
        identity.advance();
        let node = NodeId::from(slotmap::KeyData::from_ffi(0x0000000100000007));
        assert_eq!(token(identity, Some(node)), "ot1:00000000000000000000000000001234:0000000000000007:0000000000000001:0000000100000007");
        let parsed = parse_token(&token(identity, Some(node))).unwrap();
        assert_eq!(parsed.tree, identity);
        assert_eq!(parsed.node, Some(node));
        assert!(parse_token("ot1:00000000000000000000000000001234:0000000000000007:0000000000000001:0000000000000007").is_none());
    }
}
