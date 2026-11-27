//! Native startup for the Chat reference app.

use octara::prelude::*;

pub fn with_platform_modules(definition: AppDefinition) -> AppDefinition {
    definition.with_module(octara_picker::module()).with_module(octara_video::module())
}

/// Deterministic application definition for the native Inspect launcher.
pub fn startup() -> AppDefinition {
    with_platform_modules(octara_chat::startup())
}

pub fn run<V: IntoView>(
    options: WindowOptions,
    root: impl FnOnce(&mut BuildCx<'_>) -> V + 'static,
) -> Result<(), octara_host_native::ShellError> {
    octara_host_native::run_app(with_platform_modules(AppDefinition::new().window(WindowDefinition::new(options, root))))
}
