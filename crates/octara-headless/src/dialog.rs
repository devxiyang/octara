//! Modal dialog parts with window-managed focus and dismissal.

use crate::{Control, id::Id};
use octara::prelude::*;

/// Controls a portaled modal dialog or alert dialog.
#[derive(Clone, Copy)]
pub struct DialogState {
    open: Binding<bool>,
    id: Id,
    policy: LayerPolicy,
    role: Role,
}

impl DialogState {
    pub fn new(build: &mut BuildCx<'_>, open: impl Into<Control<bool>>) -> Self {
        Self::controlled(open.into().bind(build))
    }

    /// Uses an existing open binding without allocating any state in a build scope.
    pub fn controlled(open: Binding<bool>) -> Self {
        Self { open, id: Id::new(), policy: LayerPolicy::modal(), role: Role::Dialog }
    }

    pub fn focus_trap(mut self, features: FocusTrapFeatures) -> Self {
        self.policy.focus_trap = features;
        self
    }

    pub fn dismiss_on_outside_press(mut self, dismiss: bool) -> Self {
        self.policy.dismiss_on_outside_press = dismiss;
        self
    }

    pub fn alert(mut self) -> Self {
        self.role = Role::AlertDialog;
        self
    }

    pub fn is_open(self, read: &mut ReadCx<'_>) -> bool {
        self.open.get(read)
    }

    pub fn set_open(self, write: &mut WriteCx<'_>, open: bool) {
        self.open.set(write, open);
    }

    /// Attaches the dialog portal to any caller-owned element. Content may include a
    /// panel and a sibling backdrop. The portal honors exit motion on its descendants.
    pub fn root<C>(self, owner: El<C>, mut content: impl FnMut(&mut BuildCx<'_>) -> El + 'static) -> El<C> {
        owner.portal_part(
            move |read| self.is_open(read),
            move |build| {
                build.provide_close(move |write| self.set_open(write, false));
                content(build)
            },
        )
    }

    pub fn panel<C>(self, element: El<C>) -> El<C> {
        element
            .role(self.role)
            .labelled_by(self.id.part("title"))
            .described_by(self.id.part("description"))
            .layer(self.policy, move |write| self.set_open(write, false))
    }

    /// Marks a sibling of the panel as a decorative backdrop. The layer handles dismissal.
    pub fn backdrop<C>(self, element: El<C>) -> El<C> {
        element.a11y_hidden()
    }

    pub fn title<C>(self, element: El<C>) -> El<C> {
        element.id(self.id.part("title")).role(Role::Heading)
    }

    pub fn description<C>(self, element: El<C>) -> El<C> {
        element.id(self.id.part("description")).role(Role::Paragraph)
    }
}
