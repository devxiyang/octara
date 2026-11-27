//! Apply authored deltas to browser nodes before sampling their layout.

use super::{DomEngine, EntryState, exit::ExitPlan, subtrees};
use crate::{css, dom, rich};
use octara::runtime::engine::ProjectFeedback;
use octara::tree::{NodeFlags, NodeId, RetiredNode, Tree, TreeDelta};
use octara_model::geometry::WindowViewport;
use octara_model::input::{CursorStyle, HostReply};
use octara_model::paint::Image;
use octara_model::semantics::Role;
use octara_model::style::{Display, State, UserSelect};
use rustc_hash::{FxHashMap, FxHashSet};
use std::sync::Arc;
use wasm_bindgen::JsValue;
use web_sys::HtmlElement;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) struct Affordance {
    activatable: bool,
    cursor: Option<CursorStyle>,
    disabled: bool,
    hidden: bool,
    role: Option<Role>,
    select: Option<UserSelect>,
}

impl DomEngine {
    pub(super) fn patch_structure(
        &mut self,
        tree: &Tree,
        delta: &TreeDelta,
        mut exits: FxHashMap<NodeId, ExitPlan>,
        feedback: &mut ProjectFeedback,
    ) -> Result<(FxHashSet<NodeId>, FxHashSet<NodeId>), JsValue> {
        if let Some((node, active)) = self.active_editor(tree) {
            let changing_tag = tree.editor(node).is_some_and(|editor| active.local_name() != dom::editor_tag(editor));
            let changing_type = tree.editor(node).is_some_and(|editor| {
                !editor.multiline
                    && active.get_attribute("type").as_deref()
                        != Some(if editor.is_secure() && !editor.revealed { "password" } else { "text" })
            });
            if changing_tag || active.local_name() == "div" {
                self.remember_editor_focus(&active);
            } else if !tree.is_interactive(node) || changing_type {
                self.park_editor(node, active)?;
            }
        }

        let mut created = FxHashSet::default();
        for &node in &delta.created {
            if tree.node(node).is_some() && !self.entries.contains_key(&node) {
                let element = dom::create_element(&self.document, tree, node)?;
                self.remember(node);
                self.entries.insert(node, EntryState::default());
                self.dom.register(node, element);
                self.pending().created.insert(node);
                created.insert(node);
            }
        }
        for &node in &delta.editor_dirty {
            let Some(editor) = tree.editor(node) else { continue };
            let Some(element) = self.dom.element(node) else { continue };
            if element.local_name() == dom::editor_tag(editor) {
                continue;
            }
            let next = dom::create_element(&self.document, tree, node)?;
            let parent = element.parent_node();
            if let Some(parent) = &parent {
                self.pending().undo.place(&next, Some(parent), Some(&element))?;
            }
            self.pending().undo.place(&element, None, None)?;
            let dom = self.dom.clone();
            self.pending().undo.replace_element(&dom, node, next);
            if self.dom.composing() == Some(node) {
                self.dom.set_composing(None);
            }
            self.remember(node);
            let entry = self.entries.get_mut(&node).expect("retained editor");
            entry.rich_editor_rev = None;
            entry.rich_selection = None;
            entry.inline_images = false;
            self.register_indexes(node);
            created.insert(node);
        }
        if let Some(root) = self.dom.element(tree.root())
            && root.parent_element().is_none()
        {
            let surface = self.surface.clone();
            let parking = self.parking.clone();
            self.pending().undo.place(&root, Some(&surface), Some(&parking))?;
            self.root = Some(tree.root());
        }
        for &retired in &delta.retired {
            let node = match retired {
                RetiredNode::Destroyed(node) => node,
                RetiredNode::Exiting { ticket, .. } => {
                    if !tree.accepts_exit(ticket) {
                        continue;
                    }
                    let node = ticket.node;
                    let plan = exits.remove(&node);
                    if plan.is_none() {
                        feedback.replies.push(HostReply::ExitFinished(ticket));
                    }
                    if let Some(element) = self.projected_element(node) {
                        self.stage_exit(node, &element, plan)?;
                    }
                    continue;
                }
            };
            if let Some(element) = self.projected_element(node) {
                self.pending().undo.place(&element, None, None)?;
                self.pending().retired.insert(node);
                self.pending().exits.remove(&node);
                #[cfg(feature = "webgpu")]
                self.pending().gpu.insert(node, None);
            }
        }

        let mut moved = FxHashSet::default();
        for &parent in &delta.topology_dirty {
            self.reorder(tree, parent, &mut moved)?;
        }

        Ok((created, moved))
    }

    pub(super) fn patch_semantics(
        &mut self,
        tree: &Tree,
        delta: &TreeDelta,
        created: &FxHashSet<NodeId>,
        moved: &FxHashSet<NodeId>,
        modal_changed: bool,
    ) -> Result<FxHashSet<NodeId>, JsValue> {
        let modal = tree.front_modal();
        let mut styles = delta.layout_dirty.clone();
        styles.extend(&delta.paint_dirty);
        styles.extend(created);
        styles.extend(moved);
        styles.extend(delta.scroll_dirty.iter().chain(&delta.topology_dirty).copied().filter(|&node| tree.scroll(node).is_some()));
        let mut semantics = delta.semantics_dirty.clone();
        semantics.extend(created);
        if modal_changed || (modal.is_some() && !delta.semantics_dirty.is_disjoint(&delta.overlay_dirty)) {
            semantics.extend(tree.subtree(tree.root()));
        } else if modal.is_some() {
            for &node in moved {
                semantics.extend(tree.subtree(node));
            }
        }
        let modal_ancestors = modal.map_or_else(FxHashSet::default, |modal| {
            tree.overlay_nodes()
                .chain([modal])
                .filter(|&node| tree.is_in_modal_scope(node, modal))
                .flat_map(|node| std::iter::successors(tree.parent(node), |&id| tree.parent(id)))
                .collect()
        });
        let mut actions = FxHashSet::default();
        for &node in semantics.iter().chain(moved).chain(&delta.overlay_dirty) {
            if tree.is_live(node) {
                let action = dom::action(tree, node, modal);
                if let Some(previous) = self.dom.set_action(node, action) {
                    self.pending().actions.entry(node).or_insert(previous);
                    actions.insert(node);
                }
            }
        }
        actions.extend(moved);
        if !actions.is_empty() {
            let mut affected = subtrees(tree, actions.iter().copied());
            for node in tree.overlay_nodes() {
                if actions.iter().any(|&ancestor| tree.is_logically_owned_by(node, ancestor)) {
                    affected.extend(tree.subtree(node));
                }
            }
            for node in affected {
                if let Some(previous) = self.dom.set_action(node, dom::action(tree, node, modal)) {
                    self.pending().actions.entry(node).or_insert(previous);
                }
            }
        }
        // A local change can alter descendant cursors even when the node's own cursor stays the same.
        let mut cursor_roots = moved.clone();
        for &node in styles.union(&semantics) {
            if !tree.is_live(node) {
                continue;
            }
            let core = tree.node(node).expect("live node");
            let slot = tree.semantics(node);
            let affordance = Some(Affordance {
                activatable: tree.has_activation_handler(node),
                cursor: tree.style(node).and_then(|style| style.visual.cursor).or_else(|| slot.and_then(|slot| slot.cursor)),
                disabled: core.state.contains(State::DISABLED),
                hidden: core.flags.contains(NodeFlags::HIDDEN) || tree.layout_style(node).display == Display::None,
                role: slot.and_then(|slot| slot.role),
                select: tree.user_select(node),
            });
            let relates = slot.is_some_and(|slot| !slot.relations.is_empty());
            if let Some(entry) = self.entries.get(&node)
                && (entry.affordance != affordance || entry.relates != relates)
            {
                if entry.affordance != affordance {
                    cursor_roots.insert(node);
                }
                self.remember(node);
                let entry = self.entries.get_mut(&node).expect("retained semantics");
                entry.affordance = affordance;
                entry.relates = relates;
                self.register_indexes(node);
            }
        }
        styles.extend(subtrees(tree, cursor_roots));
        for &node in &semantics {
            if tree.is_live(node)
                && let Some(element) = self.projected_element(node)
            {
                dom::update_semantics(&mut self.pending().undo, &element, tree, node, modal, &modal_ancestors)?;
            }
        }
        if !delta.layout_dirty.is_empty() || !delta.topology_dirty.is_empty() || !delta.retired.is_empty() || !semantics.is_empty() {
            let related: Vec<_> = self.relations.difference(&semantics).copied().collect();
            for node in related {
                if tree.is_live(node)
                    && let Some(element) = self.projected_element(node)
                {
                    dom::update_relations(&mut self.pending().undo, &element, tree, node, modal)?;
                }
            }
        }
        Ok(styles)
    }

    pub(super) fn patch_styles(
        &mut self,
        tree: &Tree,
        delta: &TreeDelta,
        viewport: WindowViewport,
        viewport_changed: bool,
        styles: &mut FxHashSet<NodeId>,
    ) -> Result<(), JsValue> {
        styles.extend(&delta.overlay_dirty);
        let generation = Image::pixel_generation();
        if self.pending().inline_pixel_generation.unwrap_or(self.inline_pixel_generation) != generation {
            styles.extend(&self.inline_images);
            self.pending().inline_pixel_generation = Some(generation);
        }
        let portals =
            tree.overlay_nodes().filter(|&node| tree.overlay(node).is_some_and(|slot| slot.target.is_some() || slot.mount.is_some()));
        for node in portals {
            let visible = tree.is_shown(node);
            if self.entries.get(&node).is_some_and(|entry| entry.overlay_visible != Some(visible)) {
                self.remember(node);
                self.entries.get_mut(&node).expect("retained overlay").overlay_visible = Some(visible);
                styles.insert(node);
            }
            if viewport_changed {
                styles.insert(node);
            }
        }
        for &node in styles.iter() {
            if !tree.is_live(node) {
                continue;
            }
            if let Some(element) = self.projected_element(node) {
                let style = css::style(tree, node, viewport, tree.has_activation_handler(node), self.reduced_motion);
                self.pending().undo.attr(&element, "style", Some(&style))?;
                if let Some(revision) = tree.text(node).map(|text| {
                    let theme_revision = if text.runs.is_empty() && text.attachments.is_empty() { 0 } else { tree.theme_revision_at(node) };
                    let pixel_revision = if rich::has_images(&text.attachments) { Image::pixel_generation() } else { 0 };
                    (text.text_rev, theme_revision, pixel_revision)
                }) && self.entries.get(&node).is_some_and(|entry| entry.text_rev != Some(revision))
                {
                    let dom = self.dom.clone();
                    let document = self.document.clone();
                    dom::update_text(&dom, &mut self.pending().undo, &document, &element, tree, node)?;
                    self.remember(node);
                    let entry = self.entries.get_mut(&node).expect("retained text");
                    entry.text_rev = Some(revision);
                    entry.inline_images = rich::has_images(&tree.text(node).expect("text").attachments);
                    if let Some(inline) = entry.inline_media.as_deref_mut() {
                        let count = tree.inline_media_on(node).count();
                        inline.retain(|&occurrence, _| occurrence < count);
                        if inline.is_empty() {
                            entry.inline_media = None;
                        }
                    }
                    self.register_indexes(node);
                }
                if let Some(icon) = tree.icon(node)
                    && let Some(entry) = self.entries.get(&node)
                    && entry.icon.as_ref().is_none_or(|(old, _)| !Arc::ptr_eq(old, icon))
                {
                    let previous = entry.icon.as_ref().map(|(_, svg)| svg.clone());
                    let svg = dom::icon(&self.document, icon)?;
                    if let Some(previous) = previous {
                        self.pending().undo.place(&previous, None, None)?;
                    }
                    self.pending().undo.place(&svg, Some(&element), None)?;
                    self.remember(node);
                    self.entries.get_mut(&node).expect("retained icon").icon = Some((icon.clone(), svg));
                }
            }
        }

        Ok(())
    }

    pub(super) fn patch_editors(
        &mut self,
        tree: &Tree,
        delta: &TreeDelta,
        created: &FxHashSet<NodeId>,
        styles: &FxHashSet<NodeId>,
    ) -> Result<(), JsValue> {
        let mut fields = delta.editor_dirty.clone();
        fields.extend(created);
        fields.extend(styles.iter().copied().filter(|&node| tree.editor(node).is_some_and(|editor| editor.is_rich())));
        for node in fields {
            let Some(element) = self.projected_element(node) else { continue };
            let Some(editor) = tree.editor(node) else { continue };
            let composing = self.dom.composing() == Some(node);
            if !editor.is_rich() {
                dom::update_editor(&mut self.pending().undo, &element, tree, node, composing)?;
            } else if !composing {
                let inline_images = rich::has_images(editor.attachments());
                let revision =
                    (editor.document_rev(), tree.theme_revision_at(node), if inline_images { Image::pixel_generation() } else { 0 });
                let entry = &self.entries[&node];
                let rebuild = entry.rich_editor_rev != Some(revision)
                    || element.get_attribute("aria-placeholder").as_deref()
                        != (!editor.placeholder.is_empty()).then_some(&*editor.placeholder);
                let selection = editor.composition().map_or_else(|| Some(editor.selection()), |draft| draft.selection());
                if rebuild || entry.rich_selection != selection {
                    dom::update_rich_editor(&mut self.pending().undo, &element, tree, node, rebuild)?;
                    self.remember(node);
                    let entry = self.entries.get_mut(&node).expect("retained rich editor");
                    entry.rich_editor_rev = Some(revision);
                    entry.rich_selection = selection;
                    entry.inline_images = inline_images;
                    self.register_indexes(node);
                }
            }
        }
        if let Some(parked) = self.parked.as_ref()
            && tree.is_interactive(parked.node)
            && parked.stand_in.is_connected()
        {
            self.release_editor()?;
        }

        Ok(())
    }

    pub(super) fn patch_images(&mut self, tree: &Tree, styles: &FxHashSet<NodeId>) -> Result<FxHashSet<NodeId>, JsValue> {
        let mut changed = FxHashSet::default();
        let mut image_nodes = self.images.clone();
        image_nodes.extend(styles.iter().copied().filter(|&node| {
            tree.image(node).is_some() || tree.media_source(node).is_some() || tree.inline_media_on(node).next().is_some()
        }));
        for node in image_nodes {
            if !tree.is_live(node) {
                continue;
            }
            if let Some(slot) = tree.image(node) {
                let sample = slot.image.sample();
                if self.entries.get(&node).and_then(|entry| entry.image.as_ref()) != Some(&sample) {
                    let scrollports = self.scroll_ancestors(tree, [node]);
                    self.save_scrollports(&scrollports);
                    self.update_image(node, sample)?;
                    changed.insert(node);
                }
            } else if let Some(slot) = tree.media_source(node)
                && self.update_media(node, slot.asset.as_ref(), slot.fit)?
            {
                changed.insert(node);
            } else {
                self.update_inline_media(tree, node)?;
            }
        }
        for &node in styles {
            if tree.is_live(node)
                && let Some(slot) = tree.native(node).filter(|slot| slot.kind == "video")
            {
                self.update_video(node, &slot.props)?;
            }
        }
        Ok(changed)
    }

    fn reorder(&mut self, tree: &Tree, parent: NodeId, moved: &mut FxHashSet<NodeId>) -> Result<(), JsValue> {
        if !tree.is_live(parent) || tree.text(parent).is_some() {
            return Ok(());
        }
        let Some(element) = self.projected_element(parent) else { return Ok(()) };
        let mut order = Vec::new();
        let pending = self.pending.as_ref().expect("an open DOM projection");
        for &child in tree.children(parent) {
            order.push(child);
            if !tree.is_live(child) {
                // Independently fading descendants need their own opacity context.
                order.extend(tree.subtree(child).skip(1).filter(|node| self.exits.contains_key(node) || pending.exits.contains_key(node)));
            }
        }
        let mut next: Option<HtmlElement> = None;
        for child in order.into_iter().rev() {
            let Some(mut current) = self.projected_element(child) else { continue };
            let next_node = next.as_ref().map(|element| element.as_ref());
            let already_placed = current.parent_element().as_ref() == Some(element.as_ref())
                && next.as_ref().is_none_or(|next| current.next_element_sibling().as_ref() == Some(next.as_ref()));
            if !already_placed {
                if let Some((node, active)) = self.active_editor(tree)
                    && current.contains(Some(&active))
                {
                    self.park_editor(node, active)?;
                    current = self.projected_element(child).expect("projected child");
                }
                self.pending().undo.place(&current, Some(&element), next_node)?;
                moved.insert(child);
            }
            next = Some(current);
        }
        Ok(())
    }
}
