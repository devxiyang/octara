use std::rc::Rc;

use super::{BuildCx, WriteCx};
use crate::Signal;

type Close = Rc<dyn Fn(&mut WriteCx<'_>)>;

/// The closest enclosing close action. Handles expire when their provider's scope retires.
#[derive(Clone, Copy)]
pub struct CloseHandle(Signal<Close>);

impl CloseHandle {
    /// Calls the provider's close action if its scope is still live.
    pub fn close(self, write: &mut WriteCx<'_>) {
        if !write.world.is_alive(self.0.id()) {
            return;
        }
        let (scope, window) = write.world.owner(self.0.id());
        if !write.world.is_scope_alive(scope) {
            return;
        }
        let close = write.get(self.0);
        let mut owner = WriteCx::new(write.world, scope, window.or(write.window));
        owner.tasks = write.tasks.clone();
        owner.commands = write.commands.as_deref_mut();
        owner.tree = write.tree.filter(|_| owner.window == write.window);
        close(&mut owner);
    }
}

impl BuildCx<'_> {
    /// Makes a scope-owned close action available to descendants, including portal content.
    pub fn provide_close(&mut self, close: impl Fn(&mut WriteCx<'_>) + 'static) -> CloseHandle {
        let handle = CloseHandle(self.signal(Rc::new(close) as Close));
        self.provide(handle);
        handle
    }

    /// Resolves the closest close action in the logical scope hierarchy.
    pub fn close_handle(&self) -> Option<CloseHandle> {
        self.context()
    }
}

impl WriteCx<'_> {
    /// Looks up the close action for the handler's scope.
    pub fn close_handle(&self) -> Option<CloseHandle> {
        self.world.context(self.scope)
    }
}
