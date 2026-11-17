//! Execution permissions and owner restoration. Restoration alone is not admission.

use super::*;

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub(crate) enum Ctx {
    Idle,
    /// A component body or branch being built; runs once.
    Build,
    /// Inside a memo.
    Derive,
    /// Inside a binding, region or watch source.
    Observe,
    /// A handler or watch callback.
    Handler,
    /// A named value initializer: sampled reads, no mutation of existing state.
    Init,
    /// Teardown may sample retiring state but cannot register new work.
    Cleanup,
    /// Inside a view's closure (`filter`, `sort_by`, `group_by`, `map`, `join`): a pure function
    /// of the item, run per item event; reads are not tracked and are reported.
    View,
}

/// Framework-visible operations. Release remains available after failure/retirement.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Operation {
    Write,
    Register,
    Release,
    Shutdown,
}

/// Captured scope and window. None explicitly clears the ambient owner.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ExecutionOwner {
    pub(crate) scope: Option<ScopeId>,
    pub(crate) window: Option<Owner>,
}

impl ExecutionOwner {
    pub(crate) fn is_live(self, runtime: &Runtime) -> bool {
        self.scope.is_none_or(|scope| runtime.scope_alive(scope))
    }
}

impl Runtime {
    pub fn ctx(&self) -> Ctx {
        self.ctx.get().unwrap_or(Ctx::Idle)
    }

    /// The owner now.
    pub(crate) fn owner(&self) -> Option<Owner> {
        self.owner.get()
    }

    /// Restore the registration window; None deliberately clears the ambient window.
    pub(crate) fn with_owner<R>(&self, owner: Option<Owner>, f: impl FnOnce() -> R) -> R {
        let _owner = Restore::set(&self.owner, owner);
        f()
    }

    pub fn with_frame<R>(&self, ctx: Ctx, observer: Option<Idx>, scope: Option<Option<ScopeId>>, f: impl FnOnce() -> R) -> R {
        let _ctx = Restore::set(&self.ctx, Some(ctx));
        let _observer = Restore::set(&self.observer, observer);
        let _scope = scope.map(|scope| Restore::set(&self.scope, scope));
        let _sampling = Restore::set(&self.sampling, ctx == Ctx::Init);
        f()
    }

    pub(super) fn sample<R>(&self, f: impl FnOnce() -> R) -> R {
        let _observer = Restore::set(&self.observer, None);
        let _sampling = Restore::set(&self.sampling, true);
        f()
    }

    pub(crate) fn with_scope<R>(&self, scope: Option<ScopeId>, f: impl FnOnce() -> R) -> R {
        let _scope = Restore::set(&self.scope, scope);
        f()
    }

    /// Eligibility for a new framework invocation or publication, not a lasting permit.
    pub(crate) fn admits(&self, owner: ExecutionOwner) -> bool {
        self.assert_healthy();
        owner.is_live(self)
    }

    pub(crate) fn capture_owner(&self) -> ExecutionOwner {
        ExecutionOwner { scope: self.scope.get(), window: self.owner.get() }
    }

    /// Switch ownership without changing the caller's phase or collector (App::run).
    pub(crate) fn with_owned<R>(&self, owner: ExecutionOwner, f: impl FnOnce() -> R) -> R {
        self.with_owner(owner.window, || self.with_scope(owner.scope, f))
    }

    /// Restore an admitted invocation's owner and phase. This does not grant eligibility.
    /// Teardown deliberately uses this even when the captured scope is retiring.
    pub(crate) fn with_execution<R>(&self, owner: ExecutionOwner, ctx: Ctx, observer: Option<Idx>, f: impl FnOnce() -> R) -> R {
        self.with_owned(owner, || self.with_frame(ctx, observer, None, f))
    }

    pub(crate) fn allows(&self, operation: Operation, at: &'static Location<'static>) -> bool {
        if matches!(operation, Operation::Write | Operation::Register) {
            self.assert_healthy();
        }
        if operation == Operation::Register && !self.capture_owner().is_live(self) {
            self.diag(&codes::WRITE_DISPOSED, "cannot register context or work in a retired scope".into(), at);
            return false;
        }
        let code = match (self.ctx(), operation) {
            (Ctx::Derive | Ctx::Observe | Ctx::View, _) => &codes::WRITE_IN_DERIVE,
            (Ctx::Init, _) | (Ctx::Build, Operation::Write | Operation::Shutdown) => &codes::WRITE_IN_BUILD,
            (Ctx::Cleanup, Operation::Register | Operation::Shutdown) => &codes::WRITE_DISPOSED,
            _ => return true,
        };
        self.diag(code, format!("{operation:?} is not allowed in this execution phase; rejected"), at);
        false
    }

    /// Handle-based operations can run without an ambient App. If there is a caller,
    /// check its phase before switching owners, including cross-App cancellation.
    pub(crate) fn caller_allows(operation: Operation, at: &'static Location<'static>) -> bool {
        let caller = CURRENT.with(|current| current.borrow().clone());
        caller.is_none_or(|runtime| runtime.allows(operation, at))
    }
}
