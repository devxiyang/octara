//! Rules over the settled window, evaluated in retained tree order.

use octara::{
    runtime::{ReactiveWorld, WindowRuntime},
    tree::{NodeFlags, NodeKind, Tree, tables::SemanticsSlot},
};
use octara_model::{
    diagnostics::{Severity, codes},
    identity::NodeId,
    semantics::{Role, TextClip},
};
use std::{
    collections::{BTreeMap, HashMap},
    sync::Arc,
};

pub use octara_model::audit::{AuditFinding, AuditNode, AuditProvenance};

/// Inspect a deterministic preorder slice of settled nodes and append findings.
/// A rule registered in build context receives only the nodes selecting that rule.
pub trait AuditRule: Send + Sync {
    fn check(&self, nodes: &[AuditNode<'_>], findings: &mut Vec<AuditFinding>);
}

#[derive(Clone, Copy, Debug, Default)]
pub struct ClippedText;

impl AuditRule for ClippedText {
    fn check(&self, nodes: &[AuditNode<'_>], findings: &mut Vec<AuditFinding>) {
        let code = &codes::CLIPPED_TEXT;
        for node in nodes.iter().filter(|node| node.shown) {
            let message = match node.text_clip {
                Some(TextClip::Ellipsis) => "Text is truncated with an ellipsis.".into(),
                Some(TextClip::LineClamp(lines)) => format!("Text is truncated at its {lines}-line limit."),
                None => continue,
            };
            findings.push(node.finding(code.id, code.severity, message));
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct MissingSemantics;

impl AuditRule for MissingSemantics {
    fn check(&self, nodes: &[AuditNode<'_>], findings: &mut Vec<AuditFinding>) {
        let code = &codes::MISSING_SEMANTICS;
        findings.extend(
            nodes
                .iter()
                .filter(|node| node.authored_interactive && exposes_semantics(node) && node.accessible_name.is_none())
                .map(|node| node.finding(code.id, code.severity, code.summary)),
        );
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct DeadFocusTarget;

impl AuditRule for DeadFocusTarget {
    fn check(&self, nodes: &[AuditNode<'_>], findings: &mut Vec<AuditFinding>) {
        let code = &codes::DEAD_FOCUS_TARGET;
        for node in nodes.iter().filter(|node| node.authored_focusable || node.focused) {
            let message = if !node.shown || node.hidden_from_accessibility {
                "The focus target is hidden."
            } else if node.inert {
                "The focus target is inert."
            } else if !node.in_modal_scope {
                "The focus target is outside the active modal."
            } else if node.geometry.is_some_and(|geometry| geometry.rect.is_empty()) {
                "The focus target has empty bounds."
            } else {
                continue;
            };
            let severity = if node.focused { Severity::Error } else { code.severity };
            findings.push(node.finding(code.id, severity, message));
        }
    }
}

pub(crate) fn run(window: &WindowRuntime, world: &ReactiveWorld, rules: &[Arc<dyn AuditRule>]) -> Vec<AuditFinding> {
    let tree = window.tree();
    let modal = tree.front_modal();
    let clips: BTreeMap<_, _> = window.clipped_text().into_iter().collect();
    let mut nodes = Vec::new();
    let mut scoped_rules: Vec<(Arc<dyn AuditRule>, Vec<AuditNode<'_>>)> = Vec::new();
    let mut rule_indices = HashMap::new();

    for id in tree.subtree(tree.root()).filter(|&id| tree.is_live(id)) {
        let core = tree.node(id).expect("live node");
        let semantics = tree.semantics(id);
        let activation = tree.has_activation_handler(id);
        let role = role(tree, id, core.kind, semantics, activation);
        let provenance = tree.provenance(id);
        let source = provenance.or_else(|| tree.physical_parent(id).and_then(|parent| tree.provenance(parent)));
        let focusable = core.flags.contains(NodeFlags::FOCUSABLE);
        let mut node = AuditNode {
            node: tree.guard(id),
            parent: tree.physical_parent(id).map(|parent| tree.guard(parent)),
            test_id: provenance.and_then(|provenance| provenance.test_id.as_deref()),
            provenance: source.map(|provenance| provenance.origin),
            role,
            accessible_name: None,
            text: tree.text(id).map(|text| text.text.as_ref()),
            geometry: window.geometry(id),
            text_clip: clips.get(&id).copied(),
            shown: tree.is_shown(id),
            hidden_from_accessibility: hidden_from_accessibility(tree, id),
            inert: tree.is_inert(id),
            in_modal_scope: modal.is_none_or(|modal| tree.is_in_modal_scope(id, modal)),
            authored_interactive: focusable || activation || role.is_some_and(interactive_role),
            authored_focusable: focusable || tree.is_layer_focus_container(id),
            focused: tree.focus() == Some(id),
        };
        if exposes_semantics(&node) {
            node.accessible_name = accessible_name(tree, id, role, modal);
        }
        if let Some((scope, rule)) = window.node_context::<Arc<dyn AuditRule>>(world, id) {
            let index = *rule_indices.entry(scope).or_insert_with(|| {
                let index = scoped_rules.len();
                scoped_rules.push((rule, Vec::new()));
                index
            });
            scoped_rules[index].1.push(node);
        }
        nodes.push(node);
    }

    let mut findings = Vec::new();
    for rule in [&ClippedText as &dyn AuditRule, &MissingSemantics, &DeadFocusTarget] {
        rule.check(&nodes, &mut findings);
    }
    for rule in rules {
        rule.check(&nodes, &mut findings);
    }
    for (rule, nodes) in scoped_rules {
        rule.check(&nodes, &mut findings);
    }
    findings
}

fn exposes_semantics(node: &AuditNode<'_>) -> bool {
    node.shown && !node.hidden_from_accessibility && !node.inert && node.in_modal_scope
}

fn hidden_from_accessibility(tree: &Tree, node: NodeId) -> bool {
    std::iter::successors(Some(node), |&node| tree.physical_parent(node))
        .chain(std::iter::successors(tree.logical_parent(node), |&node| tree.logical_parent(node)))
        .any(|node| tree.node(node).is_some_and(|core| core.flags.contains(NodeFlags::HIDDEN)))
}

fn role(tree: &Tree, id: NodeId, kind: NodeKind, semantics: Option<&SemanticsSlot>, activation: bool) -> Option<Role> {
    semantics.and_then(|semantics| semantics.role).or_else(|| match kind {
        NodeKind::Root => Some(Role::Window),
        NodeKind::Text => Some(Role::Label),
        NodeKind::Link => Some(Role::Link),
        NodeKind::TextInput => match tree.editor(id) {
            Some(editor) if editor.is_secure() => Some(Role::PasswordInput),
            Some(editor) if editor.multiline => Some(Role::MultilineTextInput),
            _ => Some(Role::TextInput),
        },
        NodeKind::Image | NodeKind::Icon | NodeKind::Media => Some(Role::Image),
        NodeKind::Native => Some(Role::WebView),
        _ if activation => Some(Role::Button),
        _ => None,
    })
}

fn interactive_role(role: Role) -> bool {
    matches!(
        role,
        Role::Button
            | Role::CheckBox
            | Role::RadioButton
            | Role::Switch
            | Role::TextInput
            | Role::MultilineTextInput
            | Role::PasswordInput
            | Role::ComboBox
            | Role::EditableComboBox
            | Role::ListBox
            | Role::ListBoxOption
            | Role::Tree
            | Role::TreeItem
            | Role::MenuItem
            | Role::MenuItemCheckbox
            | Role::MenuItemRadio
            | Role::Tab
            | Role::Link
            | Role::Slider
            | Role::SpinButton
            | Role::Splitter
    )
}

fn accessible_name(tree: &Tree, id: NodeId, node_role: Option<Role>, modal: Option<NodeId>) -> Option<&str> {
    let read_inline_link = || {
        let link = tree.link(id)?;
        let parent = tree.physical_parent(id)?;
        tree.text(parent)?.text.get(link.range.clone()).filter(|label| !label.trim().is_empty())
    };
    if let Some(name) = own_name(tree, id).or_else(read_inline_link) {
        return Some(name);
    }
    if let Some(name) = tree.semantics(id).and_then(|slot| slot.relations.labelled_by.as_deref()) {
        // A named relationship replaces the implicit caption in the native projection.
        let [target] = tree.named_nodes(name) else { return None };
        if *target != id && is_a11y_visible(tree, *target, modal) {
            return own_name(tree, *target);
        }
        return None;
    }
    match node_role? {
        Role::Button
        | Role::CheckBox
        | Role::RadioButton
        | Role::Switch
        | Role::Link
        | Role::ListBoxOption
        | Role::TreeItem
        | Role::MenuItem
        | Role::MenuItemCheckbox
        | Role::MenuItemRadio
        | Role::Tab => {}
        _ => return None,
    }
    let mut remaining = tree.children(id).iter().rev().copied().collect::<Vec<_>>();
    while let Some(child) = remaining.pop() {
        if !tree.is_locally_accessible(child) || modal.is_some_and(|modal| !tree.is_in_modal_scope(child, modal)) {
            continue;
        }
        let core = tree.node(child).expect("locally accessible node");
        match role(tree, child, core.kind, tree.semantics(child), tree.has_activation_handler(child)) {
            Some(Role::Label | Role::Image) => {
                if let Some(name) = own_name(tree, child) {
                    return Some(name);
                }
            }
            Some(Role::GenericContainer) | None => remaining.extend(tree.children(child).iter().rev().copied()),
            _ => {}
        }
    }
    None
}

fn own_name(tree: &Tree, id: NodeId) -> Option<&str> {
    let label = tree.semantics(id).and_then(|slot| slot.label.as_deref());
    let text = || tree.text(id).map(|text| text.text.as_ref()).filter(|text| !text.trim().is_empty());
    label.filter(|label| !label.trim().is_empty()).or_else(text)
}

fn is_a11y_visible(tree: &Tree, node: NodeId, modal: Option<NodeId>) -> bool {
    tree.is_accessible_without_modal(node)
        && !hidden_from_accessibility(tree, node)
        && modal.is_none_or(|modal| tree.is_in_modal_scope(node, modal))
}
