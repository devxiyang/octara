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

Octara applies these lessons through three explicit component hooks: `before_input`, `before_key`
and `before_blur`. Hooks append in registration order, before the single replaceable `on_*`
application callback. This keeps a bound field's model write distinct from application input
notification, and allows a suggestion list to own navigation and dismissal while preserving
the editor's application callbacks. Input callbacks receive the same proposed text; unlike
Rune's change notification, Octara does not promise that the model changed or suppress equal
input requests. A custom Model setter remains responsible for normalization or rejection.

Hooks compile into the existing mounted handler at element construction. Storage is allocated
only for elements using the hooks. All callbacks share the handler's mounted scope/window, and
scope liveness is checked between them. A consumed key or scope retirement stops the chain;
retirement also prevents key fallback on a replaced tree. IME routing precedes the chain.

The initial change is deliberately limited to text_field and popup_list. Other controls'
documented replacement contracts remain as they are; future changes require their own use cases
and regression tests. No new event bus, capture phase, propagation flags or runtime is added.
