//! Deterministic module inputs for headless tests.

use octara::module::{HostLifecycleEvent, ModuleCall, ModuleError, ModuleEvent};
use octara::{AppError, CycleReceipt};

use crate::HeadlessDriver;

impl HeadlessDriver {
    /// Intercept a module call by Rust type before the installed module sees it.
    pub fn stub_module_call<C: ModuleCall>(&mut self, handle: impl FnMut(C) -> Result<C::Output, ModuleError> + 'static) {
        self.app_mut().runtime_mut().stub_module_call::<C>(handle);
    }

    /// Deliver a typed event through the application's normal cycle.
    pub fn emit_module_event<E: ModuleEvent>(&mut self, event: E) -> Result<CycleReceipt, AppError> {
        let receipt = self.app_mut().runtime_mut().emit_module_event(event)?;
        self.sync_native_views()?;
        Ok(receipt)
    }

    /// Drive the same lifecycle stream an actual host supplies.
    pub fn dispatch_lifecycle(&mut self, event: HostLifecycleEvent) -> Result<CycleReceipt, AppError> {
        let receipt = self.app_mut().runtime_mut().dispatch_lifecycle(event)?;
        self.sync_native_views()?;
        Ok(receipt)
    }
}
