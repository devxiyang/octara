//! Anchored panels with logical focus and outside-press scopes.

use crate::{Control, id::Id};
use octara::prelude::*;

/// A shared interaction scope for a set of popovers and related controls.
#[derive(Clone, Copy, Default)]
pub struct PopoverGroup(LayerGroupId);

impl PopoverGroup {
    pub fn new() -> Self {
        Self::default()
    }

    /// Includes the element and its logical descendants in the group's scope.
    pub fn scope<C>(self, element: El<C>) -> El<C> {
        element.layer_group(self.0)
    }
}

/// Controls one non-modal popover.
#[derive(Clone, Copy)]
pub struct PopoverState {
    open: Binding<bool>,
    id: Id,
    policy: LayerPolicy,
}

impl PopoverState {
    pub fn new(build: &mut BuildCx<'_>, open: impl Into<Control<bool>>) -> Self {
        Self::controlled(open.into().bind(build))
    }

    /// Uses an existing open binding without allocating any state in a build scope.
    pub fn controlled(open: Binding<bool>) -> Self {
        Self { open, id: Id::new(), policy: LayerPolicy::non_modal() }
    }

    pub fn focus_trap(mut self, features: FocusTrapFeatures) -> Self {
        self.policy.focus_trap = features;
        self
    }

    pub fn in_group(mut self, group: PopoverGroup) -> Self {
        self.policy.group = Some(group.0);
        self
    }

    pub fn is_open(self, read: &mut ReadCx<'_>) -> bool {
        self.open.get(read)
    }

    pub fn set_open(self, write: &mut WriteCx<'_>, open: bool) {
        self.open.set(write, open);
    }

    pub fn panel_id(self) -> String {
        self.id.part("panel")
    }

    pub fn button<C>(self, element: El<C>) -> El<C> {
        let button =
            element.id(self.id.part("button")).role(Role::Button).focusable().expanded(self.open).controls(self.panel_id()).behavior_first(
                |behavior| {
                    behavior.on_press(move |write| {
                        let open = write.read(|read| self.is_open(read));
                        self.set_open(write, !open);
                    })
                },
            );
        match self.policy.group {
            Some(group) => button.layer_group(group),
            None => button,
        }
    }

    /// Opens caller-authored content beside the owner. The factory inherits a close action.
    pub fn overlay<C>(
        self,
        owner: El<C>,
        placement: Placement,
        target: Option<AnchorTarget>,
        content: impl FnMut(&mut BuildCx<'_>) -> El + 'static,
    ) -> El<C> {
        self.overlay_with_target(owner, placement, target, content)
    }

    /// Positions the panel at a tracked anchor, such as an editor caret.
    pub fn overlay_with_target<C>(
        self,
        owner: El<C>,
        placement: Placement,
        target: impl IntoProp<Option<AnchorTarget>>,
        mut content: impl FnMut(&mut BuildCx<'_>) -> El + 'static,
    ) -> El<C> {
        owner.overlay_target(
            placement,
            target,
            move |read| self.is_open(read),
            move |build| {
                build.provide_close(move |write| self.set_open(write, false));
                content(build)
            },
        )
    }

    pub fn panel<C>(self, element: El<C>) -> El<C> {
        element
            .id(self.panel_id())
            .role(Role::Group)
            .labelled_by(self.id.part("button"))
            .layer(self.policy, move |write| self.set_open(write, false))
    }

    /// Marks a sibling of the panel as decorative. The layer handles outside gestures.
    pub fn backdrop<C>(self, element: El<C>) -> El<C> {
        element.a11y_hidden()
    }
}
