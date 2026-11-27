//! Themed controls and layout presets for Octara.
//!
//! Components delegate interaction to [headless] and keep appearance in Kit.
//! Pass the returned elements into your view and use Octara's styling API for
//! application-specific layout.

use octara::prelude::{ReadCx, RowSignal};

mod choices;
mod controls;
mod data;
mod dialog;
mod feedback;
mod forms;
mod locale;
mod menus;
mod navigation;
pub mod parts;
mod popover;
mod range;
mod status;
mod structure;

pub use octara_headless as headless;
pub use octara_headless::toast::{ToastId, ToastOptions, ToastQueue};

pub use choices::{ComboboxOptions, PickerOptions, choice_row, combobox, combobox_input, select};
pub use controls::{
    ButtonVariant, FileDrop, Size, Tone, button, checkbox, file_dropzone, icon_button, password_field, search_field, switch, text_area,
    text_field, toggle_button,
};
pub use data::{data_table, tree_view, tree_view_with};
pub use dialog::{DialogOptions, alert_dialog, dialog};
pub use feedback::{command_palette, toast_viewport};
pub use forms::{FieldInfo, field, fieldset, radio_group};
pub use locale::KitLocale;
pub use menus::{context_menu, menu_button};
pub use navigation::{TabsOptions, breadcrumb, segmented, tab_panel, tabs};
pub use popover::{PopupOptions, popover};
pub use range::{number_field, progress, slider};
pub use status::{avatar, badge, callout, chip, removable_chip, skeleton, spinner};
pub use structure::{collapsible, list, list_header, list_item, separator, split, splitter};

pub(crate) use controls::TOUCH_TARGET;

fn row_value<T: 'static, U: Default>(row: RowSignal<T>, read: &mut ReadCx<'_>, project: impl FnOnce(&T) -> U) -> U {
    read.try_with(row, project).unwrap_or_default()
}
