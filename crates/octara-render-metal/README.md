# octara-render-metal

The native Metal renderer for Octara on macOS and iOS. It encodes device-independent
plans from `octara-render` into a caller-owned Metal command buffer. The Octara
native host selects it by default on Apple platforms.

The renderer supports retained damage, projective layers, native Metal materials,
and direct sampling of Metal textures, IOSurfaces, and supported packed RGB Core
Video pixel buffers. See the [Octara guide](https://github.com/octoii-works/octara/blob/main/docs/start-here.md)
for host setup and the [compositor design](https://github.com/octoii-works/octara/blob/main/docs/design/v2/21-direct-metal-renderer-and-25d-compositor.md)
for render and presentation details.

The default features require neither wgpu nor Naga. Enable `custom-wgsl` to
translate custom WGSL material shaders to Metal. The opt-in `wgpu` feature
enables parity checks against the portable renderer.

Licensed under either the [MIT License](LICENSE-MIT) or the
[Apache License, Version 2.0](LICENSE-APACHE), at your option.
