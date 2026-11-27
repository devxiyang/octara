//! Scope-based form relationships and disabled state.

use crate::id::Id;
use octara::prelude::*;

#[derive(Clone, Copy)]
pub struct FieldsetState {
    id: Id,
    disabled: Memo<bool>,
}

impl FieldsetState {
    pub fn new(build: &mut BuildCx<'_>, disabled: impl IntoProp<bool>) -> Self {
        let inherited = build.context::<Self>().map(|parent| parent.disabled);
        Self { id: Id::new(), disabled: disabled_memo(build, disabled.into_prop(), [inherited, None]) }
    }

    pub fn provide(self, build: &mut BuildCx<'_>) {
        build.provide(self);
    }

    pub fn is_disabled(self, read: &mut ReadCx<'_>) -> bool {
        read.get(self.disabled)
    }

    pub fn legend<C>(self, element: El<C>) -> El<C> {
        element.role(Role::Label).id(self.id.part("legend"))
    }
}

#[derive(Clone, Copy)]
pub struct FieldState {
    id: Id,
    disabled: Memo<bool>,
}

impl FieldState {
    pub fn new(build: &mut BuildCx<'_>, disabled: impl IntoProp<bool>) -> Self {
        let inherited =
            [build.context::<FieldsetState>().map(|parent| parent.disabled), build.context::<Self>().map(|parent| parent.disabled)];
        Self { id: Id::new(), disabled: disabled_memo(build, disabled.into_prop(), inherited) }
    }

    pub fn provide(self, build: &mut BuildCx<'_>) {
        build.provide(self);
    }

    pub fn label_id(self) -> String {
        self.id.part("label")
    }

    pub fn description_id(self) -> String {
        self.id.part("description")
    }

    pub fn is_disabled(self, read: &mut ReadCx<'_>) -> bool {
        read.get(self.disabled)
    }

    pub fn label<C>(self, element: El<C>) -> El<C> {
        element.role(Role::Label).id(self.label_id())
    }

    pub fn description<C>(self, element: El<C>) -> El<C> {
        element.id(self.description_id())
    }

    pub fn control<C>(self, element: El<C>) -> El<C> {
        element.labelled_by(self.label_id()).described_by(self.description_id()).disabled_when(self.disabled)
    }
}

/// Names the supplied group and makes the fieldset available to its content factory.
pub fn fieldset<C, V: IntoView + 'static>(
    element: El<C>,
    state: FieldsetState,
    content: impl FnOnce(&mut BuildCx<'_>) -> V + 'static,
) -> El<C> {
    element.role(Role::Group).labelled_by(state.id.part("legend")).disabled_when(state.disabled).child(scoped(move |build| {
        state.provide(build);
        content(build)
    }))
}

/// Configures a field group and provides its state to dynamically built descendants.
pub fn field<C, V: IntoView + 'static>(element: El<C>, state: FieldState, content: impl FnOnce(&mut BuildCx<'_>) -> V + 'static) -> El<C> {
    element.role(Role::Group).child(scoped(move |build| {
        state.provide(build);
        content(build)
    }))
}

pub fn label<C>(build: &BuildCx<'_>, element: El<C>) -> El<C> {
    match build.context::<FieldState>() {
        Some(state) => state.label(element),
        None => element,
    }
}

pub fn description<C>(build: &BuildCx<'_>, element: El<C>) -> El<C> {
    match build.context::<FieldState>() {
        Some(state) => state.description(element),
        None => element,
    }
}

pub fn control<C>(build: &BuildCx<'_>, element: El<C>) -> El<C> {
    let element = match build.context::<FieldState>() {
        Some(state) => state.control(element),
        None => element,
    };
    if let Some(fieldset) = build.context::<FieldsetState>() { element.disabled_when(fieldset.disabled) } else { element }
}

fn disabled_memo(build: &mut BuildCx<'_>, mut local: Prop<bool>, inherited: [Option<Memo<bool>>; 2]) -> Memo<bool> {
    build.memo(move |read| {
        inherited.into_iter().flatten().any(|parent| read.get(parent))
            || match &mut local {
                Prop::Static(value) => *value,
                Prop::Dyn(read_disabled) => read_disabled(read),
            }
    })
}
