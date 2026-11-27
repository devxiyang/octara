//! Style storage, invalidation, and subtree traversal.

use crate::tables::{TableMask, ThemeScope};
use crate::{NodeFlags, NodeId, NodeKind, StyleInvalidation, Tree, TreeDelta};
use octara_model::style::{Inherited, LayoutStyle, State};
use rustc_hash::FxHashSet;
use smallvec::SmallVec;

use super::{ComputedStyle, StyleResolver, sensitivity};

impl Tree {
    pub(crate) fn inherited_at(&self, mut node: Option<NodeId>) -> Inherited {
        while let Some(current) = node {
            if let Some(style) = self.style(current) {
                return style.inherited;
            }
            node = self.parent(current);
        }
        self.initial_inherited
    }

    fn group_hovered(&self, mut node: Option<NodeId>, name: &str) -> bool {
        while let Some(current) = node {
            if self.style(current).and_then(|style| style.authored.group) == Some(name) {
                return self.nodes[current].state.contains(State::HOVER);
            }
            node = self.parent(current);
        }
        false
    }

    pub(crate) fn local_style_bloom(&self, node: NodeId) -> u64 {
        use octara_model::theme::ColorToken;

        let mut mask = sensitivity::DEFAULT | self.style(node).map_or(0, |style| style.sensitivity);
        if self.nodes[node].present.intersects(TableMask::THEME_PATCHES | TableMask::THEME_SCOPES) {
            mask |= sensitivity::PATCH | sensitivity::SCHEME;
        }
        match self.nodes[node].kind {
            NodeKind::Text => {
                if self.text(node).is_some_and(|text| !text.runs.is_empty() || !text.attachments.is_empty()) {
                    mask |= sensitivity::PROJECTION;
                }
                mask |= sensitivity::token(ColorToken::Selection) | sensitivity::token(ColorToken::TextSubtle);
            }
            NodeKind::Canvas | NodeKind::TextInput => mask |= sensitivity::PROJECTION,
            NodeKind::Native => mask |= sensitivity::token(ColorToken::Surface),
            _ => {}
        }
        if self.nodes[node].present.intersects(TableMask::PAINTS | TableMask::TEXT_REVEALS) {
            mask |= sensitivity::PROJECTION;
        }
        if self.overlay(node).is_some_and(|overlay| overlay.target.is_some_and(|(_, placement)| placement.fit)) {
            mask |= sensitivity::token(ColorToken::TextSubtle);
        }
        mask
    }

    pub(crate) fn add_style_bloom(&mut self, mut node: NodeId, added: u64) {
        while self.nodes[node].style_bloom & added != added {
            let core = &mut self.nodes[node];
            core.style_bloom |= added;
            let Some(parent) = core.parent else { break };
            node = parent;
        }
    }

    pub(crate) fn refresh_style_bloom(&mut self, mut node: NodeId) {
        loop {
            let next = self.children(node).iter().fold(self.local_style_bloom(node), |mask, child| mask | self.nodes[*child].style_bloom);
            let core = &mut self.nodes[node];
            if core.style_bloom == next {
                break;
            }
            let previous = std::mem::replace(&mut core.style_bloom, next);
            let Some(parent) = core.parent else { break };
            if previous & !next == 0 {
                self.add_style_bloom(parent, next & !previous);
                break;
            }
            node = parent;
        }
    }

    pub(crate) fn resolve_dirty_styles(&mut self, dirty: FxHashSet<NodeId>, patches: FxHashSet<NodeId>, delta: &mut TreeDelta) {
        let full = StyleInvalidation { force: true, ..StyleInvalidation::default() };
        if patches.is_empty() {
            for node in dirty.iter().copied() {
                if self.node(node).is_some() && !self.has_style_dirty_parent(node, &dirty) {
                    self.resolve_subtree(node, full, delta);
                }
            }
            return;
        }
        for (_, node) in self.style_order(dirty.union(&patches).copied().filter(|&node| !self.has_style_dirty_parent(node, &dirty))) {
            if dirty.contains(&node) {
                self.resolve_subtree(node, full, delta);
            } else if self.theme_patch_pending(node) {
                let invalidation = StyleInvalidation { theme: sensitivity::PATCH, ..StyleInvalidation::default() };
                self.walk_styles(node, invalidation, Some(&dirty), delta);
            }
        }
    }

    fn has_style_dirty_parent(&self, node: NodeId, dirty: &FxHashSet<NodeId>) -> bool {
        std::iter::successors(self.parent(node), |&parent| self.parent(parent)).any(|parent| dirty.contains(&parent))
    }

    fn style_order(&self, dirty: impl IntoIterator<Item = NodeId>) -> SmallVec<[(usize, NodeId); 8]> {
        let mut nodes: SmallVec<[(usize, NodeId); 8]> = dirty
            .into_iter()
            .filter(|&node| self.nodes.contains_key(node))
            .map(|node| (std::iter::successors(self.parent(node), |&parent| self.parent(parent)).count(), node))
            .collect();
        nodes.sort_unstable();
        nodes
    }

    pub(crate) fn resolve_focus_styles(&mut self, dirty: FxHashSet<NodeId>, delta: &mut TreeDelta) {
        let invalidation = StyleInvalidation { local_state: sensitivity::state(State::TREE_DERIVED), ..StyleInvalidation::default() };
        for (_, node) in self.style_order(dirty) {
            self.resolve_subtree(node, invalidation, delta);
        }
    }

    fn theme_patch_pending(&self, node: NodeId) -> bool {
        self.theme_scope(node).map_or_else(|| self.has_theme_patch(node), |scope| scope.base_revision.is_none())
    }

    pub(crate) fn resolve_subtree(&mut self, node: NodeId, invalidation: StyleInvalidation, delta: &mut TreeDelta) {
        self.walk_styles(node, invalidation, None, delta);
    }

    fn walk_styles(
        &mut self,
        node: NodeId,
        mut invalidation: StyleInvalidation,
        deferred: Option<&FxHashSet<NodeId>>,
        delta: &mut TreeDelta,
    ) {
        // Carry inherited values through read-only branches. Resume siblings from
        // one frame per ancestor rather than copying a wide subtree into scratch.
        let mut path = SmallVec::<[(NodeId, usize, StyleInvalidation, Inherited); 8]>::new();
        let from = self.inherited_at(self.parent(node));
        invalidation.inherited_theme = invalidation.theme != 0 && invalidation.theme & sensitivity::color(from.text) != 0;
        let mut next = Some((node, invalidation, from));
        while let Some((current, mut invalidation, from)) = next.take().or_else(|| {
            while let Some((parent, index, invalidation, from)) = path.last_mut() {
                if let Some(&child) = self.children(*parent).get(*index) {
                    *index += 1;
                    return Some((child, *invalidation, *from));
                }
                path.pop();
            }
            None
        }) {
            let core = &self.nodes[current];
            if core.flags.contains(NodeFlags::EXITING) {
                continue;
            }
            let changed = invalidation.conditions | invalidation.theme | invalidation.local_state;
            if core.style_bloom & changed == 0 && !invalidation.force && !invalidation.inherited_dirty && !invalidation.inherited_theme {
                #[cfg(test)]
                {
                    self.style_prunes += 1;
                }
                continue;
            }
            if current != node && deferred.is_some_and(|dirty| dirty.contains(&current)) {
                continue;
            }
            #[cfg(test)]
            {
                self.style_visits += 1;
            }
            let parent = self.parent(current);
            if (invalidation.force || invalidation.theme != 0)
                && core.present.intersects(TableMask::THEME_PATCHES | TableMask::THEME_SCOPES)
            {
                let authored_patch = self.theme_patch_pending(current);
                invalidation.theme = self.update_theme_scope(current, parent, delta);
                invalidation.inherited_theme = invalidation.theme != 0 && invalidation.theme & sensitivity::color(from.text) != 0;
                if authored_patch {
                    // The Web host sets color-scheme at the scope boundary, even if the palette stayed equal.
                    self.paint_changed(current, delta);
                }
            }
            let local = self.local_style_bloom(current);
            let previous_inherited = self.style(current).map(|style| style.inherited);
            let inherited = if invalidation.force
                || invalidation.inherited_dirty
                || invalidation.inherited_theme
                || local & (invalidation.theme | invalidation.conditions | invalidation.local_state) != 0
            {
                self.resolve_style(current, from, invalidation.force || invalidation.theme != 0, delta);
                self.style(current).map_or(from, |style| style.inherited)
            } else {
                previous_inherited.unwrap_or(from)
            };
            invalidation.inherited_dirty = previous_inherited.map_or(invalidation.inherited_dirty, |old| old != inherited);
            invalidation.local_state = 0;
            if (invalidation.force || invalidation.inherited_dirty || invalidation.conditions | invalidation.theme != 0)
                && let Some((&first, siblings)) = self.children(current).split_first()
            {
                invalidation.inherited_theme = invalidation.theme != 0 && invalidation.theme & sensitivity::color(inherited.text) != 0;
                if !siblings.is_empty() {
                    path.push((current, 1, invalidation, inherited));
                }
                next = Some((first, invalidation, inherited));
            }
        }
    }

    fn resolve_style(&mut self, node: NodeId, from: Inherited, theme_invalidated: bool, delta: &mut TreeDelta) {
        let Some(style) = self.style(node) else {
            // Unstyled nodes still consume inherited paint and motion properties.
            self.resolve_pointer_events(node, None, delta);
            self.paint_changed(node, delta);
            if matches!(self.nodes[node].kind, NodeKind::Text | NodeKind::TextInput | NodeKind::Link) {
                self.layout_changed(node, delta);
            }
            return;
        };
        let parent = self.parent(node);
        let theme_revision = self.theme_revision_at(node);
        let theme_changed = theme_invalidated && style.theme_revision != theme_revision;
        let resolved = StyleResolver::new(self.theme_at(node), self.breakpoint, self.responsive_breakpoint, self.touch).resolve(
            LayoutStyle::default(),
            &from,
            &style.authored.base,
            &style.authored.overrides,
            self.nodes[node].state,
            |name| self.group_hovered(parent, name),
        );
        let geometry = style.layout != resolved.layout;
        let layout = geometry
            || style.visual.text_style() != resolved.visual.text_style()
            || (theme_changed && self.text(node).is_some_and(|text| text.runs.iter().any(|(_, span)| span.mono)));
        let visual = style.visual != resolved.visual;
        let display = style.layout.display != resolved.layout.display;
        let selection = style.visual.user_select != resolved.visual.user_select;
        let pointer_events = resolved.visual.pointer_events;
        let slot = &mut self.tables.styles[node];
        slot.theme_revision = theme_revision;
        slot.inherited_from = from;
        slot.inherited = resolved.inherited;
        slot.opacity_motion_authored = resolved.opacity_motion_authored;
        if geometry || visual {
            let shared = self.style_interner.intern(ComputedStyle { layout: resolved.layout, visual: resolved.visual });
            self.style_interner.release(std::mem::replace(&mut slot.resolved, shared));
        }
        let paint = self.resolve_pointer_events(node, pointer_events, delta) || visual || theme_changed;
        self.report_missing_theme_tokens(node, resolved.missing_tokens, delta);
        if layout || paint {
            let rev = &mut self.nodes[node].style_rev;
            *rev = rev.wrapping_add(1);
        }
        if layout {
            self.layout_changed(node, delta);
        }
        if paint {
            self.paint_changed(node, delta);
        }
        if display || selection {
            self.semantics_changed(node, delta);
        }
        self.sync_scroll_slot(node, delta);
    }

    fn resolve_pointer_events(&mut self, node: NodeId, styled: Option<bool>, delta: &mut TreeDelta) -> bool {
        let core = &mut self.nodes[node];
        let excluded = styled.map_or(core.authored_pointer_events_none, |enabled| !enabled);
        if core.flags.contains(NodeFlags::POINTER_EVENTS_NONE) == excluded {
            return false;
        }
        core.flags.set(NodeFlags::POINTER_EVENTS_NONE, excluded);
        // Web descendants reset CSS's inherited pointer-events at this boundary.
        delta.paint_dirty.extend(self.children(node).iter().copied());
        true
    }

    fn update_theme_scope(&mut self, node: NodeId, parent: Option<NodeId>, delta: &mut TreeDelta) -> u64 {
        let Some(patch) = self.theme_patch(node) else {
            let Some(previous) = self.tables.theme_scopes.remove(node) else { return 0 };
            self.nodes[node].present.remove(TableMask::THEME_SCOPES);
            self.refresh_style_bloom(node);
            return sensitivity::theme(&previous.theme, parent.map_or(&self.theme, |parent| self.theme_at(parent)));
        };
        let inherited = self.theme_scope_at(parent);
        let base_revision =
            Some(if patch.base_theme.is_some() { 0 } else { inherited.map_or(self.root_theme_revision, |scope| scope.revision) });
        if self.theme_scope(node).is_some_and(|scope| scope.base_revision == base_revision) {
            return 0;
        }
        let base = inherited.map_or(&self.theme, |scope| &scope.theme);
        let (theme, missing) = patch.resolve(base);
        let previous = self.theme_scope(node).map_or(base, |scope| &scope.theme);
        let changed = sensitivity::theme(previous, &theme);
        self.report_missing_theme_tokens(node, missing, delta);
        if changed == 0
            && let Some(scope) = self.tables.theme_scopes.get_mut(node)
        {
            scope.base_revision = base_revision;
            return 0;
        }
        let revision = self.next_theme_revision();
        self.nodes[node].present.insert(TableMask::THEME_SCOPES);
        self.tables.theme_scopes.insert(node, ThemeScope { base_revision, revision, theme });
        changed
    }

    fn report_missing_theme_tokens(&self, node: NodeId, missing: impl IntoIterator<Item = &'static str>, delta: &mut TreeDelta) {
        use octara_model::diagnostics::{Diagnostic, codes};
        for name in missing {
            let mut diagnostic = Diagnostic::new(&codes::MISSING_THEME_TOKEN, format!("Missing theme token {name}"));
            diagnostic.location = self.provenance(node).map(|source| source.origin.to_string());
            delta.diagnostics.push(diagnostic);
        }
    }
}
