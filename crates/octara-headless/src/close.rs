use octara::prelude::*;

/// Closes the nearest enclosing dialog, popover, menu, or disclosure on activation.
pub fn close_button<C>(element: El<C>) -> El<C> {
    crate::interactive::button(element).behavior_first(|behavior| {
        behavior.on_press(|write| {
            if let Some(close) = write.close_handle() {
                close.close(write);
            }
        })
    })
}
