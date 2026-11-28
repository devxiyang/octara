//! Scope-owned declarations that belong to a mounted window.

use crate::app::PresentationUpdate;
use crate::{BuildCx, ReactiveId, ReactiveWorld, ReadCx, ReadSignal, ScopeId, WriteCx};
use octara_model::ask::Menu;
use octara_model::input::{Chord, KeyEvent, Modifiers};
use octara_model::theme::Theme;
use octara_model::window::{StatusBarStyle, WindowLevel, WindowMaterial, WindowPresentation};
use std::cell::RefCell;
use std::sync::Arc;

type Shortcut = dyn Fn(&mut WriteCx<'_>);
type CloseRequest = Arc<dyn Fn(&mut WriteCx<'_>) -> bool>;
type PresentationBinding = Box<dyn FnMut(&mut ReactiveWorld) -> Option<PresentationUpdate>>;

#[derive(Default)]
pub(crate) struct WindowAuthoring {
    menus: Vec<ReadSignal<Vec<Menu>>>,
    presentation: Vec<(ReactiveId, PresentationBinding)>,
    shortcuts: Vec<(ScopeId, Chord, Box<Shortcut>)>,
    close_requests: RefCell<Vec<(ScopeId, CloseRequest)>>,
}

impl WindowAuthoring {
    pub(crate) fn prune(&mut self, world: &ReactiveWorld) {
        self.menus.retain(|menu| world.is_alive(menu.id()));
        self.presentation.retain(|(source, _)| world.is_alive(*source));
        self.shortcuts.retain(|(scope, _, _)| world.is_scope_alive(*scope));
        self.close_requests.get_mut().retain(|(scope, _)| world.is_scope_alive(*scope));
    }

    pub(crate) fn menus<'a>(&'a self, world: &'a ReactiveWorld) -> impl Iterator<Item = &'a Menu> + 'a {
        self.menus.iter().flat_map(|menu| world.value::<Vec<Menu>>(menu.id()))
    }

    pub(crate) fn presentation_updates<'a>(&'a mut self, world: &'a mut ReactiveWorld) -> impl Iterator<Item = PresentationUpdate> + 'a {
        self.presentation.iter_mut().filter_map(|(_, sample)| sample(world))
    }

    pub(crate) fn has_shortcut(&self, event: KeyEvent, primary: Modifiers) -> bool {
        self.shortcuts.iter().any(|(_, chord, _)| chord.matches(event, primary))
    }

    pub(crate) fn shortcut(&self, world: &ReactiveWorld, event: KeyEvent, primary: Modifiers) -> Option<(ScopeId, &Shortcut)> {
        self.shortcuts
            .iter()
            .rev()
            .find(|(scope, chord, _)| world.is_scope_alive(*scope) && chord.matches(event, primary))
            .map(|(scope, _, callback)| (*scope, callback.as_ref()))
    }

    pub(crate) fn set_close_request(&self, scope: ScopeId, callback: impl Fn(&mut WriteCx<'_>) -> bool + 'static) {
        let mut callbacks = self.close_requests.borrow_mut();
        callbacks.retain(|(owner, _)| *owner != scope);
        callbacks.push((scope, Arc::new(callback)));
    }

    pub(crate) fn close_request(&self, world: &ReactiveWorld) -> Option<(ScopeId, CloseRequest)> {
        self.close_requests.borrow().iter().rev().find(|(scope, _)| world.is_scope_alive(*scope)).cloned()
    }
}

impl BuildCx<'_> {
    #[doc(hidden)]
    pub fn bind_window_presentation(&mut self, source: impl FnMut(&mut ReadCx<'_>) -> WindowPresentation + 'static) {
        self.bind_presentation(source, |value| PresentationUpdate::All(Box::new(value)));
    }

    #[doc(hidden)]
    pub fn bind_window_title<T: Into<Arc<str>> + Clone + PartialEq + 'static>(
        &mut self,
        source: impl FnMut(&mut ReadCx<'_>) -> T + 'static,
    ) {
        self.bind_presentation(source, |value| PresentationUpdate::Title(value.into()));
    }

    #[doc(hidden)]
    pub fn bind_window_theme(&mut self, source: impl FnMut(&mut ReadCx<'_>) -> Theme + 'static) {
        self.bind_presentation(source, |value| PresentationUpdate::Theme(Box::new(value)));
    }

    #[doc(hidden)]
    pub fn bind_window_material(&mut self, source: impl FnMut(&mut ReadCx<'_>) -> WindowMaterial + 'static) {
        self.bind_presentation(source, PresentationUpdate::Material);
    }

    #[doc(hidden)]
    pub fn bind_window_status_bar(&mut self, source: impl FnMut(&mut ReadCx<'_>) -> StatusBarStyle + 'static) {
        self.bind_presentation(source, PresentationUpdate::StatusBar);
    }

    #[doc(hidden)]
    pub fn bind_window_level(&mut self, source: impl FnMut(&mut ReadCx<'_>) -> WindowLevel + 'static) {
        self.bind_presentation(source, PresentationUpdate::Level);
    }

    fn bind_presentation<T: Clone + PartialEq + 'static>(
        &mut self,
        source: impl FnMut(&mut ReadCx<'_>) -> T + 'static,
        update: impl Fn(T) -> PresentationUpdate + 'static,
    ) {
        let source = self.memo(source).id();
        let mut observed = None;
        let sample: PresentationBinding = Box::new(move |world| {
            if !world.is_alive(source) {
                return None;
            }
            world.ensure_clean(source);
            let revision = world.revision(source);
            if observed.replace(revision) == Some(revision) {
                return None;
            }
            Some(update(world.value::<T>(source).clone()))
        });
        self.authoring.as_mut().expect("presentation requires a window build context").presentation.push((source, sample));
    }

    /// Contribute menus to the window while this scope lives. The builder can
    /// read signals and changes are published with the settled UI.
    #[track_caller]
    pub fn menus(&mut self, build: impl FnMut(&mut ReadCx<'_>) -> Vec<Menu> + 'static) {
        assert!(self.authoring.is_some(), "menus require a window build context");
        let menu = self.memo(build);
        self.authoring.as_mut().expect("window build context").menus.push(menu);
    }

    /// Register a window shortcut for this scope. The focused control can
    /// consume the key first. Later matching registrations take priority.
    #[track_caller]
    pub fn on_chord(&mut self, chord: Chord, run: impl Fn(&mut WriteCx<'_>) + 'static) {
        self.authoring.as_mut().expect("shortcuts require a window build context").shortcuts.push((self.scope, chord, Box::new(run)));
    }

    /// Intercept host close requests while this scope lives. The latest live registration takes priority.
    #[track_caller]
    pub fn on_close_request(&mut self, run: impl Fn(&mut WriteCx<'_>) -> bool + 'static) {
        self.authoring.as_ref().expect("close requests require a window build context").set_close_request(self.scope, run);
    }
}
