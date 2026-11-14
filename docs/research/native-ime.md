# Native IME ownership: Rune, Exact and Octara

Status: source review, local reference observations and a real-window run, 2026-11-14. Scope: macOS input
interpretation, native text queries and replacement, focus, composition endings, and what the
agent can actually verify. iOS mechanisms are identified separately. Octara's Pinyin candidate
contents, placement and committed-input-code reconversion passed the focused macOS scenarios
recorded in [the validation guide](../../examples/IME.md); other input methods remain separate work.

## Revisions and evidence

| Project | Revision inspected | Date and source |
| --- | --- | --- |
| Rune | `3faa09fddaa07be68ea424c806624c7cbe194231` | 2026-11-14 08:36:04 UTC; [fixed primary revision](https://github.com/anomalyco/rune/commit/3faa09fddaa07be68ea424c806624c7cbe194231) |
| Exact | `08c889a9de21a30b7c3b308422fc588cf1a480d3` | 2026-11-11 10:48:44 UTC; [fixed primary revision](https://github.com/expo/exact/commit/08c889a9de21a30b7c3b308422fc588cf1a480d3) |
| AppKit | The installed macOS SDK and stock `NSTextView`, 2026-11-14 | Local reference probe `/private/tmp/octara-native-text-reference.swift`; [Apple text-client API](https://developer.apple.com/documentation/appkit/nstextinputclient) |

The local Rune clone was older (`3686e94a`, 2026-11-10); the Rune findings below were checked
against the fixed remote source, not inferred from that clone. The Exact clone's relevant files
matched its fixed revision. Earlier research citing Exact `bb85428a` or describing Rune's
planned native protocols is not evidence of the implementations at these revisions.

## Who owns the native editing document?

| Concern | Rune | Exact on macOS |
| --- | --- | --- |
| Visible text/editor | Rust editor and framework rendering | Actual `NSTextField` / `NSTextView` subclasses |
| Native protocol client | Vendored winit `WinitView`, extended with published context and clause/replacement hooks | AppKit's native editor, with hooks around `super` |
| Input-method interpretation | Inside native `keyDown`, before queuing the resulting framework keyboard event | Normal AppKit event delivery after an application-local routing monitor |
| Application update | Native results enter the runtime action queue | Native editor changes dispatch to the runtime and apply returned batches |
| Query authority | A published surrounding-text snapshot plus winit's marked storage | Native text storage, selection and TextKit layout |

Rune calls `interpretKeyEvents` inside `WinitView.keyDown`; `setMarkedText`, `insertText` and
`doCommandBySelector` can run there. Only afterward does it decide whether to queue a normal
`KeyboardInput`. `Runtime::ime` subsequently queues `Action::Ime`. That later application queue
does **not** mean IMKit interpretation is deferred out of `keyDown`.
[Native key entry](https://github.com/anomalyco/rune/blob/3faa09fddaa07be68ea424c806624c7cbe194231/vendor/winit/src/platform_impl/macos/view.rs#L475),
[runtime queue](https://github.com/anomalyco/rune/blob/3faa09fddaa07be68ea424c806624c7cbe194231/crates/rune/src/runtime/input/keys.rs#L288).

Exact's single-line input uses a real `NSTextField`; its cell supplies a session-owned
`FieldEditor: NSTextView`. Sharing that editor preserves AppKit editing settings across fields.
A textarea is a real TextKit 2 `NSTextView` inside an `NSScrollView`. Even an author-styled,
borderless field retains a native editor; this is not a hidden editor behind GPU-drawn text.
The `setMarkedText` and insertion overrides call `super`, and the native text storage is updated
before Exact reports the resulting string. Native selection, surrounding-text and geometry
queries therefore use AppKit's editor rather than requesting answers from JavaScript.
[Field creation](https://github.com/expo/exact/blob/08c889a9de21a30b7c3b308422fc588cf1a480d3/host/apple/Sources/ExactKit/Mac/NodeViewMac.swift#L905),
[field-editor ownership and hooks](https://github.com/expo/exact/blob/08c889a9de21a30b7c3b308422fc588cf1a480d3/host/apple/Sources/ExactKit/Mac/TextAreaMac.swift#L690),
[textarea creation](https://github.com/expo/exact/blob/08c889a9de21a30b7c3b308422fc588cf1a480d3/host/apple/Sources/ExactKit/Mac/TextAreaMac.swift#L423).

Exact's local event monitor routes shortcuts and application handlers, then returns an
unconsumed event for normal AppKit delivery. While text is marked, ordinary composition keys
stay with the input method. Command chords first finish composition; an outside pointer press
also settles it before application actions run. This preserves the current native event cycle.
[Monitor](https://github.com/expo/exact/blob/08c889a9de21a30b7c3b308422fc588cf1a480d3/host/apple/Sources/ExactKit/Mac/ExactViewMac.swift#L334),
[composition routing](https://github.com/expo/exact/blob/08c889a9de21a30b7c3b308422fc588cf1a480d3/host/apple/Sources/ExactKit/KeyEvents.swift#L270).

Exact's runtime boundary is also concrete: `onInput` sends the current value and selection to
`runtime.input` / `fieldEvent`, whose bridge calls Rust `exact_dispatch` and returns a batch.
`Owner.sync` waits for that owner job; it can serve main-thread requests while waiting and
returns `busy` for a recursive owner call from such a callback. Exact therefore has explicit
reentrancy rules, not an assumption that all native callbacks are asynchronous.
[Input delivery](https://github.com/expo/exact/blob/08c889a9de21a30b7c3b308422fc588cf1a480d3/host/apple/Sources/ExactKit/Session.swift#L745),
[bridge](https://github.com/expo/exact/blob/08c889a9de21a30b7c3b308422fc588cf1a480d3/host/apple/Sources/ExactKit/Bridge.swift#L41),
[owner synchronization](https://github.com/expo/exact/blob/08c889a9de21a30b7c3b308422fc588cf1a480d3/host/apple/Sources/ExactCore/Owner.swift#L95).

## Rune's implemented boundary

Rune publishes a selection and bounded surrounding paragraph text, at most 1024 bytes on each
side of the selection. It withholds this context for secure fields and while composing. Its
macOS bridge publishes that snapshot to vendored winit, converts replacement ranges back to
field offsets, and receives clause attributes. This is useful implemented work beyond upstream
winit's preedit/commit interface.
[Context construction](https://github.com/anomalyco/rune/blob/3faa09fddaa07be68ea424c806624c7cbe194231/crates/rune/src/edit.rs#L429),
[macOS context bridge](https://github.com/anomalyco/rune/blob/3faa09fddaa07be68ea424c806624c7cbe194231/crates/rune/src/platform/ime.rs#L15).

It remains a partial native text client:

- `selectedRange` answers the published selection only without native marked text; otherwise
  it returns `NSNotFound`. `markedRange` starts at zero in the marked string.
- `attributedSubstringForProposedRange` reads the published context, not a complete displayed
  document containing the current marked range.
- `setMarkedText` captures an explicit replacement, but `insertText:replacementRange:` still
  ignores its replacement argument. Runtime `Ime::Select` is ignored mid-composition.
- `characterIndexForPoint` returns zero. `firstRectForCharacterRange` returns the supplied IME
  rectangle while ignoring both the requested range and `actualRange`.

[Selection and marked queries](https://github.com/anomalyco/rune/blob/3faa09fddaa07be68ea424c806624c7cbe194231/vendor/winit/src/platform_impl/macos/view.rs#L250),
[replacement capture](https://github.com/anomalyco/rune/blob/3faa09fddaa07be68ea424c806624c7cbe194231/vendor/winit/src/platform_impl/macos/view.rs#L288),
[substring, geometry and insertion](https://github.com/anomalyco/rune/blob/3faa09fddaa07be68ea424c806624c7cbe194231/vendor/winit/src/platform_impl/macos/view.rs#L382),
[runtime selection handling](https://github.com/anomalyco/rune/blob/3faa09fddaa07be68ea424c806624c7cbe194231/crates/rune/src/edit/ime.rs#L178).

This narrower implementation is not evidence that the omitted protocol behavior is unnecessary,
or that arbitrary native reconversion and partial marked-range replacement are complete.
Publishing a caret rectangle can support ordinary candidate anchoring without implementing
range-sensitive geometry for every native query.

## Echo, acceptance and cancellation are separate policies

Exact intentionally reports every changed editor string as `input`, **including preedit**.
Caret-only changes and accepting the same marked text are deduplicated; cancellation reports
the string after marked text disappears. Its `change` means the editing session's commit, such
as blur or single-line Return, rather than the end of each IME composition. This differs from
Octara's committed `Model<String>` contract.
[Exact composition contract](https://github.com/expo/exact/blob/08c889a9de21a30b7c3b308422fc588cf1a480d3/host/apple/Sources/ExactKit/FieldComposition.swift#L6).

Exact protects a controlled editor by making an identical echo a no-op. A different application
value arriving while text is marked becomes `pendingValue`; the latest applicable value lands
after composition ends. Outside composition it applies a minimal UTF-16 replacement and carries
the selection through it. This avoids an uppercase/filter/format handler tearing down the first
Pinyin or Japanese syllable. Styling and base-direction changes are also deferred while marked.
[Controlled writes](https://github.com/expo/exact/blob/08c889a9de21a30b7c3b308422fc588cf1a480d3/host/apple/Sources/ExactKit/Mac/TextAreaMac.swift#L449),
[styling guard](https://github.com/expo/exact/blob/08c889a9de21a30b7c3b308422fc588cf1a480d3/host/apple/Sources/ExactKit/Mac/NativeFieldsMac.swift#L166).

Exact's forced acceptance asks the input context to discard its composer first, then calls
native `unmarkText` if marking remains. It suppresses the intermediate remove/reinsert steps
and reports the result once. Its comment identifies the failure from reversing that order:
the input method can insert a syllable twice.
[Acceptance and notification suppression](https://github.com/expo/exact/blob/08c889a9de21a30b7c3b308422fc588cf1a480d3/host/apple/Sources/ExactKit/Mac/FieldEditingMac.swift#L106).

Rune has a separate `Finish` action that accepts the shown preedit on blur, caret-moving click
or window deactivation, and `Cancel` that drops it after an edit, undo or transition to secure
input. After the runtime accepts a finished composition, platform synchronization discards
IMKit's own copy so it cannot enter another field. Its native command path similarly discards
first, then calls winit `unmarkText` to return to ground state. That winit method itself emits
empty preedit; it is not equivalent to stock `NSTextView.unmarkText` accepting its buffer.
[Finish versus Cancel](https://github.com/anomalyco/rune/blob/3faa09fddaa07be68ea424c806624c7cbe194231/crates/rune/src/edit/ime.rs#L103),
[platform retirement](https://github.com/anomalyco/rune/blob/3faa09fddaa07be68ea424c806624c7cbe194231/crates/rune/src/window/platform_sync.rs#L13),
[command retirement](https://github.com/anomalyco/rune/blob/3faa09fddaa07be68ea424c806624c7cbe194231/crates/rune/src/native_menu/standard_macos.rs#L120),
[winit unmark behavior](https://github.com/anomalyco/rune/blob/3faa09fddaa07be68ea424c806624c7cbe194231/vendor/winit/src/platform_impl/macos/view.rs#L358).

Octara's draft rollback on cancellation, including its owner-retirement policy, is an explicit
framework choice. It must not be justified by equating all of `unmark`, cancel, native context
discard, and blur. Exact's preedit-as-`input` behavior and Rune's finish-on-blur behavior should
not silently change Octara's committed-value contract.

## iOS is a separate implementation

Exact uses actual `UITextField` and `UITextView` subclasses. Its marked-text and insertion hooks
call UIKit `super`; controlled writes defer while `markedTextRange` exists. It also guards focus
so an authored keyboard responder cannot steal a native field's, controller's or menu's input.
Those are useful mobile ownership rules, not implementations of macOS `NSTextInputClient`.
[UIKit fields](https://github.com/expo/exact/blob/08c889a9de21a30b7c3b308422fc588cf1a480d3/host/apple/Sources/ExactKit/IOS/NativeFieldsIOS.swift#L17),
[marked editing](https://github.com/expo/exact/blob/08c889a9de21a30b7c3b308422fc588cf1a480d3/host/apple/Sources/ExactKit/IOS/TextAreaIOS.swift#L222),
[controlled value](https://github.com/expo/exact/blob/08c889a9de21a30b7c3b308422fc588cf1a480d3/host/apple/Sources/ExactKit/IOS/FieldEditingIOS.swift#L36),
[focus ownership](https://github.com/expo/exact/blob/08c889a9de21a30b7c3b308422fc588cf1a480d3/host/apple/Sources/ExactKit/IOS/KeyboardFocusIOS.swift#L4).

## Replacement coordinates: document a measured contract

Apple's current `NSTextInputClient.setMarkedText` page describes `replacementRange` as computed
from the beginning of marked text. Its `selectedRange` parameter is relative to the newly
inserted string. The installed `NSTextInputClient.h` explicitly documents that selection
parameter but does not settle the conflicting replacement wording. Separately, InputMethodKit
documents its replacement range as addressing the client document.
[Text-client method](https://developer.apple.com/documentation/appkit/nstextinputclient/setmarkedtext(_:selectedrange:replacementrange:)),
[InputMethodKit range](https://developer.apple.com/documentation/inputmethodkit/imkinputcontroller/replacementrange()).

A local stock-`NSTextView` reference probe on 2026-11-14 resolved the behavior used by this
Octara macOS implementation: **an explicit replacement addresses the displayed document in
UTF-16 units, even when another marked range exists**. The probe marks `abcd` at offset 10 in
`0123456789abcdZ`, then compares explicit `(location: 1, length: 1)` with `(11, 1)`. The former
targets document offset 1, rather than adding the old mark's offset 10. It creates no real
window and supplies no OS key events; this establishes a protocol-method reference, not an
input-method or candidate-window acceptance test. The source was kept at
`/private/tmp/octara-native-text-reference.swift` for reproduction.

Keep displayed-document-global UTF-8 coordinates inside Octara and convert UTF-16 at the native boundary.
Preserve the distinction between the whole displayed draft, its current marked subrange, and
the relative selection inside newly inserted marked text. Never guess between relative and
global ranges by inspecting whether numbers happen to fit: the valid numeric ranges overlap.
If another platform or OS version differs, isolate the adaptation and add an explicit reference
case; do not introduce a coordinate heuristic.

## Historical failures and verification limits

Exact recorded a useful system-control comparison: on macOS 27.2, Korean 2-Set delivered
successive `insertText` replacements without ever calling `setMarkedText`. Both Exact and a
stock `NSTextField` had `hasMarkedText == false`; Chrome differed. Exact accepted AppKit's
behavior. This is a reported real input-source experiment in the upstream issue, not an
experiment rerun for this document. It shows why marked-state inspection alone is insufficient
to identify every input method's composition lifecycle.
[Korean reference comparison](https://github.com/expo/exact/blob/08c889a9de21a30b7c3b308422fc588cf1a480d3/issues/closed/20261025-macos-korean-composes-without-marked-text.md#L11).

Exact's composition test suites explicitly state that they call the native protocol methods
directly; their Mac suite uses no real input method, and the iOS field tests even send the
control-change notification themselves. The Mac agent key driver constructs `NSEvent`s and
sends them to `NSWindow`, supplying its own command route because it bypasses the application
monitor. These tests validate valuable editor and framework seams, but cannot establish that
OS-delivered Pinyin candidates or reconversion work in an active foreground window.
[Mac test boundary](https://github.com/expo/exact/blob/08c889a9de21a30b7c3b308422fc588cf1a480d3/host/apple/tests/ExactKitTests/FieldCompositionMacTests.swift#L15),
[iOS test boundary](https://github.com/expo/exact/blob/08c889a9de21a30b7c3b308422fc588cf1a480d3/host/apple/tests/ExactKitTests/FieldCompositionIOSTests.swift#L12),
[agent event delivery](https://github.com/expo/exact/blob/08c889a9de21a30b7c3b308422fc588cf1a480d3/host/apple/Sources/ExactKit/Mac/AgentMac.swift#L1070).

In Octara's initial 2026-11-14 comparison attempt, both the custom client and the stock
`NSTextView` reference received plain Latin text despite verified TIS ABC/ITABC switches. The
custom client's `last_interpret_inline: true` observation establishes that interpretation was
inside native `keyDown` in that attempt; deferral alone cannot explain the reference result.
The window was also reported occluded, and foreground activation required a manual user step
after the available computer-use activation routes were exhausted. Treat this as an unresolved
input-environment/control result, not proof of a core editing bug or of a broken system IME.

After the user brought the validation window to the foreground, real Pinyin composition worked.
The run exposed a separate query-timing bug: IMKit asked for the newest marked character's
rectangle before the next core frame. A stale geometry snapshot returned no rectangle. The
client now reuses the existing text engine with a frozen layout recipe to answer that fresh
document synchronously, without calling application handlers.

The visible candidate panel was a remote system view hosted in the **client process**. Its
SCIM service counterpart remained offscreen, so service-PID-only sampling had missed the real
panel. Read-only, explicit-client-PID captures established the candidate text and position,
including the expanded list and a resized window. The user then opened the system input menu's
“Show Substitution Candidates” for selected committed `nihao` in `A😀nihaoZ`; computer-use key
`2` selected the visibly offered `👋`, producing `A😀👋Z` as one edit. Undo restored both text and
selection. Native surrounding-text queries increased even though this reconversion did not
create a marked range. See evidence `24`–`29` in the validation guide for the exact scope.

Native callback tests, Inspector state, app-only pixel captures, OS window metadata, and an
image that actually includes the separate candidate panel prove different things. Keep them
separate in acceptance evidence. This run validates Simplified Pinyin input-code reconversion;
it does not establish arbitrary Hanzi reconversion, every IME, or every display arrangement.

## Consequences for Octara

1. Retain Rust and Signals; give the native client a synchronous, owned displayed document for
   text and selection queries. Application handlers can remain behind a safe drain boundary.
2. Preserve native event interpretation inside the actual key event cycle. An application
   action queue, such as Rune's, does not require delaying IMKit interpretation.
3. Borrow Exact's identical-echo no-op, guarded styling and explicit retirement ordering. Keep
   Octara's preedit and committed `Model<String>` semantics explicit instead of adopting
   Exact's preedit-as-`input` contract by accident.
4. Implement and test the protocol operations Octara claims. Rune's smaller implementation
   illustrates useful boundaries; its omitted operations do not establish complete reconversion.
5. Compare with a stock editor under the same active input source and foreground conditions.
   Then verify the OS popup itself, including its chosen text and caret-relative position,
   before claiming the native reconversion and candidate-window loop is complete.
