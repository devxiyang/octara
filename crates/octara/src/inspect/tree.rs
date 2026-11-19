//! Inspector projections of authoritative Kernel facts. No retained mirror of the tree.

use super::{Ancestor, ComponentInfo, Node, Picked, snake, tenth};
use crate::kernel::{HandlerKind, Kernel, Kind, NodeId, Role, State};

/// The semantic tree tools see: the nodes visible to accessibility, with presented bounds,
/// actions and source locations. Reads the retained Kernel directly.
pub fn tree(kernel: &Kernel) -> Node {
    tree_for_window(kernel, None)
}

pub(crate) fn tree_for_window(kernel: &Kernel, submit: Option<NodeId>) -> Node {
    inspect_node(kernel, kernel.root(), submit)
}

fn node_actions(kernel: &Kernel, id: NodeId, submit: Option<NodeId>) -> Vec<String> {
    let mut actions = Vec::new();
    if kernel.kind(id) == Kind::Link {
        actions.push("press".to_string());
    }
    for (kind, action) in [
        (HandlerKind::Press, "press"),
        (HandlerKind::Toggle, "toggle"),
        (HandlerKind::DoublePress, "double_press"),
        (HandlerKind::SecondaryPress, "secondary_press"),
        (HandlerKind::LongPress, "long_press"),
        (HandlerKind::Drag, "drag"),
        (HandlerKind::Input, "input"),
        (HandlerKind::NumericInput, "set_value"),
    ] {
        if kernel.handler(id, kind).is_some() {
            actions.push(action.to_string());
        }
    }
    if submit == Some(id) {
        actions.push("submit".into());
    }
    if kernel.handler(id, HandlerKind::NumericInput).is_some()
        && kernel.numeric_range(id).is_some_and(|range| range.step.is_some() && range.value.is_some())
    {
        actions.extend(["increment".into(), "decrement".into()]);
    }
    if kernel.kind(id) == Kind::TextInput && !actions.iter().any(|a| a == "input") {
        actions.push("input".into());
    }
    if kernel.is_focusable(id) {
        actions.push("focus".into());
    }
    if kernel.scrollable(id) != (false, false) {
        actions.push("scroll".into());
    }
    actions
}

fn inspect_one(kernel: &Kernel, id: NodeId, submit: Option<NodeId>) -> Node {
    let role = if id == kernel.root() { Role::Window } else { kernel.role(id) };
    let name = kernel.semantic_name(id).map(str::to_owned);
    let mut states = Vec::new();
    for (flag, name) in [
        (State::HOVER, "hovered"),
        (State::FOCUSED, "focused"),
        (State::FOCUS_VISIBLE, "focus_visible"),
        (State::PRESSED, "pressed"),
        (State::SELECTED, "selected"),
        (State::DISABLED, "disabled"),
        (State::DRAG_OVER, "drag_over"),
    ] {
        if kernel.state(id).contains(flag) {
            states.push(name.to_string());
        }
    }
    let (x, y, w, h) = kernel.shown_rect(id);
    let range = kernel.numeric_range(id);
    let relations = kernel.resolved_relations(id);
    let reference = |target: Option<NodeId>| target.and_then(|id| kernel.test_id(id)).map(str::to_owned);
    Node {
        target: super::target::node_target(kernel, id).expect("an inspected node is alive"),
        id: kernel.test_id(id).map(str::to_owned),
        role: snake(&format!("{role:?}")),
        name,
        description: kernel.description(id).map(str::to_owned),
        value: kernel.has_value(id).then(|| kernel.reported_value(id)),
        numeric_value: range.and_then(|r| r.value),
        min_numeric_value: range.map(|r| r.min),
        max_numeric_value: range.map(|r| r.max),
        numeric_value_step: range.and_then(|r| r.step),
        checked: kernel.checked(id),
        placeholder: kernel.placeholder(id).map(str::to_owned),
        required: kernel.required(id),
        invalid: kernel.invalid(id),
        expanded: kernel.expanded(id),
        active_descendant: reference(relations.active_descendant),
        labelled_by: reference(relations.labelled_by),
        controls: reference(relations.controls),
        states,
        bounds: [tenth(x), tenth(y), tenth(w), tenth(h)],
        actions: node_actions(kernel, id, submit),
        commands: kernel.command_names(id).map(str::to_owned).collect(),
        native: kernel.native(id).map(|v| crate::native::NativeInfo {
            kind: v.kind.clone(),
            slot: snake(&format!("{:?}", v.slot)),
            props: v.props.clone(),
        }),
        icon: kernel.icon(id).map(|icon| icon.name.clone()),
        source: kernel.origin(id).map(|l| format!("{}:{}:{}", crate::diagnostics::source_file(l), l.line(), l.column())),
        component: kernel.component(id).map(ComponentInfo::of),
        children: Vec::new(),
    }
}

fn inspect_node(kernel: &Kernel, id: NodeId, submit: Option<NodeId>) -> Node {
    let mut node = inspect_one(kernel, id, submit);
    if kernel.parent(id).is_some_and(|p| kernel.component(p) == kernel.component(id)) {
        node.component = None;
    }
    node.children =
        kernel.children(id).iter().copied().filter(|c| kernel.shown_to_a11y(*c)).map(|c| inspect_node(kernel, c, submit)).collect();
    node
}

/// The topmost element at `(x, y)` (logical window pixels), with what surrounds it.
pub fn pick(kernel: &Kernel, x: f32, y: f32) -> Option<Picked> {
    pick_for_window(kernel, x, y, None)
}

pub(crate) fn pick_for_window(kernel: &Kernel, x: f32, y: f32, submit: Option<NodeId>) -> Option<Picked> {
    let hit = kernel.hit_test(x, y)?;
    let node = inspect_one(kernel, hit, submit);
    let mut ancestors = Vec::new();
    let mut p = kernel.parent(hit);
    while let Some(id) = p {
        if id == kernel.root() {
            break;
        }
        if id != kernel.overlay_layer() {
            let one = inspect_one(kernel, id, submit);
            ancestors.push(Ancestor { role: one.role, id: one.id, source: one.source, component: one.component });
        }
        p = kernel.parent(id);
    }
    let describe = |role: &str, id: &Option<String>, source: &Option<String>| {
        let mut s = role.to_owned();
        if let Some(id) = id {
            s.push_str(&format!(" (id {id})"));
        }
        if let Some(src) = source {
            s.push_str(&format!(" at {src}"));
        }
        s
    };
    let mut text = describe(&node.role, &node.id, &node.source);
    if let Some(name) = &node.name {
        let name: String = name.chars().take(40).collect();
        text = text.replacen(&node.role, &format!("{} {name:?}", node.role), 1);
    }
    // The nearest ancestors that say where it is: an id, or another source line.
    let mut last = node.source.clone();
    let mut shown = 0;
    for a in &ancestors {
        if shown == 3 {
            break;
        }
        if a.id.is_some() || (a.source.is_some() && a.source != last) {
            text.push_str(&format!(", in {}", describe(&a.role, &a.id, &a.source)));
            last = a.source.clone();
            shown += 1;
        }
    }
    // The components it is part of, innermost first.
    let mut made: Vec<&crate::inspect::ComponentInfo> = Vec::new();
    for c in std::iter::once(&node.component).chain(ancestors.iter().map(|a| &a.component)).flatten() {
        if made.last() != Some(&c) {
            made.push(c);
        }
    }
    for (i, c) in made.iter().take(3).enumerate() {
        text.push_str(if i == 0 { "; made by " } else { " in " });
        text.push_str(&format!("{}() called at {}", c.name, c.called_at));
    }
    Some(Picked { node, ancestors, text })
}
