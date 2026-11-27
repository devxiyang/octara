//! Park an active editor while speculative DOM mutations are in flight.

use super::{DomEngine, FocusSnapshot, ParkedEditor};
use crate::undo::InputState;
use octara::tree::{NodeId, Tree};
use wasm_bindgen::{JsCast, JsValue};
use web_sys::HtmlElement;

impl DomEngine {
    pub(super) fn active_editor(&self, tree: &Tree) -> Option<(NodeId, HtmlElement)> {
        if self.parked.is_some() {
            return None;
        }
        let active = self.document.active_element()?;
        let node = self.dom.node(&active)?;
        let original = self.dom.element(node)?;
        // A destroyed node no longer has an editor slot, so inspect the retained element.
        (original.has_attribute("data-octara-editor") && (tree.node(node).is_none() || tree.editor(node).is_some()))
            .then_some((node, original))
    }

    pub(super) fn remember_editor_focus(&mut self, element: &HtmlElement) {
        if self.fallback_focus.is_none() {
            self.fallback_focus = Some(FocusSnapshot {
                element: element.clone(),
                input: InputState::read(element),
                rich_selection: crate::rich::selection(element),
                composing: self.dom.composing(),
            });
        }
    }

    pub(super) fn park_editor(&mut self, node: NodeId, element: HtmlElement) -> Result<(), JsValue> {
        let Some(parent) = element.parent_node() else { return Ok(()) };
        let stand_in = element.clone_node()?.dyn_into::<HtmlElement>()?;
        if let Some(input) = InputState::read(&element) {
            input.restore(&stand_in)?;
        }
        self.pending().undo.save_scroll(&element);
        self.pending().undo.place(&stand_in, Some(&parent), Some(&element))?;
        self.pending().undo.attr(&element, "id", None)?;
        let parking = self.parking.clone();
        let composing = self.dom.composing();
        let input = InputState::read(&element);
        self.pending().undo.place(&element, Some(&parking), None)?;
        let retained_focus = self.document.active_element().as_ref() == Some(element.as_ref());
        if !retained_focus && self.fallback_focus.is_none() {
            self.fallback_focus = Some(FocusSnapshot { element: element.clone(), input, rich_selection: None, composing });
        }
        self.parked = Some(ParkedEditor { node, element, stand_in, retained_focus });
        Ok(())
    }

    pub(super) fn release_editor(&mut self) -> Result<(), JsValue> {
        let Some(ParkedEditor { node, element, stand_in, retained_focus }) = self.parked.take() else { return Ok(()) };
        let parent = stand_in.parent_node();
        if let Some(parent) = parent {
            let desired = InputState::read(&stand_in);
            let before = InputState::read(&element);
            if let Some(before) = before.clone() {
                self.pending().undo.save_input(&element, before);
            }
            for name in
                element.get_attribute_names().iter().chain(stand_in.get_attribute_names().iter()).filter_map(|name| name.as_string())
            {
                let value = stand_in.get_attribute(&name);
                self.pending().undo.attr(&element, &name, value.as_deref())?;
            }
            let composing = self.dom.composing() == Some(node);
            if (!retained_focus || !composing)
                && before != desired
                && let Some(desired) = desired
            {
                desired.restore(&element)?;
            }
            self.pending().undo.place(&element, Some(&parent), Some(&stand_in))?;
            self.pending().undo.place(&stand_in, None, None)?;
        } else {
            self.pending().undo.place(&element, None, None)?;
        }
        Ok(())
    }
}
