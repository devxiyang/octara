//! State and element parts for roving tabs and their panels.

use crate::{Control, id::Id};
use octara::prelude::*;
use std::fmt::Display as KeyDisplay;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Activation {
    #[default]
    Automatic,
    Manual,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Orientation {
    #[default]
    Horizontal,
    Vertical,
}

pub struct TabsState<K> {
    id: Id,
    selected: Binding<Option<K>>,
    activation: Activation,
    orientation: Orientation,
}

impl<K> Copy for TabsState<K> {}

impl<K> Clone for TabsState<K> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<K: Clone + PartialEq + KeyDisplay + 'static> TabsState<K> {
    pub fn new(build: &mut BuildCx<'_>, selected: impl Into<Control<Option<K>>>, activation: Activation, orientation: Orientation) -> Self {
        Self { id: Id::new(), selected: selected.into().bind(build), activation, orientation }
    }

    pub fn is_selected(self, read: &mut ReadCx<'_>, key: &K) -> bool {
        read.with(self.selected, |value| value.as_ref() == Some(key))
    }

    pub fn select(self, write: &mut WriteCx<'_>, key: K) {
        if !write.read(|read| self.is_selected(read, &key)) {
            write.set(self.selected, Some(key));
        }
    }

    pub fn list<C>(self, element: El<C>) -> El<C> {
        let focus = match self.orientation {
            Orientation::Horizontal => FocusGroup::horizontal(),
            Orientation::Vertical => FocusGroup::vertical(),
        };
        element.role(Role::TabList).focus_group(focus.wrap())
    }

    /// Distinct tab keys must have distinct display forms within this tab set.
    pub fn tab<C>(self, element: El<C>, key: K) -> El<C> {
        let part = key.to_string();
        let selected_key = key.clone();
        let preferred = key.clone();
        let pressed = key.clone();
        element
            .role(Role::Tab)
            .focusable()
            .track_interaction()
            .id(self.id.part(&format!("tab-{part}")))
            .controls(self.id.part(&format!("panel-{part}")))
            .selected(move |read: &mut ReadCx<'_>| self.is_selected(read, &selected_key))
            .focus_preferred(move |read: &mut ReadCx<'_>| self.is_selected(read, &preferred))
            .behavior_first(|behavior| behavior.on_press(move |write| self.select(write, pressed.clone())))
            .on_focus(move |write| {
                if self.activation == Activation::Automatic {
                    self.select(write, key.clone());
                }
            })
    }

    pub fn panels<C>(self, element: El<C>) -> El<C> {
        element.role(Role::Group)
    }

    pub fn panel<C>(self, element: El<C>, key: K) -> El<C> {
        let part = key.to_string();
        element
            .role(Role::TabPanel)
            .id(self.id.part(&format!("panel-{part}")))
            .labelled_by(self.id.part(&format!("tab-{part}")))
            .style_with(move |read, style| if self.is_selected(read, &key) { style } else { style.display(Display::None) })
    }
}
