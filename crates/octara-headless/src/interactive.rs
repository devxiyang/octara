use octara::prelude::*;

/// Configures button semantics, focus and pointer states on the supplied element.
pub fn button<C>(element: El<C>) -> El<C> {
    data_interactive(element).role(Role::Button).focusable()
}

/// Opts the supplied element into hover, focus and press feedback.
pub fn data_interactive<C>(element: El<C>) -> El<C> {
    element.track_interaction()
}

/// Configures a button whose pressed state is controlled by a boolean binding.
pub fn toggle_button<C>(element: El<C>, pressed: Binding<bool>) -> El<C> {
    crate::checkbox::checkbox(element, pressed).role(Role::Button)
}
