//! Scope-owned declarations that belong to a mounted window.

use crate::{BuildCx, ReactiveWorld, ReadCx, ReadSignal, ScopeId, WriteCx};
use octara_model::ask::Menu;
use octara_model::input::{Chord, KeyEvent, Modifiers};
use std::cell::RefCell;
use std::sync::Arc;

type Shortcut = dyn Fn(&mut WriteCx<'_>);
type CloseRequest = Arc<dyn Fn(&mut WriteCx<'_>) -> bool>;

#[derive(Default)]
pub(crate) struct WindowAuthoring {
    menus: Vec<ReadSignal<Vec<Menu>>>,
    shortcuts: Vec<(ScopeId, Chord, Box<Shortcut>)>,
    close_requests: RefCell<Vec<(ScopeId, CloseRequest)>>,
}

impl WindowAuthoring {
    pub(crate) fn prune(&mut self, world: &ReactiveWorld) {
        self.menus.retain(|menu| world.is_alive(menu.id()));
        self.shortcuts.retain(|(scope, _, _)| world.is_scope_alive(*scope));
        self.close_requests.get_mut().retain(|(scope, _)| world.is_scope_alive(*scope));
    }

    pub(crate) fn menus<'a>(&'a self, world: &'a ReactiveWorld) -> impl Iterator<Item = &'a Menu> + 'a {
        self.menus.iter().flat_map(|menu| world.value::<Vec<Menu>>(menu.id()))
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
