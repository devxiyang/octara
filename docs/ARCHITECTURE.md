# Octara v2 implementation overview

The architecture specification defines crate boundaries, data structures, host interfaces, and acceptance checks. `python3 tools/check_layers.py` checks all 27 workspace members and their production dependency edges, including optional, build, and platform-specific dependencies. [v2 architecture specification](design/architecture-v2.md)

| Layer | Implementation |
| --- | --- |
| Values and protocols | `octara-model` defines identities, style IR, paint values, input protocols, and diagnostics. `octara-icons` supplies vector paths, and `octara-svg` parses SVG on demand. `octara-release` independently verifies signatures and release sequence numbers. |
| Tree | `octara-tree` owns compact nodes, sparse side tables, logical editor sessions, and interaction arbitration. |
| Runtime and views | `octara-runtime` owns type-partitioned storage, the reactive dependency graph, tasks, and the Cycle. `octara` exposes view and window primitives. `octara-headless` attaches behavior and semantics to caller-provided elements. `octara-kit` composes Headless parts with theme, sizing, and layout presets. |
| Documents and editing | `octara-markdown` builds streaming documents, replaceable block slots, vector math, and rich-text composition on the public authoring API. |
| Native | `octara-pipeline` runs Taffy, Parley, animation, subtree paint compilation, and incremental AccessKit projection. `octara-render` plans and encodes drawing. `octara-gpu` manages background queues and native readback. `octara-host-native` connects input, native views, system services, and presentation through per-window capabilities. |
| Web | `octara-host-web` stages DOM patches from `TreeDelta`, rolls back failed projections through an undo log, and reads layout on demand through a batched numeric bridge. The browser animates opacity for committed exit elements. WebGPU is opt-in. |
| Platform modules | `octara-webview`, `octara-video`, `octara-picker`, and `octara-haptics` are optional packages built on `octara::module`. Apps register the modules they use. Hosts supply surfaces and lifecycle events through the shared interface. |
| Verification and delivery | `octara-inspect` provides guarded actions, replay, deterministic multi-window simulation, and transport. `octara-cli` provides source vocabulary, skills, module metadata merging, and release commands. |

## The Cycle

1. Ingress validates targets and editor sessions. The interaction arbiter applies the same input rules across native, Web, and Inspect.
2. React runs events and ready tasks, then settles derived values, structure, bindings, and watch sources in topological order. Views write static and dynamic properties into the current window’s `TxBuilder`.
3. Commit uses `Tree::apply(Transaction)` to validate topology, versions, and contents before updating nodes and indices. `Tree::apply_runtime(RuntimeOp)` updates transient interaction state. Both paths emit `TreeDelta`.
4. Project passes deltas to the active `HostEngine` and settles layout and feedback within 16 causal rounds.
5. Publish emits frames and observation receipts. If projection fails, the host rolls back its staged state and the runtime retains deltas for reprojection.

At the app boundary, `App::dispatch` and `App::advance_to` return `Result<CycleReceipt, AppError>`. The receipt reports delivery, handling, and the number of causal rounds.

Lists identify rows by domain key, and text input targets editor sessions. Derived values and lists settle on demand in the same graph. Native layout and browser measurement share a logical coordinate protocol. Tests can create independent apps on separate threads and replay input with a virtual clock and random seed.

[Reactive runtime](design/v2/02-reactive-runtime.md) · [Tree and input arbitration](design/v2/03-tree-microkernel.md) · [Native projection](design/v2/04-native-pipeline-render-host.md) · [Web projection](design/v2/05-web-host.md)

## Windows and native views

During Ingress, the runtime receives per-window `ChromeChanged` and `GeometryChanged` events and updates signals that views can observe. The host routes window control commands by `WindowId`. The `persistence_key` includes geometry in `RestartState`. The tree’s interaction priorities let buttons, editors, and links inside a draggable title bar keep their own input targets.

On macOS, AppKit handles native title bar layout and system materials. The WebView module uses the same typed native node commands and events on macOS and iOS. The Metal underlay on macOS leaves a transparent region for the native view, while Octara draws its popups above that region and handles their input. [Window, rich-text, and native WebView specification](design/v2/08-native-chrome-rich-text-webview-windows.md)

Modules can declare packaging contributions in an `octara-module.toml` manifest. `octara modules merge` resolves the app’s Cargo dependencies, substitutes variables from the app’s `octara.toml`, and merges contributions for Apple plists, entitlements, and frameworks, and for Web headers. [Module architecture and packaging](design/v2/14-octara-module-architecture.md)

## Component behavior and appearance

Headless controllers compose state, bindings, and collections through copyable handles. Listbox and Combobox, for example, share keyboard navigation, domain keys, active rows, and selection rules. The window handles actual focus, input methods, exit barriers, and drag-and-drop arbitration. Kit applies themes and layouts and exposes the same behavior interfaces through `octara_kit::headless`. [App primitives and components](design/v2/07-app-primitives-and-headless-kit.md)

## Releases

Release packages use Ed25519 signatures and sequence numbers to protect accepted release state. `octara-release::Store` verifies the installation target and signature before calling the app’s prepare function. That function can use `octara::declarative::ScenePacket` to decode `octara.scene/2` and compile palettes, layouts, and app-resolved commands into a `Transaction` for tree preflight. The app holds a launch lease and calls `Session::confirm` once the first frame meets its confirmation criteria. `App::export_persisted` and `App::import_persisted` handle explicitly declared persistent state.

[Inspect protocol, simulation, and release specification](design/v2/06-inspect-dst-release-migration.md)
