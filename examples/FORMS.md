# Fields and radio groups

The `showcase forms` scene is a small settings form built from ordinary Signals,
Rust validation functions and public kit components. Its display name accepts
Chinese and emoji; an empty name or the reserved name `admin` cannot be saved.
Its reply style is a controlled radio group with stable IDs and a disabled option.
The form never rewrites text while the person edits it: saving trims only the
saved value, leaving the editor's original text intact.

## Public contract

- `field(id, label, control, info)` returns a wrapper around the actual focusable
  control. Configure the control's model, handlers and `disabled` state before
  passing it in. Chaining `disabled` on the returned wrapper affects only that
  wrapper; disabled state does not cascade to children.
- `info` is a tracked closure returning plain `FieldInfo` data: `description`,
  optional `error`, and `required`. The field applies the label, combined help/error
  description, required and invalid semantics to the control itself. A blank error
  is treated as absent. The error is also a visible, polite live region.
- `id` identifies the control; `{id}-field`, `{id}-label`, `{id}-help` and
  `{id}-error` identify its wrapper and supporting text. Label presses focus the
  control. Dynamic validation and supporting text retain the editor allocation,
  selection, undo and composition.
- `required` and `invalid` describe the control; they do not validate or block
  input. The submit handler must check validity too. Inspector and AccessKit use
  the same resolved description; explicit description takes precedence over a
  tooltip, and a tooltip repeating its label is omitted.
- `radio_group(id, label, model, options)` takes `Model<Option<String>>` and a
  tracked `Vec<RadioOption>`. Options carry stable IDs, labels, descriptions and
  disabled state. Renaming and reordering retain their node identities. Removing
  the selected option leaves the model unchanged with no checked row.
- Radio selection is `checked`, not `selected`. Tab enters a group once, at its
  checked enabled option, remembered enabled member, or first enabled member.
  Focus alone does not choose. Arrows wrap among enabled options; Home/End choose
  the ends. A navigation key also chooses an unchecked current destination, such
  as the only enabled radio. Space, Enter and presses choose normally.
- Navigation rechecks the original radio allocation, focus, visibility, disabled
  state, group and modal eligibility after focus callbacks run. Removing the radio
  or opening another modal from a focus callback cannot press the obsolete target.

For the exact source, see [forms.rs](src/showcase/forms.rs). The design and the
Rune/Exact references are in [UI primitives](../docs/design/ui-primitives.md#fields-and-radio-groups).

## Verification

Focused tests cover Field semantics and retained editing state, radio identity and
keyboard behavior, nested modals, and the actual Showcase scene:

```sh
CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0 \
  cargo test -p octara-kit -p octara-examples --test integration forms
```

The core tests additionally cover dynamic AccessKit updates, old Inspector JSON,
semantic-only invalidation and reentrant radio focus handlers. The Showcase tests
exercise Chinese preedit while descriptions/options update, Enter during
composition, and 320/390/1000-pixel layouts in both appearances. These are headless
composition checks; they do not drive a system input method.

Build and run the actual desktop windows against a new or empty output directory:

```sh
CARGO_INCREMENTAL=0 CARGO_PROFILE_RELEASE_DEBUG=0 \
  cargo build --release -p octara-examples --bin window \
  -p octara-cli --bin octara -p octara-chat --bin octara-chat
python3 examples/live_forms.py \
  --window target/release/window --octara target/release/octara \
  --chat target/release/octara-chat --display --output /tmp/octara-forms-new
```

The runner uses fresh guarded Inspector targets and checks the actual delivery
routes. It closes only its own windows and sockets. The optional Chat run clears
inherited model configuration and uses an isolated store with Scripted; it verifies
that `model-picker.description` exposes the current model and the effect of
switching it. No network model or user conversation store is used.

## Recorded macOS run, 2026-11-15

Automated verification passed: `cargo test --workspace` reports 528 passed,
zero failed and two ignored; `cargo clippy --workspace --all-targets -- -D warnings`
and `cargo fmt --all -- --check` are clean. The exported skill copy is current.
`octara check` reports no findings for both the Forms scene and Chat across
light/dark × phone/desktop. Forms uses a temporary adapter whose entry point is
`octara_examples::showcase::page("forms").unwrap()`, leaving the Showcase default
unchanged. Plain-text `octara inspect` also exposes the input's description,
`required=true` and `invalid=false`. Logs and CLI output are beside the live evidence.

Passing evidence is in `/private/tmp/octara-forms-20261115/live-02/`:

- `summary.json`: all nine form checks pass; 99 bounded Inspector requests, including
  observations, plus three for offline Chat. This count is not an input latency
  or performance measurement.
- `forms-retained-editor.json`: updating descriptions and reordering/renaming
  choices preserves the editor's selection and all recorded control targets.
- `forms-keyboard-radios.json`: one Tab stop, checked-value changes on arrows,
  wrapping past the disabled choice, and Home/End selection.
- `forms-blank.json`, `forms-reserved-name.json`: required/invalid semantics,
  disabled Save reasons and refused submission with Enter.
- `forms-saved-chinese.json`: committed Chinese and emoji save successfully;
  the editor keeps surrounding spaces while the saved value is trimmed.
- `forms-initial-gpu.png`, `forms-invalid-gpu.png`: ordinary and invalid states
  rendered by the real window's GPU at 800×600.
- `forms-narrow-gpu.png`, `forms-narrow-display.png`: a matched and synchronized
  native resize to 390×640. The latter reports `renderer: display` and
  `pixels: display`, with no GPU fallback. The final window check is clean.
- `chat-offline-model-context.json`: the normal Chat window reports
  `Scripted` and `Applies to the next message or regenerated answer.` in the
  model wrapper's description. Its GPU capture is stored beside the snapshot.

The desktop computer-control tool was unavailable. This run uses framework input
for keys/text and semantic operations for focus/commands; the Chinese text is
already committed text. `os_input_verified` and `native_ime_verified` are explicitly
false. Native resize and WindowServer capture are verified separately. System IME
composition/candidate-window validation from earlier work is not counted again.

The first run reached the display capture but used an invalid explicit scale; the
capture API correctly refused it. The runner now omits scale for display capture,
which uses the actual display scale. The failed run remains in `live-01/` and is
not counted as a passing run. Headless checking also found a small clickable label
target; Field labels now have a 24px pointer minimum and 44px touch minimum.
