use octara::prelude::*;

/// The representation of a checkbox's binding.
pub trait CheckValue: Copy + 'static {
    fn checked(self) -> Checked;
    fn toggle(self) -> Self;
}

impl CheckValue for bool {
    fn checked(self) -> Checked {
        self.into()
    }

    fn toggle(self) -> Self {
        !self
    }
}

impl CheckValue for Checked {
    fn checked(self) -> Checked {
        self
    }

    fn toggle(self) -> Self {
        match self {
            Checked::Checked => Checked::Unchecked,
            Checked::Unchecked | Checked::Mixed => Checked::Checked,
        }
    }
}

/// Attaches checkbox behavior to a boolean or tri-state binding.
pub fn checkbox<C, T: CheckValue>(element: El<C>, value: Binding<T>) -> El<C> {
    checkable(element, value, Role::CheckBox)
}

/// Attaches switch behavior to a boolean binding.
pub fn switch<C>(element: El<C>, value: Binding<bool>) -> El<C> {
    checkable(element, value, Role::Switch)
}

fn checkable<C, T: CheckValue>(element: El<C>, value: Binding<T>, role: Role) -> El<C> {
    crate::interactive::data_interactive(element)
        .role(role)
        .focusable()
        .checked(move |read: &mut ReadCx<'_>| value.get(read).checked())
        .behavior_first(|behavior| {
            behavior.on_toggle(move |write| {
                let next = write.read(|read| value.get(read).toggle());
                value.set(write, next);
            })
        })
}
