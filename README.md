# Octara 0.0.11

Build native and Web interfaces in Rust. Explicit capability contexts let you compose views and update state. Direct AppKit, UIKit, Wayland, and X11 hosts render on the GPU. The Web host projects views into the DOM. Use the same target and editor session protocol to test interactions, track frame progress, and replay failures.

Octara 0.0.11 uses `octara-render-metal` by default on macOS and iOS. The default Apple host dependency graph has no `wgpu` or `naga`. The release adds `Transform3D` for perspective effects and zero-copy compositing of external textures, plus progressive AppKit live resize and Metal frame-stage timings. [Release roadmap and hardware measurements](docs/design/roadmap.md#0011--direct-apple-metal-renderer-octara-render-metal-ultra-low-latency-presentation--25d--zero-copy-3d-compositor-released)

- Start with the [Getting Started guide](docs/start-here.md).
- Use themed controls from `octara-kit`, or compose behavior from `octara_kit::headless` with styles from `octara_kit::parts`.
- Render streaming documents with `octara-markdown` and customize blocks such as code and charts. Link pills use measured text sizes, and streaming reveals respect the system’s reduced-motion preference by default. Enable `math` for vector math on native and Web, or `composer` for rich caret-anchored completions, atomic mentions, and Markdown serialization.
- Add WebView, video playback, file pickers, or haptics through optional `octara::module` packages. [Module architecture and packaging](docs/design/v2/14-octara-module-architecture.md)
- Run `octara audit` to find clipped text, interactive controls with missing accessible names, and invalid focus targets.
- Explore the [crate boundaries and Cycle](docs/ARCHITECTURE.md) or the [v2 architecture specification](docs/design/architecture-v2.md).
- Run the [examples](examples/README.md) and the [streaming Chat app](apps/chat/README.md).

Run these commands from the workspace root with Rust 1.100 or later. The Apple deployment baselines are macOS 15.4 and iOS 18.4.

```sh
cargo run -p octara-examples-desktop --bin window -- counter
cargo run -p octara-examples-desktop --bin window -- showcase workspace
cargo run -q -p octara-cli -- vocab BuildCx
cargo run -q -p octara-cli -- audit --manifest-path examples/Cargo.toml --json
```

Run the workspace tests, dependency boundary check, Markdown feature tests, and Wasm checks.

```sh
python3 tools/check_layers.py
OCTARA_VERIFY_RELEASE_API=1 cargo test --workspace
cargo test -p octara-cli --doc
cargo test -p octara-markdown --all-features
cargo clippy --workspace --all-targets -- -D warnings
cargo check --target wasm32-unknown-unknown -p octara-markdown
cargo check --target wasm32-unknown-unknown -p octara-markdown --all-features
cargo check --target wasm32-unknown-unknown -p octara-host-web
cargo check --target wasm32-unknown-unknown -p octara-host-web --features webgpu
```

On a Mac with the iOS Simulator SDK and the matching Rust target, check the native host.

```sh
IPHONEOS_DEPLOYMENT_TARGET=18.4 cargo check --target aarch64-apple-ios-sim -p octara-host-native
```

Preflight all 24 publishable Octara 0.0.11 crates together. Cargo resolves internal dependencies in the release batch and builds from the unpacked archives.

```sh
cargo publish --workspace --dry-run --locked
```

Reuse the workspace Cargo cache. Clean up temporary captures, sockets, and test processes after verification. [Repository conventions](AGENTS.md)

Octara is licensed under [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE).
