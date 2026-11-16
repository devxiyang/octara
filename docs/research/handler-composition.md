# Component behavior and application callbacks

Source review on 2026-11-16, using the local clones under `/Users/devxiyang/code/archive/reference`.
Rune is pinned to `3686e94`; Exact to `08c889a9`. These are source observations, not claims about
newer revisions or a qualification of either framework's platforms.

Rune separates editing from notification. Its [`on_input_change`](https://github.com/anomalyco/rune/blob/3686e94/crates/rune/src/el/input.rs#L188)
callback observes an already updated model; the runtime compares text before and after the edit
before [calling it](https://github.com/anomalyco/rune/blob/3686e94/crates/rune/src/runtime/input/keys.rs#L184).
Its [`on_key`](https://github.com/anomalyco/rune/blob/3686e94/crates/rune/src/el/focus.rs#L85)
uses a boolean to consume a key, with inner handlers before outer ones. The builder still
replaces its callback slot; this does not demonstrate implicit chaining of repeated setters.

Exact explicitly distinguishes [stopping propagation from preventing a default action](https://github.com/expo/exact/blob/08c889a9/docs/contract-grammar.md#events).
Stopping a key's propagation need not stop typing. That distinction is a useful design question;
it does not require adopting Exact's complete event model. Octara retains its existing boolean
key consumption: `true` stops later callbacks, ancestors and the default action together.

Rune's [Checkbox](https://github.com/anomalyco/rune/blob/3686e94/crates/rune-kit/src/checkbox.rs)
accepts a checked value and an explicit application toggle action; it does not own Octara-style
Model bindings. Thus its callback setters are not evidence that replacing a binding callback is
safe. Exact's [number-field recipe](https://github.com/expo/exact/blob/08c889a9/docs/recipes/number-field.md)
separates raw input from Enter/blur normalization and keeps invalid text available to correct.

Octara applies these lessons with one typed component layer: `El::behavior(|b| b.on_input(..))`.
The preliminary three `before_*` hooks have been removed. Component layers and application
callbacks share `Behavior` storage. Each layer has one replaceable slot per event; layers run
in registration order, followed by the application's independently replaceable callback.
This covers ordinary element events uniformly, rather than adding a new before-method and
composition implementation for every event kind.

At mount, callbacks are grouped by event and compiled into existing Handler variants. Their
common chain owns ordering, mounted-scope retirement checks and key consumption. Signature
adapters preserve owned text, borrowed drag/key data and numeric values. No new dispatch phase,
event bus or runtime is added. Commands, modal dismissal and native protocol events retain
separate owners; this is not a mechanism for intercepting platform protocol callbacks.

TextField, Checkbox, Switch, Slider and NumberField place their binding/editing behavior in
component layers; PopupList preserves editor callbacks while owning suggestion navigation and
blur dismissal. Numeric and text arguments are proposed values; a Model setter can transform
or reject them. Application callbacks can inspect the resulting model, but invocation does not
promise a changed or valid value. Keyboard stepping does not manufacture a numeric-input event.
NumberField normalizes only complete finite drafts on Enter or blur and then notifies even if
the draft was invalid. Application validation remains ordinary Rust.

All callbacks share the handler's mounted scope/window. Scope liveness is checked between
callbacks; consumed keys and scope retirement stop the chain. Retirement also prevents key
fallback on a replacement tree. IME routing remains first. This is a deliberate Octara API
choice informed by the references, not an assertion that Rune or Exact exposes this API.
