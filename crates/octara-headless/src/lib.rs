//! Unstyled interaction parts built on the Octara authoring facade.
//!
//! Controllers are stored in their owning build scope. Pass your actual elements
//! to the parts and use Octara's styling API to configure their appearance.
//!
//! ```
//! use octara::prelude::*;
//! use octara_headless::{Control, checkbox::checkbox};
//!
//! fn toolbar(build: &mut BuildCx<'_>) -> El {
//!     let pinned = Control::Uncontrolled(false).bind(build);
//!     checkbox(text("Pin item"), pinned)
//! }
//! ```

pub mod checkbox;
pub mod close;
pub mod collection;
pub mod combobox;
pub mod data_table;
pub mod dialog;
pub mod disclosure;
pub mod field;
mod id;
pub mod interactive;
pub mod listbox;
pub mod menu;
pub mod popover;
pub mod pull_to_refresh;
pub mod radio_group;
pub mod range;
pub mod splitter;
pub mod tabs;
pub mod toast;
pub mod tree_view;

pub use collection::{Choice, Collection, Navigation, Selection};
pub use combobox::ComboboxState;
pub use data_table::DataTableState;
pub use dialog::DialogState;
pub use disclosure::DisclosureState;
pub use field::{FieldState, FieldsetState};
pub use listbox::ListboxState;
pub use menu::{ContextMenuState, MenuSection, MenuState};
pub use popover::{PopoverGroup, PopoverState};
pub use pull_to_refresh::PullToRefresh;
pub use radio_group::RadioGroupState;
pub use range::{NumberFieldState, ProgressState, SliderState};
pub use tabs::TabsState;
pub use toast::ToastQueue;
pub use tree_view::TreeViewState;

use octara::prelude::{Binding, BuildCx, Signal};

/// An external binding or the initial value of a scope-owned control.
pub enum Control<T> {
    Controlled(Binding<T>),
    Uncontrolled(T),
}

impl<T: 'static> Control<T> {
    /// Resolves the control to the binding its parts will share.
    pub fn bind(self, build: &mut BuildCx<'_>) -> Binding<T> {
        match self {
            Self::Controlled(binding) => binding,
            Self::Uncontrolled(initial) => build.signal(initial).into(),
        }
    }
}

impl<T> From<Binding<T>> for Control<T> {
    fn from(binding: Binding<T>) -> Self {
        Self::Controlled(binding)
    }
}

impl<T> From<Signal<T>> for Control<T> {
    fn from(signal: Signal<T>) -> Self {
        Self::Controlled(signal.into())
    }
}
