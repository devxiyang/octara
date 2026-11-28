# Examples and window verification

Build and run from the repository root. `octara-examples` contains the example apps, headless tests, and CPU tools. `octara-examples-desktop` contains the native launcher shared by desktop and iOS, plus GPU tests.

| Feature | Command |
| --- | --- |
| Counter and restorable state | `cargo run -p octara-examples-desktop --bin window -- counter` |
| Stable keys, filtering, sorting, and grouping | `cargo run -p octara-examples-desktop --bin window -- todo` or `contacts` |
| Chat replies, virtualized rows, and an editor | `cargo run -p octara-examples-desktop --bin window -- chat_virtual` |
| Kit forms, comboboxes, and popups | `cargo run -p octara-examples-desktop --bin window -- showcase forms` or `choices`, `interaction`, `controls` |
| Release workspace with a Navigator, tree view, data table, drag-and-drop reordering, command palette, context menu, and toast | `cargo run -p octara-examples-desktop --bin window -- showcase workspace` |
| Live Markdown, selectable rich text, canvas, and native WebView | `cargo run -p octara-examples-desktop --bin window -- showcase markdown` or `text`, `drawing`, `web` |
| Multiple windows and per-window undo history | `cargo run -p octara-examples-desktop --bin window -- windows` or `undo` |
| Virtualized row geometry and offscreen editors | `cargo run -p octara-examples-desktop --bin virtual_geometry` or `--bin virtual_interaction` |
| System input methods and marked text | `cargo run -p octara-examples-desktop --bin ime`. [IME walkthrough](IME.md) |
| Interactive GPU materials, flow fields, and refraction | `cargo run -p octara-examples-desktop --features wgpu --bin window -- gpu_lab` |
| CPU reference screenshots | `cargo run -p octara-examples --bin screenshot -- /tmp/octara-shots 2 --bundled` |
| CPU projection benchmark | `cargo run --release -p octara-examples --bin bench` |
| Retained scroll, Markdown flings, and list switches | `cargo run --release -p octara-examples --bin scroll_bench -- 120` |
| wgpu painting and cache benchmarks | `cargo run --release -p octara-examples-desktop --features wgpu --bin gpu_bench -- gpu` or `-- gpu-retain` |

CPU screenshots use bundled fonts by default. To use system fonts, add the Cargo flag `--features system-fonts` and omit `--bundled`.

Run `cargo test -p octara-examples` to verify headless app behavior and CPU rendering. Run `cargo test -p octara-examples-desktop` to verify GPU frame-planning budgets, shader fallbacks, and CPU/GPU pixel parity for rich text. Set `OCTARA_REQUIRE_GPU=1` to require a usable GPU adapter for the parity tests.

`gpu_lab replay 60` and `gpu_lab replay 120 stress` update shaders continuously using the app clock. For `gpu_lab input-pressure`, click **Start workload**, then use the system keyboard, clicks, and drags to interact with the controls. Set `OCTARA_GPU_LAB_TRANSITION=1` to show a local GPU transition in the glass panel.

In the release workspace, filter builds by platform and click a table header to sort. Open a review and edit its notes, then navigate back and forward to see the draft the Navigator retained. Reorder the review queue by dragging or by using Space, the Up and Down arrows, and Enter. **Notify on move** uses the Headless Checkbox with custom styling. Right-click **Queue actions**, or focus the button and press Shift+F10, to reset the queue order.

The Markdown example uses a local conversation about a lunar eclipse. Click **Replay answer** to see the streaming reveal and citation label updates. Expand **Observation plan** to read the plan, then click an inline thumbnail or standalone image to open a preview. Type **@Luna** to insert a contact, **/replay** to replay the answer, or **/question** to fill in the sample question. Use the arrow keys and Enter to select a completion.

## Inspect a live window

```sh
cargo build -p octara-examples-desktop --bin window -p octara-cli --bin octara
python3 -B examples/live_inspect.py --window target/debug/window
```

The script opens the counter on an interactive desktop, reads a live window receipt, clicks the button using a guarded target, and verifies the count change, presentation of a new frame, and PNG capture. It cleans up the process, socket, and capture on exit.

For manual testing, set `OCTARA_INSPECT` to an unused Unix socket path. In another terminal, use `target/debug/octara inspect --connect "$socket" --json` to get a receipt, with `socket` set to the path from the first terminal. Use `target` from `snapshot.nodes` in action requests. Reopening the app or recreating the window produces new targets. [Protocol operations and receipts](../docs/design/v2/06-inspect-dst-release-migration.md)

```sh
socket=$(mktemp -d /tmp/octara-example.XXXXXX)/inspect.sock
OCTARA_INSPECT="$socket" target/debug/window counter
```

Run the `trackpad` example to test trackpad scrolling. Flick the inner viewport, lift your fingers, and watch the direct scroll and momentum. Use **Reset** to recreate the viewport. Set `OCTARA_TRACE_SCROLL=1` to collect host scroll traces.

## iOS Simulator

Build and install the same examples on a booted iOS 18.4 or later iPhone simulator on Apple Silicon. Set `device` to its UDID.

```sh
IPHONEOS_DEPLOYMENT_TARGET=18.4 cargo build --locked --target aarch64-apple-ios-sim -p octara-examples-desktop --bin panel_scroll
sim_run=$(mktemp -d /tmp/octara-ios.XXXXXX)
python3 -B examples/ios_panel.py --device "$device" \
  --binary target/aarch64-apple-ios-sim/debug/panel_scroll --output "$sim_run/run"
```

The generated `run.json` lists the build receipt, socket, and log paths for the process. Run `target/debug/octara inspect --connect SOCKET --receipt RECEIPT --json` to verify the build identity and inspect the window. Drag the list with the simulator’s touch controls. The height and `scroll_offset` of `pan-list` show the panel expanding and the list scrolling. To test the back gesture, build `album_back` and use `--scene album`. Use `--scene interaction`, `--scene review`, or `--scene ime` with the corresponding `window`, `review_queue`, or `ime` binary to test those windows.

When finished, run `xcrun simctl terminate DEVICE BUNDLE` and `xcrun simctl uninstall DEVICE BUNDLE`, then remove the run directory. Find the device UDID and bundle identifier in `run.json`.

## Browser input

The browser example exercises composition, UTF-8 selections, key interception, focus, and viewport sizing. Generate the browser bindings with a `wasm-bindgen` CLI that matches the Rust dependency version.

```sh
cargo build --locked --target wasm32-unknown-unknown -p octara-host-web --example web_input
web_run=$(mktemp -d /tmp/octara-host-web.XXXXXX)
wasm-bindgen --target web --out-dir "$web_run" target/wasm32-unknown-unknown/debug/examples/web_input.wasm
cp crates/octara-host-web/examples/web_input.html "$web_run/"
python3 -m http.server --bind 127.0.0.1 --directory "$web_run" 0
```

Open `web_input.html` on the local port printed by the server and read the assertions on the page. To verify the browser GPU canvas, build with the Cargo flag `--features webgpu`, regenerate the bindings, and open `web_input.html?webgpu`. Stop the server and remove the temporary directory when finished. [Web input example](../crates/octara-host-web/examples/web_input.rs) · [DOM transaction example](../crates/octara-host-web/examples/dom_transactions.rs)

## Local signed release example

`signed_review` is a native macOS example. Set `public_key` to the 64-digit hexadecimal release public key before building.

```sh
OCTARA_REVIEW_PUBLIC_KEY="$public_key" MACOSX_DEPLOYMENT_TARGET=15.4 \
  cargo build -p octara-examples-desktop --bin signed_review
```

Run `signed_review STORE SIGNED_PACKAGE` to stage the package. The app confirms it after the first frame on the next launch. The editing draft is stored in `STORE/draft.json`. Use `package_signed_review.py` to assemble and verify the local app bundle. [Signed release workflow](../docs/design/v2/06-inspect-dst-release-migration.md)
