# Build interfaces with Octara 0.0.10

Octara composes views with ordinary Rust functions. The app owns reactive state and the window tree. The host projects the tree into a native window or the browser DOM. The Apple deployment baselines are macOS 15.4 and iOS 18.4. `cargo test -p octara-cli --doc` compiles and runs the Rust examples marked for doctesting.

## Declare windows and interactions

Builders receive `BuildCx` to create state. Dynamic properties receive `ReadCx` to track reads. Event handlers receive `WriteCx` to update state or queue requests. Presentation-only components can return an `El` directly. Use `El::select_none()` to disable text selection for UI text such as button labels.

Use short `#[ui]` functions named after their purpose so Inspect can trace components to their source. Put roles, accessible names, and event handlers on the nodes users interact with. Write static styles as `const STYLE: TwStyle = tw!(...)` and apply them with `.tw(STYLE)`. The `tw!` macro checks style classes at compile time.

Use `.child((a, b, c))` to add a fixed group of children. Tuple members can have different `IntoView` types. Use `.children(iter)` with an array or iterator of same-type children.

```rust
use octara::prelude::*;

const COUNTER_STYLE: TwStyle = tw!("p-4 gap-2");
const BUTTON_STYLE: TwStyle = tw!("px-3 py-2 rounded-md bg-surface");

#[ui]
fn increment_button(count: Signal<u32>) -> El {
    el()
        .tw(BUTTON_STYLE)
        .role(Role::Button)
        .label("Increment")
        .test_id("increment")
        .focusable()
        .select_none()
        .on_press(move |w| w.update(count, |count| *count += 1))
        .child("Increment")
}

#[ui]
fn counter(cx: &mut BuildCx<'_>) -> El {
    let count = cx.signal(0_u32);
    col()
        .tw(COUNTER_STYLE)
        .child((
            text("Counter").role(Role::Heading),
            move |r: &mut ReadCx<'_>| format!("Count {}", r.get(count)),
            increment_button(count),
        ))
}

pub fn startup() -> AppDefinition {
    let options = WindowOptions {
        presentation: WindowPresentation { title: "Counter".into(), ..Default::default() },
        ..Default::default()
    };
    AppDefinition::new().window(WindowDefinition::new(options, counter))
}

# let mut driver = octara_inspect::HeadlessDriver::default();
# let windows = driver.launch(startup()).unwrap();
# assert_eq!(windows.len(), 1);
```

```rust
use octara::prelude::*;

#[ui]
fn tags(labels: &[&str]) -> El {
    row().children(labels.iter().copied().map(text))
}
```

The desktop executable calls `octara_host_native::run_app(startup())`. It uses AppKit on macOS and selects Wayland or X11 on Linux. The iOS host uses UIKit. For a fresh browser session, call `octara_host_web::WebApp::launch(root, startup(), None)` and keep the returned `WebApp` alive. Each `WebApp` owns one browser mount.

Declare a drop target with `El::on_file_drop`. Its handler receives `WriteCx` and a borrowed slice of `DroppedFile` values. Native hosts provide local paths, and browsers provide file display names.

Use `El::on_drag_start(|w| ...)` to start a drag that can leave the app on macOS, Linux, or the Web. Return `Some(Vec<DragItem>)` to offer text, file paths, or images, or return `None` to decline. File paths in a drag are supported on macOS and Linux. Use `El::draggable(DragPayload)` for in-app reordering.

Use `BuildCx::menus` to register menus that update with reactive state, and `BuildCx::on_chord` to register scoped shortcuts. The `mod` modifier maps to Command on macOS and Control elsewhere. Menus and shortcuts can share a command handler. `WindowDefinition` also supports both registration methods.

```rust
use octara::prelude::*;

fn inspector(cx: &mut BuildCx<'_>) -> El {
    let open = cx.signal(false);
    cx.menus(move |r| vec![Menu {
        name: "View".into(),
        items: vec![MenuItem::Action {
            id: "inspector.toggle".into(),
            label: "Toggle Inspector".into(),
            enabled: true,
            checked: Some(r.get(open)),
            shortcut: Some("mod+i".into()),
        }],
    }]);
    cx.on_chord("mod+i".parse().unwrap(), |w| w.request_command("inspector.toggle"));

    el()
        .on_command("inspector.toggle", move |w| w.update(open, |open| *open = !*open))
        .child(show(move |r| r.get(open), |_| text("Inspector")))
}
```

## Configure native windows and geometry

`WindowOptions` sets the initial position, content size limits, resizability, and initial maximized state. Use `BuildCx::window_geometry()` to subscribe to the position, content size, scale factor, and maximized and fullscreen states reported by the host. In event handlers, use `WriteCx` to change the current window’s title or geometry, minimize it, toggle maximization, or activate another window by `WindowId`. `WindowGeometryRequest` updates the fields you set. The host reports the actual geometry.

Assign a stable, unique `persistence_key` to each window whose geometry should be restored. Exporting `RestartState` includes geometry for these windows. Pass that state to `AppDefinition::launch` on the next startup to restore it.

On macOS, `TitlebarStyle::HiddenInset` extends content into the native title bar, and `BuildCx::window_chrome()` reports the title bar height and the space taken by the traffic light buttons. Mark a draggable area with `window_drag_region(true)`. Double-clicking it toggles maximization. Buttons, inputs, and other controls within the area handle their own clicks and focus. `WindowMaterial::Vibrancy` uses AppKit to apply a system material to transparent areas. Other hosts use the window capabilities available on their platform.

Set `WindowPresentation::level` to `WindowLevel::Floating` or `WindowLevel::AlwaysOnTop` to request above-normal stacking on macOS or X11. Set `WindowOptions::non_activating_panel` to create an AppKit `NSPanel` that can take keyboard focus without activating the app. On iOS, set `WindowPresentation::status_bar` to `StatusBarStyle::Light`, `Dark`, or `Hidden`. `Auto` chooses a contrasting foreground based on the window theme. Bind these presentation values with `WindowDefinition::level_with` and `status_bar_with`, or update them in a handler with `WriteCx::set_window_level` and `set_window_status_bar`.

```rust
use octara::prelude::*;

fn workspace() -> WindowDefinition {
    let options = WindowOptions {
        presentation: WindowPresentation {
            title: "Workspace".into(),
            material: WindowMaterial::Vibrancy(VibrancyMaterial::Sidebar),
            ..Default::default()
        },
        min_size: Some(Size::new(480.0, 320.0)),
        titlebar: TitlebarStyle::HiddenInset { traffic_light_offset: Some(point(14.0, 12.0)) },
        persistence_key: Some("workspace".into()),
        ..WindowOptions::default()
    };
    WindowDefinition::new(options, |build| {
        let chrome = build.window_chrome();
        let geometry = build.window_geometry();
        col()
            .child(row()
                .style_with(move |r, style| style.pl(r.get(chrome).leading_inset))
                .window_drag_region(true)
                .child(el().role(Role::Button).focusable().on_press(|w| {
                    w.set_window_title("Workspace settings");
                }).child("Settings")))
            .child(text(move |r: &mut ReadCx<'_>| {
                let size = r.get(geometry).size;
                format!("{:.0} × {:.0}", size.width, size.height)
            }))
    })
}
```

## Use platform modules

Use `octara::module` for typed platform calls. The first-party packages are `octara-webview`, `octara-video`, `octara-picker`, and `octara-haptics`. Add the optional dependencies you need, then register them with `AppDefinition::with_module`. For headless tests, disable the package’s default features and register its pure-Rust `mock()` backend.

Register WebView with `.with_module(octara_webview::module())`. Apple hosts use WKWebView, and the browser uses a sandboxed iframe. macOS uses an underlay by default so Octara popups can receive clicks above the WebView. iOS defaults to an overlay. Within a window, use `octara_webview::send` and a mounted WebView’s `test_id` to send typed commands, and use `on_typed_native` to receive `WebViewEvent`. The Apple backend lets trusted documents call the app through Promise-based RPC. Use `WebViewProps::html` for app-owned documents and `trust_origin` to specify which external origins can use the bridge.

For a custom native overlay, `octara::module::NativeViewFactory::airspace_policy` can return `AirspacePolicy::SnapshotOnOcclude`. The native pipeline can then display a snapshot while an Octara popup covers the view. The Apple WebView factory opts into this behavior.

```rust,ignore
use octara::prelude::*;
use octara_webview::{WebViewCommand, WebViewEvent, WebViewProps, send, view};

fn preview() -> El {
    col().child((
        el().role(Role::Button).focusable().on_press(|w| {
            send(w, "preview", WebViewCommand::Evaluate {
                script: "document.title".into(),
                request_id: Some("preview-title".into()),
            });
        }).child("Read title"),
        view(WebViewProps::html("<title>Preview</title><p>Hello</p>"))
            .test_id("preview")
            .h(260.0)
            .on_typed_native(|_, _, event: &WebViewEvent| {
                if let WebViewEvent::JavaScript { result, .. } = event {
                    println!("{result:?}");
                }
            }),
    ))
}

let app = AppDefinition::new()
    .with_module(octara_webview::module())
    .window(WindowDefinition::new(WindowOptions::default(), |_| preview()));
```

Register `octara_video::module()` to enable playback in `video(VideoSource)` and Markdown video blocks. If playback is unavailable, the view shows the authored poster. Register `octara_picker::module()` for file and folder selection. From a user gesture, use `WriteCx::call_module` with `PickFiles`, `PickFolder`, or `PickSavePath`. Browsers can return file contents and names, and native hosts can return paths. The app chooses the picker to invoke from a `file_dropzone` click handler. Register `octara_haptics::module()` and use `WriteCx::notify_module` to request haptic feedback.

For packaging, modules declare plist, entitlement, framework, and Web header contributions in a schema-v1 `octara-module.toml` at the package root. Put required template variables in `octara.toml` beside the app package’s `Cargo.toml`. Run `octara modules merge` with a new output directory to collect and merge contributions from the app’s resolved Cargo dependencies. When packaging an existing app, pass its base files with `--ios-info-plist`, `--ios-entitlements`, `--macos-info-plist`, or `--macos-entitlements` to retain metadata such as bundle identifiers. Generated plists default to macOS 15.4 and iOS 18.4 when no minimum version is supplied.

Set `MACOSX_DEPLOYMENT_TARGET=15.4` or `IPHONEOS_DEPLOYMENT_TARGET=18.4` for direct Cargo builds targeting the corresponding Apple platform.

```toml
[modules."octara.picker"]
photo_library_usage_description = "Select photos to attach."
```

```sh
cargo run -p octara-cli -- modules merge -p octara-chat-desktop --out-dir target/chat-module-metadata
```

## Read, update, and clean up state

`cx.signal(value)` stores a value in the current scope. Copy the handle into an event handler or derived closure. Use `r.with(handle, f)` to borrow the value and `r.get(handle)` to get an owned value. Both work with handles that implement `Readable<T>`, including signals, store fields, list rows, and bindings. During construction, use `cx.sample(handle)` or `cx.read(|r| ...)` for a one-time, untracked read.

`Binding<T>` is a copyable two-way binding. Use `signal.into()` to bind a signal directly, or `cx.binding(read, write)` to define its read and write behavior. Controlled `octara-kit` widgets accept `Binding<T>`. Read with `r.get(binding)` and write with `w.set(binding, next)`.

```rust
use octara::prelude::*;

fn editor(cx: &mut BuildCx<'_>) -> El {
    let title = cx.persisted("title", || "Untitled".to_owned());
    let value: Binding<String> = title.into();
    let initial_title = cx.sample(value);
    let empty = cx.memo(move |r| r.with(value, String::is_empty));
    col()
        .child(input()
            .value(move |r: &mut ReadCx<'_>| r.get(value))
            .label("Title")
            .on_input(move |w, next| w.set(value, next.into())))
        .child(text(format!("Opened as {initial_title}")))
        .child(show(move |r| r.get(empty), |_| text("Enter a title")))
}
```

`#[derive(Store)]` generates field accessors for a struct. `cx.store(init)` holds the whole value, and `store.field()` returns a copyable field handle. An update to one field notifies subscribers to that field and the containing store. Pass the store handle to read or write the whole value. Mark a nested field with `#[store(nested)]` to access its fields too.

```rust
use octara::prelude::*;

#[derive(Store)]
struct Document {
    title: String,
    views: u32,
}

fn document(cx: &mut BuildCx<'_>) -> El {
    let document = cx.store(Document { title: "Untitled".into(), views: 0 });
    let title = document.title();
    let views = document.views();
    col()
        .child(input()
            .value(title)
            .label("Title")
            .on_input(move |w, next| {
                w.set_if_changed(title, next.to_owned());
            }))
        .child(text(move |r: &mut ReadCx<'_>| format!("Views: {}", r.get(views))))
        .child(el().role(Role::Button).focusable().on_press(move |w| {
            w.mutate(views, |views| {
                if *views == 10 {
                    return false;
                }
                *views += 1;
                true
            });
        }).child("Record view"))
}
```

`cx.memo` computes a derived value and compares the old and new results. `cx.memo_with(init, |r, value| ...)` reuses existing storage. As with `w.mutate(handle, |value| ...)`, the callback returns `true` when the value changes. `cx.trigger()` creates a notification without a payload. Subscribe with `r.track(trigger)` during computation and call `w.notify(trigger)` in a handler to recompute subscribers.

`cx.persisted` identifies restorable state by scope path and domain key. `AppDefinition::state_schema` identifies the data schema. Export `RestartState` before exit, a release switch, or a process handoff, and restore persisted values with the same schema. Use `scope` or a list row for an independent lifetime, and `cx.on_cleanup` to release external resources. The cleanup callback can sample state through `CleanupCx`.

A task is tied to the scope that registered it. The virtual clock drives sleeps, and the runtime polls ready tasks during a Cycle. `task.write` lends a `WriteCx` to a synchronous closure.

```rust
use octara::prelude::*;
use std::time::Duration;

fn delayed_label(cx: &mut BuildCx<'_>) -> El {
    let ready = cx.signal(false);
    cx.spawn("mark-ready", move |task| async move {
        task.sleep(Duration::from_millis(250)).await;
        task.write(|w| w.set(ready, true));
    });
    text(move |r: &mut ReadCx<'_>| if r.get(ready) { "Ready" } else { "Waiting" })
}
```

## Update lists by domain key

`each` reuses mounted rows by domain key and passes the row builder a reference to that key and a copyable `RowSignal<T>`. `virtual_each` chooses which rows to keep mounted based on the viewport measured by the host. It keeps rows with focus or an active pointer contact mounted.

Use `r.get(row)` or `r.with(row, f)` when the row exists. If a row might be missing, use `r.try_get(row)` or `r.try_with(row, f)`, which return `Option`. Outside a list, create the same handle with `cx.row_signal(list, key)`. Reads subscribe only to the specified key. `WriteCx` can also read rows. Update rows with `w.update_row(list, &key, f)`.

Compose existing lists with incremental operators such as `filter_with`, `map_list`, and `group_by`. `filter_with` takes an explicit parameter signal, and the predicate borrows the row and parameter values. For lists derived from an ordinary signal or several reactive reads, use `cx.derive_list` to return rows with domain keys.

```rust
use octara::prelude::*;

fn tasks(cx: &mut BuildCx<'_>) -> El {
    let rows = cx.list::<u64, String>();
    col()
        .child(
            el().role(Role::Button).focusable().test_id("add").on_press(move |w| {
                let id = w.len(rows) as u64;
                w.push_row(rows, id, format!("Task {id}"));
            }).child("Add"),
        )
        .child(each(rows, |_, _, row| {
            text(move |r: &mut ReadCx<'_>| r.try_get(row).unwrap_or_default())
        }))
}

fn selected_tasks(cx: &mut BuildCx<'_>, selection: Signal<Vec<u64>>) -> El {
    let rows = cx.derive_list(move |r| {
        r.with(selection, |ids| ids.iter().map(|&id| (id, format!("Task {id}"))).collect())
    });
    col().child(each(rows, |_, _, row| {
        text(move |r: &mut ReadCx<'_>| r.try_get(row).unwrap_or_default())
    }))
}
```

## Combine Kit styling and Headless behavior

`octara-kit` provides themed controls for native and Web. Use `button(label, size, variant)` to set size and visual emphasis. For custom controls, use themed parts such as `button_part`, `editor_style`, and `popup_surface` from `octara_kit::parts`, and behavior from `octara_kit::headless`. Checkboxes accept `Binding<bool>` or `Binding<Checked>`.

```rust
use octara::prelude::*;
use octara_kit::{ButtonVariant, Size as KitSize, button, headless, toggle_button};

fn toolbar(build: &mut BuildCx<'_>) -> El {
    let pinned = build.signal(false);
    let included = build.signal(Checked::Mixed);
    row()
        .gap(sp(2.0))
        .child(button("Save", KitSize::Md, ButtonVariant::Primary))
        .child(toggle_button("Pin", pinned.into(), KitSize::Sm, ButtonVariant::Ghost))
        .child(headless::checkbox::checkbox(
            text("Include archived").px(sp(2.0)).rounded(Radius::Full),
            included.into(),
        ))
}
```

Use `headless::Selection` for single or multiple selection and `headless::Choice` for options. Give options unique domain keys so their labels can repeat. `PickerOptions::Virtual` mounts options based on the viewport. Supply a row factory to put icons, labels, and descriptions on the option node. Use `PopupOptions` to set popup placement, width, maximum height, and surface style. `ComboboxOptions` configures the list, popup, and input together.

```rust
use octara::prelude::*;
use octara_kit::{ButtonVariant, PickerOptions, PopupOptions, Size as KitSize, button, choice_row, headless, select};

fn model_picker(build: &mut BuildCx<'_>) -> View {
    let choices = build.list_from([
        ("fast", headless::Choice::new("Fast")),
        ("deep", headless::Choice::new("Deep")),
    ]);
    let model = build.signal(Some("fast"));
    select(
        "model",
        "Model",
        headless::Selection::Single(model.into()),
        choices.read_only(),
        PickerOptions::Standard,
        PopupOptions::default(),
        |caption| button(caption, KitSize::Md, ButtonVariant::Secondary),
        choice_row,
    )
}
```

Call `build.provide(octara_kit::KitLocale::zh_cn())` before building controls to give Kit’s built-in labels in that scope a Simplified Chinese translation. Override individual labels through `KitLocale` fields.

Run `cargo run -p octara-examples-desktop --bin window -- showcase workspace` to try the data table, tree view, command palette, context menu, and toast. The workspace also demonstrates Navigator state retention and drag-and-drop reordering.

## Handle touch gestures and input preferences

Use `.on_gesture(|write, event| ...)` to receive pinch, rotation, long press, and double tap. The event follows the logical parent chain until a handler returns `true`. Long press uses the app clock, so a headless test can dispatch a pointer down and advance to its 500 ms deadline.

Set `.scroll_dismisses_keyboard(ScrollDismissesKeyboard::Immediately)` to dismiss the active soft keyboard when a touch drag starts scrolling. `Interactively` dismisses it after a downward drag, and `Never` is the default. Use `.observe_overscroll(signal)` to read the signed vertical offset in logical pixels. The offset is positive above the scrollport and returns to zero on release.

Create `PullToRefresh::new(build, threshold)` and attach it to a vertical scrollport, or pass the same controller and scrollport to `octara_kit::pull_to_refresh` for a themed indicator. The controller starts refreshing after a qualifying release. Call `finish(write)` when the work completes. The Headless controller also accepts a threshold haptics callback.

Set `.keyboard_type(...)`, `.return_key(...)`, `.content_type(...)`, `.autocapitalize(...)`, and `.autocorrect(...)` on `TextInput` to configure the native iOS keyboard and browser input attributes. Continue validating the submitted value in the app.

Pair `TextInput::submit_on_enter()` with `on_submit(...)` to submit on Enter. Use `observe_composition` to track whether the editor has marked text. Native hosts give keys to an active input method during composition. Focused `on_key` handlers and shortcuts can consume non-composing keys before default editing. When implementing a host adapter, send `octara::EditorEvent::{Edit, Select, Command, KeyIntercept}` with the active session and expected revision. Use `EditorCommand` for semantic actions such as deleting a word or extending a selection.

From a user gesture, call `WriteCx::request_service` with `ServiceRequest::WriteClipboard { items }` to write a batch of `ClipboardItem` values. Handle `ServiceOutcome::ClipboardWritten` or `ServiceOutcome::Error` in the callback. Supported formats depend on the host. The Web clipboard path accepts plain text and PNG where browser support and permissions allow it.

Direction defaults to the host locale and inherits along logical parents. Set `.text_direction(Direction::Rtl)` for a right-to-left subtree. Use `.ms(...)` and `.me(...)` for inline margins, `.ps(...)` and `.pe(...)` for inline padding, and `.start(...)` and `.end(...)` for inline insets. Read `build.system_preferences()` to follow changes in locale, text scale, and contrast.

## Contain layout and load images

Use `.contain(Contain::LAYOUT | Contain::PAINT)` or `.contain_content()` to isolate layout and clip painted descendants. Use `.contain_strict()` to also exclude descendants from intrinsic sizing. Give a strictly contained box an explicit size or let its parent determine its size. Native layout applies these flags in `LayoutKernel`, and the Web host maps them to CSS `contain`.

Pass `ImageSource::File(path.into())` to `image_source` for an app-authorized local image on native hosts. For bytes already available to the app, use `ImageSource::Encoded { bytes, mime }`. Apple hosts fetch remote images with URLSession and decode with ImageIO. To enable HTTP image fetching on Linux or override the Apple transport, implement `octara_host_native::MediaTransport` and pass `NativeAppOptions::default().with_media_transport(transport)` to `run_app_with_options(startup(), options)`. The loader still handles caching and decoding, and the transport honors the byte limit, redirect policy, and cancellation.

## Verify native frame pacing and scroll rendering

The Apple hosts use `CADisplayLink` to pace active windows at a rate chosen for the current display. Input can update the next frame while the current frame is on the GPU. Octara presents completed frames in order and applies their native view placements in the same presentation transaction. The clock pauses when the window settles.

The iOS output of `octara modules merge` enables ProMotion through `CADisableMinimumFrameDurationOnPhone` by default and preserves an explicit app value. The display and system determine the available refresh rate.

Native scrollports retain the layout, text, and drawing data of unchanged children. Compatible GPU frames update the spatial offsets and reuse their instance buffers. For a small changed region in a compatible frame, the renderer redraws that region in a persistent backing texture before presenting the complete image. The native accessibility projection starts when the platform or an Inspect audit requests it.

Use `octara inspect` to check committed and presented frame revisions separately. Use `octara audit` for accessible names, focus targets, and clipped text. To measure scroll and list-update work in the Octara checkout, run `cargo run --release -p octara-examples --bin scroll_bench -- 120`. That argument selects the benchmark input rate. Measure presentation with the native host and a display that supports the refresh rate you want to verify.

## Customize streaming Markdown

Add `octara-markdown` and use `markdown(build, source)` to render a string, reactive read, or `TextBuffer`. Use `markdown_stream` for model output and update the completion signal when input ends. The buffer then drains and the document switches to a full parse. If the upstream producer already batches deltas, disable the extra pacing buffer with `StreamingOptions::pace` while keeping stream filtering and text reveal effects. During incremental appends, completed blocks reuse their syntax trees and mounted nodes.

The default streaming reveal follows changes to `SystemPreferences.reduced_motion`. Use `MarkdownConfig::streaming_effects_with` to configure it from reactive state, and `allow_reduced_motion_effects(true)` to explicitly allow the reveal in reduced-motion mode. `StreamingOptions::effects` inherits the scoped configuration by default. Set it to `Some(StreamingEffects::None)` to disable the reveal for one document.

Use `markdown_with(build, source, config)` to configure rendering for one document. Use `ParseOptions` to control syntax such as tables, task lists, and GFM alerts. Choose `MarkdownTheme::document()`, `chat()`, or `compact()`, or use `theme_with` to follow app state. `on_link` receives primary press, secondary press, and hover events with viewport coordinates.

```rust
use octara::prelude::*;
use octara_markdown::{MarkdownConfig, markdown_with};

fn notes(build: &mut BuildCx<'_>, source: TextBuffer) -> El {
    let config = MarkdownConfig::chat().on_link(|w, event| {
        if event.kind != LinkEventKind::Press {
            return false;
        }
        if let Some(path) = event.href.strip_prefix("workspace:") {
            w.set_window_title(path);
            return true;
        }
        false
    });
    markdown_with(build, source, config)
}
```

Use `BuildCx::provide::<Arc<dyn BlockSlotRenderer>>` to register a renderer for code blocks, standalone links, images, videos, equations, and directives in a scope. Return `None` from `render_slot` to use the default component. Signals update the body and completion state of code, equations, and directives, preserving component state when the closing fence arrives. `DocumentStore::snapshot` provides an index of images, links, and slots in document order.

Enable `highlight` for built-in syntax highlighting, or inject an `Arc<dyn CodeHighlighter>`. The highlighter receives source with LF line endings. Enable `media` to load images through `MediaResolver` and play supported `<video>` elements through the host video component. Use `on_context_menu` to discover the available copy formats for the selection, including plain text, Markdown, table Markdown, and image URLs.

Use `TextRecognizer::file_paths` to turn paths in prose into links. The resolver receives the whole path, including line suffixes such as those in `src/lib.rs#L42` and `./Cargo.toml#L10-L20`. Use `LinkPillContent` to update a link pill’s label, icon, and color. In custom rich text, pass `InlinePill::attachment` to `Rich::attachment`. The host measures the label and icon and aligns the pill to the text baseline. The logical text covered by the attachment remains copyable.

Enable `math` and use `$...$` for inline math and `$$...$$` for display math. Equations inherit the theme text color, copying a selection preserves TeX source, and wide display equations scroll horizontally. Reactive color changes through `El::theme_patch` reuse the vector outlines. Use `BuildCx::provide::<Arc<dyn MathRenderer>>` to register a custom vector typesetter or `MarkdownConfig::on_math_error` to receive parse errors. Invalid expressions display their source in monospace.

Enable `composer` and use `MarkdownComposer` to build a multiline input with live Markdown highlighting. Use `TriggerConfig` to set trigger characters such as `@`, `#`, or `/`, then read `active_trigger()` or pass suggestions directly to `editor`. The popup follows the caret. Mentions are atomic for deletion and undo, and `markdown()` serializes them as links such as `[@Alice](user://u_1)`. Provide a scoped `Arc<dyn EditorBackend>` to customize decorations, attachments, and atomic ranges for controlled editors.

Add metadata to a `Completion` with `.icon(...)`, `.description(...)`, and `.category(...)`. Use `editor_with_completion_row` to customize the suggestion layout while retaining the editor’s keyboard navigation and selection behavior.

Run `cargo run -p octara-examples-desktop --bin window -- showcase markdown` to see a streaming document and a composer with completions.

## Verify apps with Inspect

Make the app library and desktop launcher members of the same workspace. Expose `startup() -> AppDefinition` from `my-app/src/lib.rs` and declare UI and test dependencies in `my-app/Cargo.toml`.

```toml
[dependencies]
octara = "0.0.10"

[dev-dependencies]
octara-inspect = "0.0.10"
```

The sibling `my-app-desktop/Cargo.toml` depends on the app library and native host. Call `octara_host_native::run_app(my_app::startup())` from the desktop package’s `src/main.rs`.

```toml
[dependencies]
my-app = { path = "../my-app" }
octara-host-native = "0.0.10"
```

Use `cargo test -p my-app` to run headless tests for the app library on its own. Use `HeadlessDriver::launch(startup())` to launch the same app definition or open a single view for testing. The driver uses `octara-pipeline` for layout and semantics and the `octara-render` CPU renderer for painted screenshots. Use `cargo tree -p my-app` to check for dependencies such as `octara-host-native`, `octara-gpu`, `wgpu`, and `objc2`. Building the app library by itself keeps desktop dependency features out of the test build. [Cargo tree reference](https://doc.rust-lang.org/cargo/commands/cargo-tree.html) · [Cargo feature resolution](https://doc.rust-lang.org/cargo/reference/resolver.html#features)

Both the headless driver and live hosts accept observations and actions through `octara.inspect/2`. Get the complete target from an observation. Observe again after a node is recreated or moved to another window. Inspect receipts report delivery, the current tree revision, whether the window settled, and the committed frame. Hosts that report presentation also include the presented frame. When driving the runtime directly, `App::dispatch` and `App::advance_to` return `Result<CycleReceipt, AppError>`. The Cycle receipt reports delivery, handling, and the number of causal rounds.

```rust
use octara::prelude::*;
use octara_inspect::{HeadlessDriver, protocol::{AgentCommand, AgentRequest}};

let mut driver = HeadlessDriver::default();
let id = driver.app_mut().open_window(WindowOptions::default(), |_| {
    text("Ready").test_id("status")
}).unwrap();
let receipt = driver.execute(AgentRequest::new(Some(id), AgentCommand::Observe));
assert!(receipt.ok && receipt.progress.settled);
let snapshot = receipt.snapshot.unwrap();
assert!(snapshot.nodes.iter().any(|node| node.test_id.as_deref() == Some("status")));
```

The CLI calls the app library’s public `startup` function to launch a headless session. For a package named `my-app`, declare the entry in its `Cargo.toml`.

```toml
[package.metadata.octara]
entry = "my_app::startup"
```

Use `octara vocab` to look up signatures in the current Rust source, `octara inspect` to observe a window, and `octara audit` to check for UI issues.

```sh
cargo run -q -p octara-cli -- vocab BuildCx
cargo run -q -p octara-cli -- inspect --manifest-path examples/Cargo.toml --json
cargo run -q -p octara-cli -- audit --manifest-path examples/Cargo.toml --json
```

Use `octara run` to send JSONL requests through the same protocol. Pass `--connect SOCKET` to connect to a running app. This request lists its windows.

```json
{"protocol":"octara.inspect/2","op":"list_windows"}
```

For native verification after source changes, launch again with `octara launch -p my-app --socket SOCKET --receipt FILE` using a fresh absolute socket path and a new receipt file. `octara launch` stages a copy of the rebuilt executable and records its SHA-256 digest and process ID. Verify that the responding process matches the receipt with `octara inspect --connect SOCKET --receipt FILE --json`. Use `octara run --connect SOCKET --require-build FILE` to check that identity before sending actions. A Cargo rebuild leaves any previously launched process running its staged executable.

Look up runtime diagnostics through `octara::diagnostics` or with `octara explain CODE`. Unsettled projections, stale targets, and destroyed reactive handles have distinct diagnostics.

`octara audit` checks settled windows for text clipping, missing accessible names on interactive controls, and invalid focus targets. The text report points to the defining node at `file:line`. The JSON report uses `octara.audit/1` and lists findings by window, with severity, target, and source location. Error-level findings produce exit code 1. Failure to launch, connect, or complete the audit produces exit code 2. To audit a single window, pass its ID from the `list_windows` receipt to `--window-id`. To audit a running host, pass `--connect SOCKET`.

## Configure development and test builds

Set Cargo profiles in the workspace root `Cargo.toml`. The wildcard overrides below use `opt-level = 2` for dependencies outside the workspace in development and test builds. In a typical downstream app, these include `octara`, `octara-*`, `wgpu`, `parley`, and `taffy`. Workspace members retain the default `opt-level = 0` for faster iteration on app code. [Cargo profile overrides](https://doc.rust-lang.org/cargo/reference/profiles.html#overrides)

```toml
[profile.dev.package."*"]
opt-level = 2

[profile.test.package."*"]
opt-level = 2
```

Dependency optimization can reduce CPU cost for text layout and rendering in debug builds. Results vary by workload. On macOS arm64 at commit `6ea4f49`, increasing external dependencies from `opt-level = 1` to `2` reduced the median startup and initial projection time for a 1,000-item to-do list from 514.30 ms to 463.64 ms. Across 27 Chat and example test executables, `__eh_frame` and `__unwind_info` shrank from a combined 43,613,036 bytes to 40,669,392 bytes, a reduction of about 6.7%. Use `xcrun size -m` to inspect these sections in your own project.

The Octara CLI copies the root manifest’s `[profile.*]` tables into the generated program’s standalone workspace and uses the app’s Cargo target directory. Cargo treats the app library and its dependencies as external to the generated workspace, so the wildcard also applies to any original workspace members it builds. To build the app library at `opt-level = 0`, set an override such as `[profile.dev.package.my-app]` in the applicable profile. Named package overrides take precedence over the wildcard. [Cargo profile overrides](https://doc.rust-lang.org/cargo/reference/profiles.html#overrides)
