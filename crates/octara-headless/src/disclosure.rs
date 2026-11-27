//! Expandable content in the document flow.

use crate::{Control, id::Id};
use octara::prelude::*;

/// Controls the visibility of one in-flow panel.
#[derive(Clone, Copy)]
pub struct DisclosureState {
    open: Binding<bool>,
    id: Id,
}

impl DisclosureState {
    pub fn new(build: &mut BuildCx<'_>, open: impl Into<Control<bool>>) -> Self {
        Self { open: open.into().bind(build), id: Id::new() }
    }

    pub fn is_open(self, read: &mut ReadCx<'_>) -> bool {
        self.open.get(read)
    }

    pub fn set_open(self, write: &mut WriteCx<'_>, open: bool) {
        self.open.set(write, open);
    }

    pub fn button<C>(self, element: El<C>) -> El<C> {
        element
            .id(self.id.part("button"))
            .role(Role::Button)
            .focusable()
            .expanded(self.open)
            .controls(self.id.part("panel"))
            .behavior_first(|behavior| {
                behavior.on_press(move |write| {
                    let open = write.read(|read| self.is_open(read));
                    self.set_open(write, !open);
                })
            })
    }

    /// Builds the caller's panel within a scope that provides its close action.
    pub fn panel(self, mut build_panel: impl FnMut(&mut BuildCx<'_>) -> El + 'static) -> View {
        show(
            move |read| self.is_open(read),
            move |build| {
                build.provide_close(move |write| {
                    self.set_open(write, false);
                    if !write.read(|read| self.is_open(read)) {
                        write.focus(&self.id.part("button"));
                    }
                });
                build_panel(build).id(self.id.part("panel")).role(Role::Group).labelled_by(self.id.part("button"))
            },
        )
    }
}
