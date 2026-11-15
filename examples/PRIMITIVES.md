# Composed UI primitives

This round adds Dialog, Tabs/Segmented and editor-owned suggestions, followed by Slider,
NumberField, Progress and command menus. The examples use ordinary Signals and Rust functions.
No form engine, panel router or separate focus manager is required.

## Scenes

```sh
cargo run -p octara-examples --bin window -- showcase interaction
cargo run -p octara-examples --bin window -- showcase controls
```

`interaction` opens an editing dialog with retained tab panels. Type `/` to see suggestions.
Up/Down navigate enabled suggestions and Enter chooses without submitting. Cmd/Ctrl+Shift+U
renames and reorders suggestions. `controls` combines a numeric draft, a slider and progress
with a command menu and nested submenu. Editing a draft and committing it are separate actions.

## Contracts

| API | Ownership and behavior |
| --- | --- |
| `dialog(id, title, open, options, content)` | `Model<bool>` controls visibility. Title, description and close control are accessible. The panel fits the viewport and scrolls its body. The existing modal stack traps/restores focus and handles Escape/outside dismissal. |
| `tabs(id, label, value, options)` | Stable `Option<String>` selection and `TabOption` data. Arrows/Home/End move focus; Enter/Space/press select. Disabled tabs are skipped; missing selected IDs leave the model unchanged. |
| `segmented(id, label, value, options)` | Compact `RadioOption` choices. Radio semantics use `checked`; arrows both focus and select. |
| `popup_list(id, label, open, editor, items, choose)` | The application supplies an existing editor and already filtered `PopupItem`s. Stable IDs survive rename/reorder. Focus and selection remain in the editor; `choose` receives the option ID and owns insertion. |
| `slider(id, value, min, max, step)` | Controlled `f64`; track presses, drag, arrows, Home/End and Page keys clamp/snap consistently. The numeric setter is shared by accessibility and Inspector. |
| `number_field(raw, min, max, step)` | Controlled `String`. Empty strings, signs and incomplete exponents remain editable. Enter/blur normalize complete finite numbers; incomplete drafts remain unchanged. |
| `progress(value, max)` | `Option<f64>` distinguishes known progress from unknown. Unknown publishes no numeric value and reuses the spinner; the control is read-only. |
| `menu_button(id, label, open, menu)` | Snapshots `Menu` once per opening. Actions use command IDs; submenu IDs are scoped to the opening. Keyboard navigation, typeahead and submenu behavior share the runtime's drawn context-menu surface. |

Closing a dialog disposes its content, local state, tasks and editor history. Application-owned
drafts can survive reopening, but reopened controls have new identity targets. A tab bar owns no
panels: `show` disposes a hidden panel, while a display binding retains its editor/undo/scroll.
Applications decide when inactive panels' resources should run. Use stable IDs and `TabPanel`
roles with accessible labels. There is no implicit tab-to-panel `controls` relation in this API.
For retained hidden panels, also gate any owned modal popup's `open` condition on the active
panel: overlay content lives in the separate window layer and hiding its anchor alone does
not dispose it. Editor-owned suggestions close when their editor loses focus.

PopupList owns the supplied editor's key/blur handlers, expanded state and active-descendant
relation. It does not rewrite text or fetch search results. Escape and Tab/Shift+Tab close the
list; other controls remain usable. A suggestion popup anchored inside a dialog belongs to that
dialog's modal interaction scope, while physical containment remains unchanged. Candidate
navigation keys and suggestion pointer choices yield to an active IME. Modified editing keys
and Home/End remain the editor's. The list is finite and retained, without virtualization.

NumberField owns its default submit/blur handlers. Chaining another handler replaces that
behavior; `parse_number` and `normalize_number` allow explicit validation and commit policies.
The Controls example uses Field for errors and refuses out-of-range submission. Its slider
updates the committed value immediately, while text editing keeps a draft until commit.
Static bounds and steps must be finite, with `min < max` and `step > 0`; invalid configuration
panics with an explanatory message. External slider values are normalized for display without
writing back during rendering. Numeric setters refuse hidden, disabled, inert or composing
targets. Numeric semantics are additive fields in `octara.inspect/1`.

Menu item labels, checked state and availability update on the next opening. Action IDs must
be unique within the snapshot; same-label submenus remain separate by snapshot position. A pick
closes the menu and queues its command through `invoke_after_update`, which routes after view
updates and focus restoration. This lets Copy and contextual commands reach the original editor.
The queue is window-owned and bounded; `COMMAND_UNHANDLED` and `COMMAND_QUEUE_LIMIT` are registered
diagnostics. Shortcut captions do not register shortcuts: use `bind_keys` for that.

## Agent and real-window entry points

```sh
CARGO_INCREMENTAL=0 CARGO_PROFILE_RELEASE_DEBUG=0 \
  cargo build --release -p octara-examples --bin window -p octara-cli --bin octara
python3 -B examples/live_primitives.py --scene all \
  --window target/release/window --octara target/release/octara \
  --display --output /tmp/octara-primitives-new
```

Use a new or empty output directory. `--scene interaction` or `--scene controls` runs one scene.
The runner observes fresh node/window targets for each action, never retries delivered input,
and bounds each window to 160 requests. Captures and transcripts are bounded too. It closes only
its own processes and sockets and retains the evidence.

Inspector `set_value` invokes an editable control's numeric setter:

```json
{"op":"set_value","id":"controls-slider","value":0.76,"target":"<observed node target>"}
```

The Slider reports `0.75` for the scene's `0.05` step. `numeric_value`, `min_numeric_value`,
`max_numeric_value` and `numeric_value_step` describe the actual control. Unknown progress
omits `numeric_value`; no numeric setter is advertised for it. Numeric values and form validity
are also visible in plain-text `octara inspect` output.

`drag` takes `id`, finite `dx`/`dy` and the observed node target. Its fixed pointer path starts
at the control's center. The live host waits for real time between four movement phases while
the native event loop continues; it does not advance virtual app time. Each phase checks the
original target and owned pointer. A stale/interrupted gesture cancels only its own pointer
and reports whether earlier input was delivered. A path shorter than the gesture recognizer's
drag threshold can resolve as a press; choose a meaningful travel distance for drag verification.

Live key/text/tap/drag operations are framework input; focus, commands and numeric setters are
semantic operations. Native client-area resize and WindowServer capture are separate routes.
Neither route constitutes OS input injection or native IME validation. The runner reports
`os_input_verified: false` and `native_ime_verified: false`; composition regressions are headless.
An unknown-progress spinner intentionally remains pending until the caller supplies a value.

## References

- Rune: [Dialog](https://github.com/anomalyco/rune/blob/80022c59736b9ef48c2b6954ba29bafbc78ae428/crates/rune-kit/src/dialog.rs),
  [PopupList](https://github.com/anomalyco/rune/blob/80022c59736b9ef48c2b6954ba29bafbc78ae428/crates/rune-kit/src/popup_list.rs),
  [tab strip](https://github.com/anomalyco/rune/blob/80022c59736b9ef48c2b6954ba29bafbc78ae428/crates/rune-kit/src/shell/tab_strip.rs),
  [Segmented](https://github.com/anomalyco/rune/blob/38e3535b797377c38a34654ec0e98694e7e46696/crates/rune-kit/src/segmented.rs),
  [numeric accessibility](https://github.com/anomalyco/rune/blob/80022c59736b9ef48c2b6954ba29bafbc78ae428/crates/rune/src/el/a11y.rs).
- Exact: [edit-sheet draft ownership](https://github.com/expo/exact/blob/32f79aa86ce5ea5a528f69ba52636df72320be79/docs/recipes/edit-sheet.md),
  [raw number editing](https://github.com/expo/exact/blob/814d5e8821c1a1aace2f027770fc4f95c9c3fe0b/docs/recipes/number-field.md),
  [tab keyboard regression](https://github.com/expo/exact/blob/814d5e8821c1a1aace2f027770fc4f95c9c3fe0b/issues/closed/20261110-tablist-arrows-do-not-rove-on-painted-hosts.md),
  [painted numeric input](https://github.com/expo/exact/blob/32a19f18abc117d58f10fbbfdae8855a2bd65b5e/host/painted/src/presenter/range.rs).

Octara retains Rust functions, Signals and its existing kernel. These references informed
behavior and failure cases, rather than a new runtime architecture.

## Recorded macOS verification, 2026-11-15

Artifacts are under `/private/tmp/octara-primitives-20261115/`. Automated verification passed:

- `cargo test --workspace`: 594 passed, zero failed, three ignored. The final headless Drag
  regression rerun also passed both cases after boxing its internal error return for Clippy.
- `cargo clippy --workspace --all-targets -- -D warnings`, `cargo fmt --all --check` and
  `git diff --check`: clean. The generated Octara skill matches its source.
- `octara check`: no findings for Interaction, Controls or Chat, each across
  light/dark × phone/desktop. The two Showcase adapters use their scene's `page` entry point;
  they do not change the default Showcase. Their temporary build products were removed after
  the checks; adapter sources, reports and the existing workspace cache remain.
- Plain-text `octara inspect` exposes numeric values, bounds, steps and actions for the
  NumberField, Slider and read-only Progress. Reports are `check-interaction.json`,
  `check-controls.json`, `check-chat.json` and `inspect-controls.txt`.

The passing live Inspector run is `live-02/summary.json`. Interaction used 109 requests and
Controls used 126, including observations; these counts are not latency or performance measures.

- Interaction checks named dialogs, modal isolation, restored focus, stale targets after
  reopening, manual tab activation, retained editor selection, stable suggestion rows through
  reorder/rename, keyboard and pointer choices without submitting, and segmented radio behavior.
- Controls checks raw/incomplete numeric drafts, explicit normalization, numeric setter snapping,
  slider keyboard bounds, track presses and timed pointer drag, nested menu navigation,
  disabled-item skipping, command dispatch after restored focus, refused disabled setters and
  unknown progress without a fabricated zero value.
- Both scenes pass native 390×640 resize synchronization and final clean diagnostics.
  `interaction-dialog-gpu.png`, `interaction-narrow-gpu.png` and `controls-narrow-gpu.png`
  were visually reviewed. The captures are GPU-rendered pixels from the running windows.
- Both `*-narrow-display.png` requests **fell back to GPU capture because the screen was locked**.
  Their capture reports preserve the reason. They are not evidence of visible desktop pixels;
  WindowServer capture remains pending an unlocked session. OS input and system IME remain
  unverified in this round, independently of the passing framework-input checks.

The first live attempt (`live-01/`) stopped because the runner incorrectly expected a disabled
numeric setter to report delivery. The runtime correctly refused it; correcting the expectation
produced the passing run. Other regressions found and fixed include an overlay wrapper with zero
width, insufficient selected-text contrast, hidden retained editors receiving input, queued
commands surviving window closure, handled IME Space being synthesized again, and drag
cancellation looking up an allocation replaced during the gesture. Each has automated coverage.
