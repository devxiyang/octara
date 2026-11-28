---
name: octara
description: "Build and change native or browser UI with the Octara Rust framework: elements, styles, state, lists, input, motion, async work, headless tests and diagnostics. Use whenever code uses the `octara` crates (`use octara::prelude::*`), the task touches Octara views or tests, or an Octara diagnostic code (such as STALE_TARGET or LAYOUT_LOOP) appears."
license: MIT OR Apache-2.0
metadata:
  octara-version: "0.0.11"
---

# Build and verify apps with Octara 0.0.11

Read the [Getting Started guide for this version](references/start-here.md). `octara vocab` lists the prelude, and `octara vocab <name>` shows signatures, import paths, and definitions from the app’s source dependencies. `octara explain <code>` explains diagnostics. `octara skill install` generates an [API index organized by module](references/api/index.md).

## Write views and manage state

- Compose views with ordinary Rust functions. Declare window entry points with `AppDefinition` and `WindowDefinition`. Use `input()` for single-line editing and `textarea()` for multiline editing. Put semantic roles, labels, and handlers on the nodes users interact with.
- Name components with short, purpose-specific `#[ui]` functions so Inspect can show their origins. Declare static styles with `const STYLE: TwStyle = tw!(...)` and apply them with `.tw(STYLE)`. Add fixed children with `.child((a, b, c))`, which accepts a mix of `IntoView` types. Use `.children(iter)` for same-type children.
- Use `BuildCx` to create signals, memos, collections, and task state. Use `ReadCx` for reactive reads and `WriteCx` to update state in handlers. Use `cx.read(|r| ...)` for a one-time, untracked read during construction. Controlled widgets use a copyable `Binding<T>`. Convert a signal into a binding or define its behavior with `cx.binding(read, write)`.
- Use `cx.persisted(key, init)` and declare a state schema for app state that should survive a process restart. Give changing lists stable, unique domain keys. Start incremental derivations from `StateList::read_only()`, or use `cx.derive_list` to build a keyed list from reactive values. Mount rows by key with `each`.
- Declare window menus with `cx.menus` and register shortcuts with `cx.on_chord`. Menu items and shortcuts can invoke the same command. Use `El::select_none()` to disable text selection for UI labels.
- Use `cx.window_chrome()` to lay out a custom title bar and `cx.window_geometry()` to observe the window’s actual position and size. Set `WindowOptions::persistence_key` to include its geometry in `RestartState`. Mark the draggable part of the title bar with `window_drag_region(true)`. Set `WindowPresentation::level` for macOS and X11 stacking and `WindowOptions::non_activating_panel` for an AppKit panel. Set `WindowPresentation::status_bar` for iOS. Bind these presentation values with `WindowDefinition::level_with` and `status_bar_with`.
- Use `octara::module` for typed platform calls. Add the optional packages your app needs and register them with `AppDefinition::with_module`. With `octara-webview`, mount pages using `octara_webview::view` and use typed commands and events for navigation and JavaScript. Native overlay factories can opt into `AirspacePolicy::SnapshotOnOcclude`. Use `octara-video` for playback, `octara-picker` for file selection such as `PickFiles`, and `octara-haptics` for haptic feedback. Use each package’s `mock()` for headless tests and disable default features to omit platform bindings.
- Before packaging, set manifest variables in the app’s `octara.toml` and run `octara modules merge` with a new output directory. The CLI reads declarative `octara-module.toml` files from dependencies to generate Apple plists, entitlements, and framework lists, plus Web headers. The Apple baselines are macOS 15.4 and iOS 18.4.
- Numeric style values are logical pixels. `sp(2.0)` uses theme spacing. Use `FontSizeToken`, `ColorToken`, and `Radius` for theme values and shapes.
- Use `.on_gesture(...)` for named gestures, `.scroll_dismisses_keyboard(...)` for touch scrollports, and `PullToRefresh` when a downward drag should initiate a refresh. Set keyboard traits on `TextInput` for iOS and Web. Set `.text_direction(...)` when a subtree needs a direction override and use logical spacing helpers to follow that direction.
- Use `El::on_drag_start` to return `Some(Vec<DragItem>)` for drag-out or `None` to decline. Use `El::draggable(DragPayload)` for in-app drags. For clipboard writes, call `WriteCx::request_service` with `ServiceRequest::WriteClipboard { items }` from a user gesture and handle the outcome. Formats vary by host.
- Pair `TextInput::submit_on_enter()` with `on_submit(...)` to submit on Enter. Use `observe_composition` to track marked text. Native IME composition gets key priority. Focused `on_key` handlers can intercept non-composing keys. Host adapters use `octara::EditorEvent` with a session and document revision, and `EditorCommand` for semantic editing.
- Apply `.contain_content()` for layout and paint containment or `.contain_strict()` when descendants should also be excluded from intrinsic sizing. Both use the native layout kernel or Web CSS `contain`.
- Use `image_source(ImageSource::File(path.into()))` for app-authorized native image paths. Apple uses URLSession and ImageIO. Linux HTTP image fetches require a `MediaTransport` supplied through `NativeAppOptions::with_media_transport` and `octara_host_native::run_app_with_options`.
- Set button size and emphasis with `octara_kit::button(label, size, variant)`. For custom controls, use themed styles from `octara_kit::parts` and behavior from `octara_kit::headless`. Call `build.provide(KitLocale::zh_cn())` to give Kit controls in that scope Simplified Chinese labels.
- Use `headless::Choice` for options and `headless::Selection` for single or multiple selection. Configure the list with `PickerOptions` and a row factory, and configure the popup with `PopupOptions`. `ComboboxOptions` combines list, popup, and input settings.
- Use `markdown_with(build, source, MarkdownConfig::chat())` to customize Markdown parsing, layout, links, and code blocks. Provide an `Arc<dyn MarkdownRenderer>` in scope to share a renderer across `markdown(build, source)` calls. `TextBuffer` supports incremental appends, and the block cache preserves nodes and selections.
- Use `markdown_stream` to render model output. The reveal follows the system’s reduced-motion preference by default. Use `MarkdownConfig::streaming_effects_with` to configure it reactively. Customize code or chart blocks with `BlockSlotRenderer` and update link pill labels and icons reactively with `LinkPillContent`.
- Embed pills in rich text with `InlinePill::attachment`. The host positions them using measured text metrics. Selection and copying use the logical text in the attachment’s range.
- Enable `octara-markdown`’s `math` feature to render `$...$` and `$$...$$` as vector math while keeping the TeX source copyable. Enable `composer` and use `MarkdownComposer` for caret-anchored completions. Add icons, descriptions, and categories to `Completion`, or supply a custom row with `editor_with_completion_row`.

## Verify the app

In one workspace, expose `startup() -> AppDefinition` from the `my-app` library and add `octara-inspect` to the library’s `[dev-dependencies]`. Put the native host dependency in the sibling `my-app-desktop` package. Its `src/main.rs` calls `octara_host_native::run_app(my_app::startup())`.

```toml
[dependencies]
my-app = { path = "../my-app" }
octara-host-native = "0.0.11"
```

Use `cargo test -p my-app` to test the app library on its own. `HeadlessDriver::launch(startup())` launches the app definition with the layout pipeline and CPU renderer. Use `cargo tree -p my-app` to check for native test dependencies such as `octara-host-native`, `octara-gpu`, `wgpu`, or `objc2`. [Cargo tree reference](https://doc.rust-lang.org/cargo/commands/cargo-tree.html)

Give test nodes a stable `test_id`. Send `Observe` to get a target, then `Act` and assert the updated state. When driving the app directly, `App::dispatch` and `App::advance_to` return `Result<CycleReceipt, AppError>`.

1. Set `entry = "my_app::startup"` in Cargo’s `[package.metadata.octara]`. The function returns `AppDefinition`. Run `octara check -p my-app` to check diagnostics and settling at phone and desktop sizes in light and dark themes. Run `octara inspect -p my-app --json` to inspect a headless window. Run `octara audit -p my-app --json` to check text clipping, accessible names, and focus targets. Use `provenance.file` and `provenance.line` to locate findings. Error-level findings produce exit code 1.
2. Use `octara launch -p my-app --socket SOCKET --receipt FILE` to launch the same entry point in a native window. Choose an unused absolute Unix socket path and a new receipt file. The receipt identifies the staged executable by SHA-256 digest and process ID. Use `octara inspect --connect SOCKET --receipt FILE --json` to verify that process. After a source change, launch again with a fresh socket and receipt before validating the change. Cargo rebuilds leave existing processes on their staged executable.
3. Use `octara run` to observe and act in one session, or use `octara run --connect SOCKET --require-build FILE` to check the launch receipt before driving a running host. Write one `octara.inspect/2` request per line. Start with `{"protocol":"octara.inspect/2","op":"list_windows"}` or `{"protocol":"octara.inspect/2","op":"observe"}` to locate windows and nodes. In each `act`, pass the current snapshot’s `target` verbatim. Use the node’s geometry for coordinates and verify the window and interaction target. Look up the arguments with `octara vocab AgentRequest` and `octara vocab AgentAction`.
4. Check `delivery`, `progress.settled`, `progress.committed_frame`, and `progress.presented_frame` after each action. Verify the value and focus with the next `observe`. If you get `STALE_TARGET` or the connection drops, observe again before deciding whether to retry. Use the `painted` capture source to verify rendered pixels, and test system input methods and presentation in the actual host.

The native host uses direct Metal on macOS and iOS and `wgpu` on Linux. On Apple, the native host’s `wgpu` feature selects that backend. macOS uses a Core Video display link for interactive frames, and iOS uses `CADisplayLink`. The generated iOS plist opts into ProMotion unless the app overrides it. Compatible scroll frames reuse retained GPU instances and transfer small spatial updates. Small damage can be redrawn in the backing texture before presentation. Use `octara audit` to request the accessibility projection. For timing work, record the input and actual presentation rates separately and use Release builds.

## Configure development and test builds

Set `opt-level = 2` in `[profile.dev.package."*"]` and `[profile.test.package."*"]` in the workspace root `Cargo.toml`. This applies to external dependencies such as `octara`, `octara-*`, `wgpu`, `parley`, and `taffy`. Workspace members use the default `opt-level = 0`. [Cargo profile overrides](https://doc.rust-lang.org/cargo/reference/profiles.html#overrides)

Dependency optimization can reduce CPU cost for layout and rendering in debug builds. Results vary by workload. On macOS arm64, raising external dependencies from `opt-level = 1` to `2` at commit `6ea4f49` reduced the combined size of `__eh_frame` and `__unwind_info` by about 6.7% across 27 Chat and example test executables.

The CLI copies the root manifest’s `[profile.*]` tables into the generated program’s standalone workspace. There, the app library and its dependencies count as external dependencies, so the wildcard also applies to any original workspace members it builds. Named package overrides take precedence. For example, set `opt-level = 0` in `[profile.dev.package.my-app]` to override the wildcard for the app library. [Cargo profile overrides](https://doc.rust-lang.org/cargo/reference/profiles.html#overrides)
