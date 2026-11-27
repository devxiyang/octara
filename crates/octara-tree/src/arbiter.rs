//! One guarded pointer contact state machine for all hosts.

mod gesture;
mod scrollbar;
mod text;
mod transfer;

pub use gesture::GestureIntent;
use gesture::{ContactGesture, HardwareGestures};
use scrollbar::ScrollbarCapture;
pub use scrollbar::{ScrollbarAxis, ScrollbarHit, ScrollbarKey, ScrollbarPart, ScrollbarScroll};
use std::{sync::Arc, time::Duration};
use text::{TextContact, TextPress};
pub use text::{TextPointerIntent, TextPointerTarget};
pub use transfer::PointerGeometry;
use transfer::{Grab, Transfer};

use octara_model::{
    geometry::{Point, Vec2},
    identity::{NodeId, TargetGuard},
    input::{Button, ContactTarget, DropEvent, DropHover, InteractionEvent, Key, Modifiers, PointerId, PointerKind},
    semantics::Role,
    style::State,
};

use crate::{HandlerId, HandlerKind, NodeFlags, NodeKind, RuntimeOp, Tree, focus::Arrow};

#[derive(Clone, Copy, Debug)]
pub struct Tuning {
    pub drag_threshold: f32,
    pub touch_drag_threshold: f32,
    pub double_click: Duration,
    pub double_tap: Duration,
    pub double_tap_slop: f32,
}

impl Default for Tuning {
    fn default() -> Self {
        Self {
            drag_threshold: 4.0,
            touch_drag_threshold: 8.0,
            double_click: Duration::from_millis(500),
            double_tap: Duration::from_millis(400),
            double_tap_slop: 16.0,
        }
    }
}

impl Tuning {
    fn record_press(self, previous: &mut Option<LastPress>, target: TargetGuard, kind: PointerKind, pos: Point, now: Duration) -> u32 {
        let (gap, slop) =
            if kind == PointerKind::Touch { (self.double_tap, self.double_tap_slop) } else { (self.double_click, self.drag_threshold) };
        let count = previous
            .filter(|last| {
                last.target == target
                    && last.kind == kind
                    && now.checked_sub(last.now).is_some_and(|elapsed| elapsed <= gap)
                    && distance(last.pos, pos) <= slop
            })
            .map_or(1, |last| last.count.saturating_add(1));
        *previous = Some(LastPress { target, kind, pos, now, count });
        count
    }
}

#[derive(Clone, Copy, Default)]
pub struct InteractionContext<'a> {
    /// The cycle's virtual monotonic time.
    pub now: Duration,
    pub keyboard_open: bool,
    pub pointer: Option<PointerGeometry<'a>>,
    pub text_selection: bool,
    pub scrollbar: Option<ScrollbarHit>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DragPhase {
    Start,
    Move,
    End,
    Cancel,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Drag {
    pub phase: DragPhase,
    /// Window-space pointer position.
    pub pos: Point,
    pub displacement: Vec2,
}

#[derive(Clone, Debug, PartialEq)]
pub enum InteractionPayload {
    None,
    Press { pos: Point, modifiers: Modifiers },
    Drag(Drag),
    Drop { event: DropEvent, keyboard: bool },
    DropHover { event: DropHover, keyboard: bool },
    TouchScroll { available: Vec2 },
    Overscroll(octara_model::input::OverscrollEvent),
}

#[derive(Clone, Debug, PartialEq)]
pub struct HandlerIntent {
    pub target: TargetGuard,
    pub kind: HandlerKind,
    pub payload: InteractionPayload,
    handler: Option<HandlerId>,
}

impl HandlerIntent {
    /// Check the allocation, modal barrier, and handler registration at delivery time.
    pub fn resolve(&self, tree: &Tree) -> Option<HandlerId> {
        let node = guarded_node(tree, self.target, tree.front_modal())?;
        let current = match &self.payload {
            InteractionPayload::Drop { event, keyboard } => {
                transfer::transfer_is_current(tree, node, event.source, &event.kind, &event.data)
                    && (!keyboard || transfer::keyboard_position(tree, event.source, node) == Some(event.position))
            }
            InteractionPayload::DropHover { event, keyboard } if event.position.is_some() => {
                transfer::transfer_is_current(tree, node, event.source, &event.payload.kind, &event.payload.data)
                    && (!keyboard || transfer::keyboard_position(tree, event.source, node) == event.position)
            }
            _ => true,
        };
        if !current {
            return None;
        }
        self.handler.filter(|&handler| tree.handler(node, self.kind) == Some(handler))
    }

    /// A synthetic inline link targeted by a default press.
    pub fn resolve_link(&self, tree: &Tree) -> Option<NodeId> {
        if self.handler.is_some() || !matches!(self.kind, HandlerKind::Press | HandlerKind::SecondaryPress) {
            return None;
        }
        let link = guarded_node(tree, self.target, tree.front_modal())?;
        tree.link(link).map(|_| link)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct FocusHandoff {
    hit: TargetGuard,
    action: Option<TargetGuard>,
    outside_dismissal: bool,
}

impl FocusHandoff {
    /// Resolve a guarded pointer focus request against the current scopes.
    pub fn resolve(self, tree: &Tree) -> Option<RuntimeOp> {
        let modal = tree.front_modal();
        let hit = scoped_hit(tree, self.hit, modal)?;
        let action = scoped_action(tree, self.action, modal)?;
        let preserve = hit.is_some_and(|id| {
            tree.logical_ancestors(id).any(|id| tree.node(id).is_some_and(|core| core.flags.contains(NodeFlags::PRESERVE_FOCUS)))
        });
        let node = action.filter(|&node| tree.focusable_in(node, modal));
        (!preserve && (!self.outside_dismissal || node.is_some())).then_some(RuntimeOp::SetFocus { node, visible: false })
    }

    pub fn focus_leave(self, tree: &Tree, previous: NodeId) -> Option<HandlerIntent> {
        if self.outside_dismissal {
            return None;
        }
        focus_leave(tree, previous, tree.focus().or_else(|| self.hit.node()))
    }
}

/// The active eligible layer whose logical scope a focus move has left.
pub fn focus_leave(tree: &Tree, previous: NodeId, next: Option<NodeId>) -> Option<HandlerIntent> {
    let layer = tree.front_layer(tree.front_modal(), |node, policy| {
        policy.dismiss_on_focus_leave && tree.handler(node, HandlerKind::Dismiss).is_some()
    })?;
    if !tree.is_in_interaction_scope(previous, layer) || next.is_some_and(|node| tree.is_in_interaction_scope(node, layer)) {
        return None;
    }
    handler_intent(tree, tree.guard(layer), HandlerKind::Dismiss, InteractionPayload::None)
}

/// A guarded activation that follows dismissal and focus handoff.
#[derive(Debug)]
pub struct ActivationIntent {
    hit: TargetGuard,
    action: TargetGuard,
    kind: ActivationKind,
    extends_link_selection: bool,
}

#[derive(Clone, Copy, Debug)]
enum ActivationKind {
    Pointer { button: Button, kind: PointerKind, pos: Point, modifiers: Modifiers, now: Duration },
    FocusGroup { group: TargetGuard },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TouchScrollPhase {
    Start,
    Move,
    End,
    Cancel,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TouchScrollStage {
    Pre,
    Post,
}

/// A captured touch delta. The cycle offers it to Pre, the viewport, and Post in order.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TouchScrollIntent {
    pub target: TargetGuard,
    pub contact: PointerId,
    pub phase: TouchScrollPhase,
    pub delta: Vec2,
    pub at: Duration,
}

impl TouchScrollIntent {
    pub fn handler(self, tree: &Tree, stage: TouchScrollStage, available: Vec2) -> Option<HandlerIntent> {
        if !self.moves() || available == Vec2::default() {
            return None;
        }
        let kind = match stage {
            TouchScrollStage::Pre => HandlerKind::TouchScrollPre,
            TouchScrollStage::Post => HandlerKind::TouchScrollPost,
        };
        handler_intent(tree, self.target, kind, InteractionPayload::TouchScroll { available })
    }

    /// Filter the requested viewport motion to the captured container's scroll axes.
    /// The cycle supplies the amount actually consumed using its current scroll bounds.
    pub fn viewport_delta(self, tree: &Tree, available: Vec2) -> Option<Vec2> {
        if !self.moves() {
            return None;
        }
        let node = guarded_node(tree, self.target, tree.front_modal())?;
        let (x, y) = scroll_axes(tree, node);
        Some(Vec2::new(if x { available.x } else { 0.0 }, if y { available.y } else { 0.0 }))
    }

    /// Offers the exact signed remainder to the next stage, bounding invalid consumption.
    pub fn remaining(available: Vec2, used: Vec2) -> Vec2 {
        fn axis(available: f32, used: f32) -> f32 {
            if available.is_finite() && used.is_finite() {
                available - used.clamp(available.min(0.0), available.max(0.0))
            } else {
                available
            }
        }
        Vec2::new(axis(available.x, used.x), axis(available.y, used.y))
    }

    fn moves(self) -> bool {
        matches!(self.phase, TouchScrollPhase::Start | TouchScrollPhase::Move)
    }
}

#[derive(Debug)]
pub enum InteractionIntent {
    Invoke(HandlerIntent),
    Gesture(GestureIntent),
    Focus(FocusHandoff),
    Activate(ActivationIntent),
    TextPointer(TextPointerIntent),
    TouchScroll(TouchScrollIntent),
    Scrollbar(ScrollbarScroll),
    WindowDrag { target: TargetGuard, maximize: bool },
}

/// Apply tree operations first, then settle each intent in order before resolving the next.
#[derive(Default)]
pub struct InteractionDecision {
    pub operations: Vec<RuntimeOp>,
    pub intents: Vec<InteractionIntent>,
}

impl InteractionDecision {
    fn invoke(&mut self, tree: &Tree, target: TargetGuard, kind: HandlerKind, payload: InteractionPayload) {
        if let Some(intent) = handler_intent(tree, target, kind, payload) {
            self.intents.push(InteractionIntent::Invoke(intent));
        }
    }

    fn dismiss(&mut self, tree: &Tree, layer: NodeId) {
        self.invoke(tree, tree.guard(layer), HandlerKind::Dismiss, InteractionPayload::None);
    }

    fn pointer_focus(&mut self, tree: &Tree, focus: Option<FocusHandoff>, dismissal: Option<NodeId>) {
        let locked = dismissal.is_some()
            && focus.and_then(|focus| focus.action).and_then(TargetGuard::node).is_some_and(|node| !tree.is_focusable(node));
        if let Some(layer) = dismissal.filter(|_| locked) {
            self.dismiss(tree, layer);
        }
        self.intents.extend(focus.map(InteractionIntent::Focus));
        if let Some(layer) = dismissal.filter(|_| !locked) {
            self.dismiss(tree, layer);
        }
    }

    fn state(&mut self, node: NodeId, state: State, on: bool) {
        self.operations.push(RuntimeOp::SetState { node, state, on });
    }
}

#[derive(Clone, Copy)]
enum ContactMode {
    Pressed,
    Dragging(TargetGuard),
    Transferring(TargetGuard),
    Scrolling(TargetGuard),
    Selecting,
    Spent,
}

struct Contact {
    id: PointerId,
    kind: PointerKind,
    button: Button,
    target: ContactTarget,
    pressed_feedback: Option<NodeId>,
    deferred_dismissal: Option<TargetGuard>,
    deferred_editor: Option<TargetGuard>,
    text: Option<TextContact>,
    extends_link_selection: bool,
    outside_modal: bool,
    down: Point,
    at: Point,
    mode: ContactMode,
    gesture: Option<ContactGesture>,
}

impl Contact {
    fn clear_press(&self, decision: &mut InteractionDecision) {
        if self.button == Button::Primary && matches!(self.mode, ContactMode::Pressed) {
            for node in self.target.action.and_then(TargetGuard::node).into_iter().chain(self.pressed_feedback) {
                decision.state(node, State::PRESSED, false);
            }
        }
    }

    fn drag(&self, tree: &Tree, decision: &mut InteractionDecision, target: TargetGuard, phase: DragPhase) {
        decision.invoke(
            tree,
            target,
            HandlerKind::Drag,
            InteractionPayload::Drag(Drag { phase, pos: self.at, displacement: displacement(self.at, self.down) }),
        );
    }

    fn scroll(&self, decision: &mut InteractionDecision, target: TargetGuard, phase: TouchScrollPhase, delta: Vec2, at: Duration) {
        decision.intents.push(InteractionIntent::TouchScroll(TouchScrollIntent { target, contact: self.id, phase, delta, at }));
    }

    fn is_current(&self, tree: &Tree, modal: Option<NodeId>) -> bool {
        match self.mode {
            ContactMode::Selecting => self.text.is_some_and(|text| text.target.resolve(tree).is_some()),
            ContactMode::Dragging(target) | ContactMode::Scrolling(target) | ContactMode::Transferring(target) => {
                guarded_node(tree, target, modal).is_some()
            }
            _ => {
                let hit_scope = if self.outside_modal { None } else { modal };
                scoped_hit(tree, self.target.hit, hit_scope).is_some() && scoped_action(tree, self.target.action, modal).is_some()
            }
        }
    }
}

#[derive(Clone, Copy)]
struct LastPress {
    target: TargetGuard,
    kind: PointerKind,
    pos: Point,
    now: Duration,
    count: u32,
}

pub struct InteractionArbiter {
    tuning: Tuning,
    contact: Option<Contact>,
    scrollbar: Option<ScrollbarCapture>,
    transfer: Option<Transfer>,
    last_press: Option<LastPress>,
    last_text_press: Option<TextPress>,
    last_tap: Option<LastPress>,
    hardware_gestures: HardwareGestures,
    window_drag_press: Option<(PointerId, LastPress)>,
    hover: Vec<NodeId>,
    active: bool,
}

impl Default for InteractionArbiter {
    fn default() -> Self {
        Self::new(Tuning::default())
    }
}

impl InteractionArbiter {
    pub fn new(tuning: Tuning) -> Self {
        Self {
            tuning,
            contact: None,
            scrollbar: None,
            transfer: None,
            last_press: None,
            last_text_press: None,
            last_tap: None,
            hardware_gestures: HardwareGestures::default(),
            window_drag_press: None,
            hover: Vec::new(),
            active: true,
        }
    }

    pub fn dragged_scrollbar(&self) -> Option<ScrollbarKey> {
        self.scrollbar.filter(|capture| capture.scale > 0.0).map(|capture| capture.key)
    }

    pub fn pointer_hover_active(&self) -> bool {
        self.active && !self.contact.as_ref().is_some_and(|contact| contact.kind == PointerKind::Touch)
    }

    /// The innermost interactive hover target with tooltip text.
    pub fn hovered_tooltip<'a>(&self, tree: &'a Tree, hit: Option<TargetGuard>, modal: Option<NodeId>) -> Option<(NodeId, &'a Arc<str>)> {
        if !self.active || self.contact.is_some() || self.scrollbar.is_some() {
            return None;
        }
        self.hover_path(tree, hit, modal).find_map(|node| {
            let text = tree.semantics(node)?.tooltip.as_ref()?;
            (!text.trim().is_empty()).then_some((node, text))
        })
    }

    fn hover_path<'a>(&self, tree: &'a Tree, hit: Option<TargetGuard>, modal: Option<NodeId>) -> impl Iterator<Item = NodeId> + 'a {
        let hit = hit.and_then(|target| guarded_node(tree, target, modal)).filter(|_| self.pointer_hover_active());
        std::iter::successors(hit, |&node| tree.logical_parent(node))
            .filter(move |&node| Some(node) == hit || tree.node(node).is_some_and(|core| !core.flags.contains(NodeFlags::HOVER_ALONE)))
    }

    pub fn window_moved(&mut self) {
        self.window_drag_press = None;
    }

    pub fn is_active(&self) -> bool {
        self.active
    }

    /// The live contact target that must stay resident until the gesture ends.
    pub fn captured_node(&self) -> Option<NodeId> {
        let contact = self.contact.as_ref()?;
        match contact.mode {
            ContactMode::Pressed => contact.target.action.unwrap_or(contact.target.hit).node(),
            ContactMode::Selecting => contact.text.and_then(|text| text.target.guard().node()),
            ContactMode::Dragging(target) | ContactMode::Scrolling(target) | ContactMode::Transferring(target) => target.node(),
            ContactMode::Spent => contact.gesture.and(contact.target.hit.node()),
        }
    }

    pub fn update_hover(&mut self, tree: &Tree, hit: Option<TargetGuard>) -> InteractionDecision {
        let mut decision = InteractionDecision::default();
        let next: Vec<_> = self.hover_path(tree, hit, tree.front_modal()).collect();
        for &node in &self.hover {
            if !next.contains(&node) {
                decision.state(node, State::HOVER, false);
            }
        }
        for &node in &next {
            if !self.hover.contains(&node) {
                decision.state(node, State::HOVER, true);
            }
        }
        self.hover = next;
        decision
    }

    /// Clear captures that have been retired, hidden, disabled, or blocked.
    pub fn reconcile(&mut self, tree: &Tree, now: Duration) -> InteractionDecision {
        let mut decision = InteractionDecision::default();
        let modal = tree.front_modal();
        self.hardware_gestures.reconcile(tree, modal);
        if self.last_tap.is_some_and(|tap| guarded_node(tree, tap.target, modal).is_none()) {
            self.last_tap = None;
        }
        self.hover.retain(|&node| {
            let active = tree.interactive_in(node, modal);
            if !active {
                decision.state(node, State::HOVER, false);
            }
            active
        });
        self.validate_transfer(tree, &mut decision);
        if self.scrollbar.is_some_and(|capture| capture.resolve(tree).is_none()) {
            self.scrollbar = None;
        }
        if self.contact.as_ref().is_some_and(|contact| !contact.is_current(tree, tree.front_modal())) {
            self.cancel(tree, &mut decision, now);
        }
        decision
    }

    pub fn handle(&mut self, tree: &Tree, event: InteractionEvent, context: InteractionContext<'_>) -> InteractionDecision {
        let mut decision = self.reconcile(tree, context.now);
        if let InteractionEvent::WindowActive(active) = event {
            self.active = active;
            if !active {
                self.hardware_gestures = HardwareGestures::default();
                self.cancel(tree, &mut decision, context.now);
                decision.operations.extend(self.update_hover(tree, None).operations);
            }
            return decision;
        }
        if !self.active {
            return decision;
        }
        if let Some(capture) = self.scrollbar
            && capture.handle(event, &mut self.scrollbar, &mut decision)
        {
            return decision;
        }
        if let InteractionEvent::PointerDown { id, kind, button: Button::Primary, pos, .. } = event
            && kind != PointerKind::Touch
            && self.contact.is_none()
            && self.scrollbar.is_none()
            && let Some(hit) = context.scrollbar
            && scrollbar::scrollbar_target(tree, hit.key.node)
        {
            self.end_transfer(tree, &mut decision, None);
            self.last_press = None;
            self.last_text_press = None;
            self.last_tap = None;
            self.window_drag_press = None;
            self.scrollbar = Some(ScrollbarCapture::begin(tree, id, pos, hit, &mut decision));
            return decision;
        }

        match event {
            InteractionEvent::PointerDown { id, kind, target, pos, button, mods } => {
                if self.contact.is_none() {
                    self.end_transfer(tree, &mut decision, None);
                }
                let contact = Contact {
                    id,
                    kind,
                    button,
                    target,
                    pressed_feedback: None,
                    deferred_dismissal: None,
                    deferred_editor: None,
                    text: None,
                    extends_link_selection: false,
                    outside_modal: false,
                    down: pos,
                    at: pos,
                    mode: ContactMode::Pressed,
                    gesture: None,
                };
                self.down(tree, &mut decision, contact, mods, context);
            }
            InteractionEvent::PointerMove { id, pos, .. } if finite(pos) => {
                if self.window_drag_press.is_some_and(|(pointer, press)| pointer == id && distance(press.pos, pos) > self.slop(press.kind))
                {
                    self.window_drag_press = None;
                }
                if let Some(mut contact) = self.contact.take_if(|contact| contact.id == id) {
                    let previous = contact.at;
                    self.moved(tree, &mut decision, &mut contact, pos, context.now);
                    self.move_contact_gesture(&mut decision, &mut contact, previous);
                    self.contact = Some(contact);
                    self.pointer_transfer(tree, &mut decision, id, pos, context.pointer);
                }
            }
            InteractionEvent::PointerUp { id, target, pos, mods } => {
                if self.window_drag_press.is_some_and(|(pointer, press)| pointer == id && distance(press.pos, pos) > self.slop(press.kind))
                {
                    self.window_drag_press = None;
                }
                if !finite(pos) {
                    if self.contact.as_ref().is_some_and(|contact| contact.id == id) {
                        self.cancel(tree, &mut decision, context.now);
                    }
                } else if let Some(mut contact) = self.contact.take_if(|contact| contact.id == id) {
                    if matches!(contact.mode, ContactMode::Transferring(_)) {
                        self.pointer_transfer(tree, &mut decision, id, pos, context.pointer);
                        self.end_transfer(tree, &mut decision, Some(mods));
                        return decision;
                    }
                    match contact.mode {
                        ContactMode::Pressed if contact.text.is_some() && contact.kind != PointerKind::Touch => {
                            self.moved(tree, &mut decision, &mut contact, pos, context.now);
                        }
                        ContactMode::Pressed if distance(contact.down, pos) > self.slop(contact.kind) => {
                            self.last_press = None;
                            self.last_text_press = None;
                            contact.clear_press(&mut decision);
                            contact.mode = ContactMode::Spent;
                        }
                        ContactMode::Dragging(_) | ContactMode::Scrolling(_) | ContactMode::Selecting => {
                            self.moved(tree, &mut decision, &mut contact, pos, context.now);
                        }
                        _ => contact.at = pos,
                    }
                    self.up(tree, &mut decision, &contact, target, mods, context.now);
                    self.finish_contact_gesture(tree, &mut decision, &contact, target, pos, context.now);
                }
            }
            InteractionEvent::PointerCancel { id } => {
                if self.window_drag_press.is_some_and(|(pointer, _)| pointer == id) {
                    self.window_drag_press = None;
                }
                if self.contact.as_ref().is_some_and(|contact| contact.id == id) {
                    self.cancel(tree, &mut decision, context.now);
                }
            }
            InteractionEvent::PointerLeave => self.clear_receiver(tree, &mut decision),
            InteractionEvent::Key(key) => {
                if let Some(transfer) = self.transfer_key(tree, key) {
                    decision.operations.extend(transfer.operations);
                    decision.intents.extend(transfer.intents);
                } else {
                    self.key(tree, &mut decision, key.key, key.modifiers);
                }
            }
            _ => {}
        }
        decision
    }

    fn down(
        &mut self,
        tree: &Tree,
        decision: &mut InteractionDecision,
        mut contact: Contact,
        mods: Modifiers,
        context: InteractionContext<'_>,
    ) {
        if let Some(active) = &mut self.contact {
            if contact.id != active.id && contact.kind == PointerKind::Touch && active.kind == PointerKind::Touch {
                active.cancel_gesture(decision);
                self.last_tap = None;
            }
            return;
        }
        if !finite(contact.down) {
            return;
        }
        let previous_drag = self.window_drag_press.take();
        let Some(action) = contact_action(tree, contact.target, contact.button, None) else { return };
        let modal = tree.front_modal();
        let hit = contact.target.hit.node();
        let blocked = modal.is_some_and(|modal| hit.is_none_or(|node| !tree.is_in_modal_scope(node, modal)));
        if !blocked && scoped_action(tree, action, modal).is_none() {
            return;
        }
        contact.outside_modal = blocked;
        contact.target.action = if blocked { None } else { action };
        let text_target = (context.text_selection && contact.button == Button::Primary && !blocked)
            .then(|| TextPointerTarget::at(tree, hit?, action.and_then(TargetGuard::node)))
            .flatten();
        contact.extends_link_selection = contact.button == Button::Primary
            && mods.contains(Modifiers::SHIFT)
            && action.and_then(TargetGuard::node).is_some_and(|node| {
                tree.link(node).is_some() && tree.region_of(node).and_then(|region| tree.doc_selection(region)).is_some()
            });
        contact.text = self.text_press(text_target, contact.kind, contact.down, mods, context.now);

        let dismissal = tree.outside_dismissal(hit, modal);
        if contact.kind == PointerKind::Touch && context.keyboard_open {
            contact.deferred_dismissal = dismissal.map(|layer| tree.guard(layer));
        }
        if blocked && contact.deferred_dismissal.is_none() {
            if let Some(layer) = dismissal {
                decision.dismiss(tree, layer);
            }
            return;
        }

        if contact.button == Button::Primary
            && !blocked
            && action.is_none()
            && contact.text.is_none()
            && let Some(region) = hit.and_then(|hit| window_drag_region(tree, hit, modal))
        {
            if let Some(layer) = dismissal {
                decision.dismiss(tree, layer);
            }
            let target = tree.guard(region);
            let maximize = contact.kind != PointerKind::Touch
                && previous_drag.is_some_and(|(pointer, press)| {
                    pointer == contact.id
                        && press.target == target
                        && press.kind == contact.kind
                        && context.now.checked_sub(press.now).is_some_and(|elapsed| elapsed <= self.tuning.double_click)
                        && distance(press.pos, contact.down) <= self.slop(contact.kind)
                });
            if !maximize {
                self.window_drag_press =
                    Some((contact.id, LastPress { target, kind: contact.kind, pos: contact.down, now: context.now, count: 1 }));
            }
            self.last_press = None;
            self.last_tap = None;
            decision.intents.push(InteractionIntent::WindowDrag { target, maximize });
            return;
        }

        let mut focus = None;
        if contact.button == Button::Primary && !blocked {
            let action = contact.target.action.and_then(TargetGuard::node);
            contact.pressed_feedback = hit.and_then(|hit| {
                tree.logical_ancestors(hit).find(|&node| {
                    Some(node) != action
                        && tree.interactive_in(node, modal)
                        && tree.node(node).is_some_and(|core| core.flags.contains(NodeFlags::INTERACTION_STATE))
                })
            });
            for node in action.into_iter().chain(contact.pressed_feedback) {
                decision.state(node, State::PRESSED, true);
            }
            if contact.kind == PointerKind::Touch && context.keyboard_open && action.is_none_or(|node| tree.editor(node).is_none()) {
                contact.deferred_editor = tree.focus().filter(|&node| tree.editor(node).is_some()).map(|node| tree.guard(node));
            }
            if contact.deferred_editor.is_none() {
                focus =
                    Some(FocusHandoff { hit: contact.target.hit, action: contact.target.action, outside_dismissal: dismissal.is_some() });
            }
        }
        decision.pointer_focus(tree, focus, dismissal.filter(|_| contact.deferred_dismissal.is_none()));
        if context.text_selection && contact.button == Button::Primary && !blocked && contact.kind != PointerKind::Touch {
            decision
                .intents
                .push(InteractionIntent::TextPointer(contact.text.map_or(TextPointerIntent::Clear, |text| text.down(contact.down))));
        }
        contact.begin_gesture(tree, context.now);
        self.contact = Some(contact);
    }

    fn moved(&mut self, tree: &Tree, decision: &mut InteractionDecision, contact: &mut Contact, pos: Point, now: Duration) {
        let previous = std::mem::replace(&mut contact.at, pos);
        if previous == pos {
            return;
        }
        let delta = displacement(previous, pos);
        match contact.mode {
            ContactMode::Dragging(target) => contact.drag(tree, decision, target, DragPhase::Move),
            ContactMode::Scrolling(target) => contact.scroll(decision, target, TouchScrollPhase::Move, delta, now),
            ContactMode::Selecting => contact.text.expect("selecting contact").moved(decision, pos),
            ContactMode::Pressed if distance(contact.down, pos) > self.slop(contact.kind) => {
                self.last_press = None;
                self.last_text_press = None;
                contact.clear_press(decision);
                contact.mode = ContactMode::Spent;
                if contact.button != Button::Primary || contact.outside_modal {
                    return;
                }
                if let Some(text) = contact.text.filter(|_| contact.kind != PointerKind::Touch) {
                    contact.mode = ContactMode::Selecting;
                    text.moved(decision, pos);
                    return;
                }
                let modal = tree.front_modal();
                let drag = contact
                    .target
                    .action
                    .filter(|target| target.node().is_some_and(|node| tree.handler(node, HandlerKind::Drag).is_some()));
                // A resize handle owns its drag before a transferable ancestor can grab it.
                let local_drag = drag.and_then(|target| target.node());
                let transferable = |node| tree.drag_drop(node).is_some_and(|slot| slot.source.is_some());
                if let Some(source) = contact.target.hit.node().and_then(|hit| {
                    std::iter::successors(Some(hit), |&node| tree.logical_parent(node))
                        .find(|&node| tree.interactive_in(node, modal) && (transferable(node) || Some(node) == local_drag))
                }) && transferable(source)
                    && self.grab(tree, decision, source, Grab::Pointer(contact.id))
                {
                    contact.mode = ContactMode::Transferring(tree.guard(source));
                    return;
                }
                if let Some(target) = drag {
                    contact.mode = ContactMode::Dragging(target);
                    contact.drag(tree, decision, target, DragPhase::Start);
                } else if contact.kind == PointerKind::Touch {
                    let delta = displacement(contact.down, pos);
                    if let Some(node) = scroll_target(tree, contact.target.hit.node(), delta) {
                        let target = tree.guard(node);
                        contact.mode = ContactMode::Scrolling(target);
                        contact.scroll(decision, target, TouchScrollPhase::Start, delta, now);
                    }
                }
            }
            _ => {}
        }
    }

    fn up(
        &self,
        tree: &Tree,
        decision: &mut InteractionDecision,
        contact: &Contact,
        target: Option<ContactTarget>,
        modifiers: Modifiers,
        now: Duration,
    ) {
        contact.clear_press(decision);
        match contact.mode {
            ContactMode::Dragging(target) => return contact.drag(tree, decision, target, DragPhase::End),
            ContactMode::Scrolling(target) => return contact.scroll(decision, target, TouchScrollPhase::End, Vec2::default(), now),
            ContactMode::Spent | ContactMode::Transferring(_) | ContactMode::Selecting => return,
            ContactMode::Pressed => {}
        }

        let modal = tree.front_modal();
        let release_scope = if contact.target.action.is_some() { modal } else { None };
        let Some(released) = target.map_or(Some(None), |target| contact_action(tree, target, contact.button, release_scope)) else {
            return;
        };
        if contact.target.action.is_some() && contact.target.action != released {
            return;
        }
        let hit = target.and_then(|target| target.hit.node());
        let dismissal =
            contact.deferred_dismissal.and_then(TargetGuard::node).filter(|&layer| tree.outside_dismissal(hit, modal) == Some(layer));

        let deferred_editor = contact.deferred_editor.is_some_and(|editor| editor.node() == tree.focus() && tree.contains_target(editor));
        let deferred_dismissal = dismissal.is_some() && contact.button == Button::Primary && !contact.outside_modal;
        let focus = (deferred_editor || deferred_dismissal).then_some(FocusHandoff {
            hit: contact.target.hit,
            action: contact.target.action,
            outside_dismissal: dismissal.is_some(),
        });
        decision.pointer_focus(tree, focus, dismissal);
        if contact.kind == PointerKind::Touch
            && let Some(text) = contact.text
        {
            decision.intents.push(InteractionIntent::TextPointer(text.down(contact.down)));
        }
        if let Some(action) = contact.target.action {
            decision.intents.push(InteractionIntent::Activate(ActivationIntent {
                hit: contact.target.hit,
                action,
                kind: ActivationKind::Pointer { button: contact.button, kind: contact.kind, pos: contact.at, modifiers, now },
                extends_link_selection: contact.extends_link_selection,
            }));
        }
    }

    fn cancel(&mut self, tree: &Tree, decision: &mut InteractionDecision, now: Duration) {
        self.scrollbar = None;
        self.last_press = None;
        self.last_text_press = None;
        self.last_tap = None;
        self.window_drag_press = None;
        self.end_transfer(tree, decision, None);
        if let Some(mut contact) = self.contact.take() {
            contact.clear_press(decision);
            contact.cancel_gesture(decision);
            match contact.mode {
                ContactMode::Dragging(target) => contact.drag(tree, decision, target, DragPhase::Cancel),
                ContactMode::Scrolling(target) => contact.scroll(decision, target, TouchScrollPhase::Cancel, Vec2::default(), now),
                _ => {}
            }
        }
    }

    fn slop(&self, kind: PointerKind) -> f32 {
        if kind == PointerKind::Touch { self.tuning.touch_drag_threshold } else { self.tuning.drag_threshold }
    }

    fn key(&self, tree: &Tree, decision: &mut InteractionDecision, key: Key, modifiers: Modifiers) {
        if key == Key::Escape && modifiers.is_empty() {
            if let Some(layer) = tree.front_layer(tree.front_modal(), |node, policy| {
                policy.dismiss_on_escape && tree.handler(node, HandlerKind::Dismiss).is_some()
            }) {
                decision.dismiss(tree, layer);
            }
            return;
        }
        if key == Key::Tab && (modifiers - Modifiers::SHIFT).is_empty() {
            if let Some(node) = tree.tab_target(!modifiers.contains(Modifiers::SHIFT)) {
                decision.operations.push(RuntimeOp::SetFocus { node: Some(node), visible: true });
            }
            return;
        }
        let Some(focused) = tree.focus().filter(|&node| tree.editor(node).is_none() && modifiers.is_empty()) else { return };
        if matches!(key, Key::Enter | Key::Space) {
            if key == Key::Space && tree.link(focused).is_some() {
                return;
            }
            let target = tree.guard(focused);
            decision.invoke(tree, target, HandlerKind::Press, InteractionPayload::None);
            decision.invoke(tree, target, HandlerKind::Toggle, InteractionPayload::None);
            return;
        }
        let arrow = match key {
            Key::Up => Arrow::Up,
            Key::Down => Arrow::Down,
            Key::Left => Arrow::Left,
            Key::Right => Arrow::Right,
            Key::Home => Arrow::Home,
            Key::End => Arrow::End,
            _ => return,
        };
        if let Some(node) = tree.group_step(focused, arrow) {
            decision.operations.push(RuntimeOp::SetFocus { node: Some(node), visible: true });
            if let Some(group) = tree.group_activation(focused, node) {
                let target = tree.guard(node);
                decision.intents.push(InteractionIntent::Activate(ActivationIntent {
                    hit: target,
                    action: target,
                    kind: ActivationKind::FocusGroup { group: tree.guard(group) },
                    extends_link_selection: false,
                }));
            }
        }
    }

    /// Finalize a qualified release against the settled tree. The previous action
    /// allocation is checked again before handlers are obtained.
    pub fn resolve_activation(&mut self, tree: &Tree, activation: ActivationIntent) -> Vec<HandlerIntent> {
        let modal = tree.front_modal();
        let Some(node) = guarded_node(tree, activation.action, modal).filter(|_| scoped_hit(tree, activation.hit, modal).is_some()) else {
            self.last_press = None;
            return Vec::new();
        };
        let intentions = |kinds: &[HandlerKind], payload: InteractionPayload| {
            kinds
                .iter()
                .filter_map(|&kind| handler_intent(tree, activation.action, kind, payload.clone()))
                .filter(|intent| !(activation.extends_link_selection && intent.kind == HandlerKind::Press && intent.handler.is_none()))
                .collect()
        };
        match activation.kind {
            ActivationKind::FocusGroup { group } => {
                if group.node().is_some_and(|group| tree.focus_group_of(node) == Some(group))
                    && tree.contains_target(group)
                    && tree.focus() == Some(node)
                {
                    intentions(&[HandlerKind::Press, HandlerKind::Toggle], InteractionPayload::None)
                } else {
                    Vec::new()
                }
            }
            ActivationKind::Pointer { button: Button::Primary, kind, pos, modifiers, now } => {
                let count = self.tuning.record_press(&mut self.last_press, activation.action, kind, pos, now);
                let kinds = [HandlerKind::Press, HandlerKind::Toggle, HandlerKind::DoublePress];
                intentions(&kinds[..if count == 2 { 3 } else { 2 }], InteractionPayload::Press { pos, modifiers })
            }
            ActivationKind::Pointer { button: Button::Secondary, pos, modifiers, .. } => {
                self.last_press = None;
                intentions(&[HandlerKind::SecondaryPress], InteractionPayload::Press { pos, modifiers })
            }
            ActivationKind::Pointer { .. } => Vec::new(),
        }
    }
}

fn finite(pos: Point) -> bool {
    pos.x.is_finite() && pos.y.is_finite()
}

fn displacement(to: Point, from: Point) -> Vec2 {
    Vec2::new(to.x - from.x, to.y - from.y)
}

fn distance(from: Point, to: Point) -> f32 {
    let delta = displacement(to, from);
    delta.x.hypot(delta.y)
}

fn guarded_node(tree: &Tree, guard: TargetGuard, modal: Option<NodeId>) -> Option<NodeId> {
    guard.node().filter(|&node| tree.contains_target(guard) && tree.interactive_in(node, modal))
}

fn window_drag_region(tree: &Tree, hit: NodeId, modal: Option<NodeId>) -> Option<NodeId> {
    let mut region = None;
    for node in std::iter::successors(Some(hit), |&node| tree.physical_parent(node)) {
        let core = tree.node(node)?;
        if region.is_none() && core.flags.contains(NodeFlags::WINDOW_DRAG_REGION) && tree.interactive_in(node, modal) {
            region = Some(node);
        }
        if core.flags.contains(NodeFlags::FOCUSABLE)
            || matches!(core.kind, NodeKind::TextInput | NodeKind::Link | NodeKind::Native)
            || tree.semantics(node).is_some_and(|semantics| matches!(semantics.role, Some(Role::Button | Role::Link)))
            || tree.handlers(node).any(|(kind, _)| {
                matches!(
                    kind,
                    HandlerKind::Press | HandlerKind::Toggle | HandlerKind::SecondaryPress | HandlerKind::DoublePress | HandlerKind::Drag
                )
            })
            || tree.drag_drop(node).is_some_and(|drag| drag.source.is_some() || drag.target.is_some())
        {
            return None;
        }
    }
    region
}

fn scoped_hit(tree: &Tree, hit: TargetGuard, modal: Option<NodeId>) -> Option<Option<NodeId>> {
    if hit.node().is_some() {
        return guarded_node(tree, hit, modal).map(Some);
    }
    (modal.is_none() && tree.contains_target(hit)).then_some(None)
}

fn scoped_action(tree: &Tree, action: Option<TargetGuard>, modal: Option<NodeId>) -> Option<Option<NodeId>> {
    match action {
        Some(action) => guarded_node(tree, action, modal).map(Some),
        None => Some(None),
    }
}

fn contact_action(tree: &Tree, target: ContactTarget, button: Button, modal: Option<NodeId>) -> Option<Option<TargetGuard>> {
    let hit = scoped_hit(tree, target.hit, modal)?;
    let action = scoped_action(tree, target.action, modal)?;
    if let Some(action) = action
        && !hit.is_some_and(|hit| tree.is_logically_owned_by(hit, action))
    {
        return None;
    }
    if button == Button::Secondary {
        let receiver = hit.and_then(|node| {
            tree.logical_ancestors(node).find(|&id| {
                tree.interactive_in(id, modal)
                    && (tree.link(id).is_some()
                        || tree.handler(id, HandlerKind::SecondaryPress).is_some()
                        || tree.handler(id, HandlerKind::ContextMenu).is_some())
            })
        });
        return Some(receiver.map(|node| tree.guard(node)));
    }
    Some(target.action)
}

fn handler_intent(tree: &Tree, target: TargetGuard, kind: HandlerKind, payload: InteractionPayload) -> Option<HandlerIntent> {
    let node = guarded_node(tree, target, tree.front_modal())?;
    let handler = tree.handler(node, kind);
    if handler.is_none() {
        let synthetic = match kind {
            HandlerKind::Press => tree.link(node).is_some(),
            HandlerKind::SecondaryPress => std::iter::successors(Some(node), |&node| tree.logical_parent(node)).any(|node| {
                [HandlerKind::Link, HandlerKind::ContextMenu, HandlerKind::SecondaryPress]
                    .into_iter()
                    .any(|kind| tree.handler(node, kind).is_some())
            }),
            _ => false,
        };
        if !synthetic {
            return None;
        }
    }
    Some(HandlerIntent { target, kind, payload, handler })
}

fn scroll_axes(tree: &Tree, node: NodeId) -> (bool, bool) {
    tree.style(node).map_or((false, false), |slot| (slot.layout.overflow_x.scrolls(), slot.layout.overflow_y.scrolls()))
}

fn scroll_target(tree: &Tree, hit: Option<NodeId>, delta: Vec2) -> Option<NodeId> {
    let hit = hit?;
    let modal = tree.front_modal();
    let nearest = |x, y| {
        tree.ancestor_matching(hit, |node| {
            let (sx, sy) = scroll_axes(tree, node);
            (sx && x || sy && y) && tree.interactive_in(node, modal)
        })
    };
    let horizontal = delta.x.abs() > delta.y.abs();
    nearest(horizontal, !horizontal).or_else(|| nearest(delta.x != 0.0, delta.y != 0.0))
}
