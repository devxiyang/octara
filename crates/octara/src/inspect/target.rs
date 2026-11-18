//! Opaque wire targets and request validation. App owns operation-local guards; Kernel owns
//! tree lifetime. This adapter encodes their identity without a token table or property revision.

use super::{Delivery, Reply, Request, Window, codes};
use crate::app::{Button, Chord, Event, InputEvent, Key, Modifiers, Phase, PointerEvent, PointerKind, TargetGuard};
use crate::identity::TreeIdentity;
use crate::kernel::{Kernel, NodeId};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

fn token(identity: TreeIdentity, node: Option<NodeId>) -> String {
    let mut token = format!("ot1:{:032x}:{:016x}:{:016x}", identity.namespace, identity.window, identity.generation);
    if let Some(node) = node {
        use slotmap::Key;
        use std::fmt::Write;
        write!(token, ":{:016x}", node.data().as_ffi()).expect("writing to a string");
    }
    token
}

pub(crate) fn window_target(kernel: &Kernel) -> String {
    token(kernel.identity(), None)
}

pub(crate) fn node_target(kernel: &Kernel, node: NodeId) -> Option<String> {
    kernel.is_alive(node).then(|| token(kernel.identity(), Some(node)))
}

/// A validated request with its routed window and optional operation-local target guard.
/// Hosts must retain the guard when they wait between input phases.
pub struct ParsedRequest {
    pub window: Window,
    pub request: Request,
    pub guard: Option<TargetGuard>,
}

enum Binding<'a> {
    Node(&'a str),
    Focus,
    Window,
}

fn binding(request: &Request) -> Binding<'_> {
    match request {
        Request::Tap { id }
        | Request::DoubleTap { id }
        | Request::LongPress { id }
        | Request::Drag { id, .. }
        | Request::SecondaryTap { id }
        | Request::Focus { id }
        | Request::Hover { id, .. }
        | Request::SetValue { id, .. }
        | Request::Type { id: Some(id), .. } => Binding::Node(id),
        Request::Expect(expect) if expect.id.is_some() => Binding::Node(expect.id.as_deref().expect("an id")),
        Request::Type { id: None, .. } | Request::Key { .. } => Binding::Focus,
        _ => Binding::Window,
    }
}

fn node_for(window: &Window, binding: &Binding<'_>) -> Option<NodeId> {
    match binding {
        Binding::Node(id) => match window.kernel().find_all(id).as_slice() {
            [node] => Some(*node),
            _ => None,
        },
        Binding::Focus => window.focused_node(),
        Binding::Window => None,
    }
}

fn failure(code: &'static crate::diagnostics::Code, message: impl Into<String>, delivery: Delivery) -> Reply {
    Reply { delivery: Some(delivery), ..Reply::fail(code, message) }
}

fn stale(delivery: Delivery) -> Reply {
    failure(&codes::STALE_TARGET, "the target no longer identifies this operation's window, node or focus; inspect again", delivery)
}

/// Checks the token's canonical shape without interpreting its opaque identity fields.
fn token_is_node(token: &str) -> Option<bool> {
    let parts: Vec<_> = token.split(':').collect();
    if !(parts.len() == 4 || parts.len() == 5) || parts[0] != "ot1" {
        return None;
    }
    for (part, len) in parts[1..].iter().zip([32, 16, 16, 16]) {
        if part.len() != len || !part.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) {
            return None;
        }
    }
    Some(parts.len() == 5)
}

/// Parse `target` and `window` before dispatch, preserving a validated guard for every phase.
/// Missing `target` retains the test-id protocol. Null, non-string or malformed targets are
/// errors; a well-formed retired identity is `STALE_TARGET`, including a closed window.
pub fn parse_guarded_request(window: &Window, line: &str) -> Result<ParsedRequest, Box<Reply>> {
    let bad = |message| Box::new(failure(&codes::BAD_REQUEST, message, Delivery::Refused));
    let mut value: serde_json::Value = serde_json::from_str(line).map_err(|e| bad(format!("{e}: {line}")))?;
    let object = value.as_object_mut().ok_or_else(|| bad("a request must be a JSON object".into()))?;
    let target = object
        .remove("target")
        .map(|target| {
            let token = target.as_str().ok_or_else(|| bad("`target` must be a nonempty opaque string".into()))?;
            let node = token_is_node(token).ok_or_else(|| bad("`target` is not a valid opaque target".into()))?;
            Ok::<_, Box<Reply>>((token.to_owned(), node))
        })
        .transpose()?;
    let named = object.remove("window");
    let request: Request = serde_json::from_value(value).map_err(|e| bad(format!("{e}: {line}")))?;
    let routed = match named {
        None => (!window.is_closed()).then(|| window.clone()),
        Some(number) => {
            let number = number.as_u64().ok_or_else(|| bad("`window` must be a window number".into()))?;
            window.app().window(crate::app::WindowId::from_number(number))
        }
    };
    let routed = routed.ok_or_else(|| {
        if target.is_some() { Box::new(stale(Delivery::Refused)) } else { bad("the requested window is closed or does not exist".into()) }
    })?;
    let guard = if let Some((token, node_token)) = target {
        let binding = binding(&request);
        if node_token == matches!(binding, Binding::Window) {
            return Err(bad(
                "this operation requires a window target or its specific node target; `select` accepts only window targets".into()
            ));
        }
        let node = node_for(&routed, &binding);
        let expected = if node_token { node.and_then(|node| node_target(&routed.kernel(), node)) } else { Some(routed.target()) };
        if expected.as_deref() != Some(token.as_str()) {
            return Err(Box::new(stale(Delivery::Refused)));
        }
        Some(TargetGuard::new(&routed, node, matches!(binding, Binding::Focus)))
    } else {
        None
    };
    Ok(ParsedRequest { window: routed, request, guard })
}

/// Executes one operation while retaining its original target through focus, pointer phases
/// and key-to-text fallback. A failed later phase reports that earlier input was delivered.
pub fn handle_guarded(window: &mut Window, request: Request, guard: Option<&TargetGuard>) -> Reply {
    super::request_boundary(window, request, |window, request| handle_guarded_inner(window, request, guard))
}

fn handle_guarded_inner(window: &mut Window, request: Request, guard: Option<&TargetGuard>) -> Reply {
    let Some(guard) = guard else { return super::handle_inner(window, request) };
    let binding = binding(&request);
    if !guard.is_current(window) || node_for(window, &binding) != guard.node || matches!(binding, Binding::Focus) != guard.focus {
        return stale(Delivery::Refused);
    }
    match request {
        Request::Type { id, text } => {
            let focused = id.is_some();
            if let Some(id) = id {
                if let Err(error) = window.dispatch(Event::Focus(id)) {
                    return failure(&codes::STEP_FAILED, error, Delivery::Delivered);
                }
                if !guard.is_focused(window) {
                    return stale(Delivery::Delivered);
                }
            }
            if window.text_field().is_none() {
                return failure(
                    &codes::STEP_FAILED,
                    "the guarded node is not a text field",
                    if focused { Delivery::Delivered } else { Delivery::Refused },
                );
            }
            window.type_text(&text);
            Reply { delivery: Some(Delivery::Delivered), ..Reply::ok() }
        }
        Request::Key { chord } => {
            let event = match Chord::parse(&chord) {
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
            if let Some(text) = text {
                if !guard.is_focused(window) {
                    return stale(Delivery::Delivered);
                }
                window.type_text(&text);
            }
            Reply { delivery: Some(Delivery::Delivered), ..Reply::ok() }
        }
        Request::Tap { id } => pointer(window, guard, &id, PointerKind::Mouse, Button::Primary, false, false),
        Request::DoubleTap { id } => pointer(window, guard, &id, PointerKind::Mouse, Button::Primary, true, false),
        Request::LongPress { id } => pointer(window, guard, &id, PointerKind::Touch, Button::Primary, false, true),
        Request::Drag { id, dx, dy } => drag(window, &id, dx, dy, Some(guard)),
        Request::SecondaryTap { id } => pointer(window, guard, &id, PointerKind::Mouse, Button::Secondary, false, false),
        request => {
            let input = !matches!(
                request,
                Request::Inspect | Request::Issues | Request::Pick { .. } | Request::Expect(_) | Request::Capture { .. }
            );
            let mut reply = super::handle_inner(window, request);
            if input && reply.delivery.is_none() && reply.code.as_deref() != Some(codes::APP_FAILED.id) {
                reply.delivery =
                    Some(if reply.code.as_deref() == Some(codes::BAD_REQUEST.id) { Delivery::Refused } else { Delivery::Delivered });
            }
            reply
        }
    }
}

fn pointer(window: &mut Window, guard: &TargetGuard, id: &str, kind: PointerKind, button: Button, double: bool, long: bool) -> Reply {
    // Reserved synthetic ids keep cancellation away from a person's mouse or touch. Only
    // the current operation's id is ever cancelled; there is no pointer/token registry.
    static NEXT_POINTER: AtomicU64 = AtomicU64::new(1 << 63);
    if window.held_pointer().is_some() {
        return failure(&codes::STEP_FAILED, "a pointer is already held", Delivery::Refused);
    }
    let pointer = NEXT_POINTER.fetch_add(1, Ordering::Relaxed);
    let mut delivered = Delivery::Refused;
    let event = |phase, (x, y)| PointerEvent { id: pointer, kind, button, ..PointerEvent::mouse(phase, x, y) };
    let cancel = |window: &mut Window, at| {
        if !window.is_closed() && window.held_pointer() == Some((pointer, kind)) {
            window.event(InputEvent::Pointer(event(Phase::Cancel, at)));
        }
    };
    let point = |window: &Window, delivered| -> Result<(f32, f32), Box<Reply>> {
        if !guard.is_current(window) {
            return Err(Box::new(stale(delivered)));
        }
        match window.pointer_target(id) {
            Ok((node, at)) if Some(node) == guard.node => Ok(at),
            Ok(_) => Err(Box::new(stale(delivered))),
            Err(error) => Err(Box::new(failure(&codes::STEP_FAILED, error, delivered))),
        }
    };
    for click in 0..if double { 2 } else { 1 } {
        if click != 0 {
            window.app().advance(Duration::from_millis(50));
        }
        let at = match point(window, delivered) {
            Ok(at) => at,
            Err(reply) => return *reply,
        };
        if kind != PointerKind::Touch {
            window.event(InputEvent::Pointer(event(Phase::Move, at)));
            delivered = Delivery::Delivered;
        }
        let at = match point(window, delivered) {
            Ok(at) => at,
            Err(reply) => return *reply,
        };
        window.event(InputEvent::Pointer(event(Phase::Down, at)));
        delivered = Delivery::Delivered;
        if long {
            window.app().advance(window.tuning().long_press);
            if window.pointer_long_pressed(pointer) {
                cancel(window, at);
                return Reply { delivery: Some(Delivery::Delivered), ..Reply::ok() };
            }
        }
        let at = match point(window, delivered) {
            Ok(at) => at,
            Err(reply) => {
                cancel(window, at);
                return *reply;
            }
        };
        window.event(InputEvent::Pointer(event(Phase::Up, at)));
    }
    Reply { delivery: Some(Delivery::Delivered), ..Reply::ok() }
}

/// The headless equivalent of the host's realtime drag job. Even without an explicit token,
/// every phase keeps the original node and cancels only its unique synthetic mouse pointer.
pub(super) fn drag(window: &mut Window, id: &str, dx: f32, dy: f32, guard: Option<&TargetGuard>) -> Reply {
    static NEXT_POINTER: AtomicU64 = AtomicU64::new(1 << 62);
    let refused = Delivery::Refused;
    if !dx.is_finite() || !dy.is_finite() {
        return failure(&codes::BAD_REQUEST, "drag requires finite dx and dy", refused);
    }
    if window.held_pointer().is_some() {
        return failure(&codes::STEP_FAILED, "a pointer is already held", refused);
    }
    if !window.kernel().exists(id) {
        return failure(&codes::NO_SUCH_ELEMENT, format!("no element has test id `{id}`"), refused);
    }
    let (node, start) = match window.pointer_target(id) {
        Ok(target) => target,
        Err(error) => return failure(&codes::STEP_FAILED, error, refused),
    };
    if !(start.0 + dx).is_finite() || !(start.1 + dy).is_finite() {
        return failure(&codes::BAD_REQUEST, "drag endpoint must be finite", refused);
    }
    let current = |window: &Window, delivery| -> Result<(), Box<Reply>> {
        if window.is_closed() {
            return Err(Box::new(failure(&codes::STEP_FAILED, "the target window closed during the drag", delivery)));
        }
        if guard.is_some_and(|guard| !guard.is_current(window) || guard.node() != Some(node)) {
            return Err(Box::new(stale(delivery)));
        }
        match window.pointer_target(id) {
            Ok((found, _)) if found == node => {
                if window.kernel().state(node).contains(crate::kernel::State::DISABLED) {
                    Err(Box::new(failure(&codes::STEP_FAILED, "the drag target is disabled", delivery)))
                } else {
                    Ok(())
                }
            }
            Ok(_) => Err(Box::new(failure(
                if guard.is_some() { &codes::STALE_TARGET } else { &codes::STEP_FAILED },
                "the drag target was replaced",
                delivery,
            ))),
            Err(error) => Err(Box::new(failure(&codes::STEP_FAILED, error, delivery))),
        }
    };
    if let Err(reply) = current(window, refused) {
        return *reply;
    }
    let pointer = NEXT_POINTER.fetch_add(1, Ordering::Relaxed);
    let event = |phase, (x, y)| PointerEvent { id: pointer, ..PointerEvent::mouse(phase, x, y) };
    let cancel = |window: &mut Window| {
        if !window.is_closed() && window.held_pointer() == Some((pointer, PointerKind::Mouse)) {
            window.event(InputEvent::Pointer(event(Phase::Cancel, start)));
        }
    };
    let delivered = Delivery::Delivered;
    for phase in [Phase::Move, Phase::Down] {
        if let Err(reply) = current(window, if phase == Phase::Move { refused } else { delivered }) {
            cancel(window);
            return *reply;
        }
        if window.held_pointer().is_some() {
            return failure(&codes::STEP_FAILED, "another pointer interrupted the drag before its down", delivered);
        }
        window.event(InputEvent::Pointer(event(phase, start)));
    }
    for step in 1..=4 {
        window.app().advance(Duration::from_millis(16));
        if let Err(reply) = current(window, delivered) {
            cancel(window);
            return *reply;
        }
        if window.held_pointer() != Some((pointer, PointerKind::Mouse)) {
            return failure(&codes::STEP_FAILED, "the drag pointer was cancelled or replaced", delivered);
        }
        let fraction = step as f32 / 4.0;
        window.event(InputEvent::Pointer(event(Phase::Move, (start.0 + dx * fraction, start.1 + dy * fraction))));
    }
    if let Err(reply) = current(window, delivered) {
        cancel(window);
        return *reply;
    }
    if window.held_pointer() != Some((pointer, PointerKind::Mouse)) {
        return failure(&codes::STEP_FAILED, "the drag pointer was cancelled or replaced before release", delivered);
    }
    window.event(InputEvent::Pointer(event(Phase::Up, (start.0 + dx, start.1 + dy))));
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
    }
}
