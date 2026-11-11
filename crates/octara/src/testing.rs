//! Headless driving for tests and agents: the `Host` interface and a from-scratch consistency
//! check (self-adjusting computation / Nominal Adapton): after every event, the incrementally
//! updated tree must equal the tree rebuilt from scratch from the same state.

use crate::app::{App, Event, Work};
use crate::diagnostics::Diagnostic;
use crate::kernel::Kernel;
use std::cell::{Ref, RefMut};

/// Something tests can drive: an app, or an app wrapped in a checker.
pub trait Host {
    fn kernel(&self) -> Ref<'_, Kernel>;
    fn kernel_mut(&mut self) -> RefMut<'_, Kernel>;
    fn dispatch(&mut self, ev: Event) -> Result<(), String>;
    fn hot_reload(&mut self);
    fn diagnostics(&self) -> Vec<Diagnostic>;
    fn work(&self) -> Work;
    fn reset_work(&mut self);
}

impl Host for App {
    fn kernel(&self) -> Ref<'_, Kernel> {
        App::kernel(self)
    }
    fn kernel_mut(&mut self) -> RefMut<'_, Kernel> {
        App::kernel_mut(self)
    }
    fn dispatch(&mut self, ev: Event) -> Result<(), String> {
        App::dispatch(self, ev)
    }
    fn hot_reload(&mut self) {
        App::hot_reload(self)
    }
    fn diagnostics(&self) -> Vec<Diagnostic> {
        App::diagnostics(self)
    }
    fn work(&self) -> Work {
        App::work(self)
    }
    fn reset_work(&mut self) {
        App::reset_work(self)
    }
}

/// An app checked against a from-scratch rebuild after every event.
pub struct Checked(pub App);

impl Host for Checked {
    fn kernel(&self) -> Ref<'_, Kernel> {
        self.0.kernel()
    }
    fn kernel_mut(&mut self) -> RefMut<'_, Kernel> {
        self.0.kernel_mut()
    }
    fn dispatch(&mut self, ev: Event) -> Result<(), String> {
        let label = format!("{ev:?}");
        self.0.dispatch(ev)?;
        let incremental = self.0.kernel().tree_dump();
        self.0.hot_reload();
        let scratch = self.0.kernel().tree_dump();
        if incremental != scratch {
            return Err(format!(
                "after {label}: incremental and from-scratch trees differ\n--- incremental\n{incremental}--- scratch\n{scratch}"
            ));
        }
        Ok(())
    }
    fn hot_reload(&mut self) {
        self.0.hot_reload()
    }
    fn diagnostics(&self) -> Vec<Diagnostic> {
        self.0.diagnostics()
    }
    fn work(&self) -> Work {
        self.0.work()
    }
    fn reset_work(&mut self) {
        self.0.reset_work()
    }
}

/// Fails if the incremental tree differs from a from-scratch rebuild right now.
pub fn check_consistent(app: &mut App) -> Result<(), String> {
    let incremental = app.kernel().tree_dump();
    app.hot_reload();
    let scratch = app.kernel().tree_dump();
    if incremental == scratch { Ok(()) } else { Err(format!("--- incremental\n{incremental}--- scratch\n{scratch}")) }
}
