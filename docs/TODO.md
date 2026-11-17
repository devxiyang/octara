# TODO

What is open, by area. For phase goals see [ROADMAP](ROADMAP.md); for what is built see
[ARCHITECTURE](ARCHITECTURE.md) and the [designs](design/). When an item is done, remove it and
make the corresponding design document describe it.

## `init` architecture refactor (2026-11-17)

Execution plan for the [layered redesign](design/layered-architecture-redesign.md) and
[primitive contracts](design/core-primitive-contracts.md). Work on `init`; no compatibility
aliases or permanent parallel implementations. Keep this checklist as the progress record;
update the design documents with implemented contracts and verification evidence.
Commit each completed and verified stage before proceeding to the next stage.

Completed foundation: `octara-primitives` and `octara-text` are independent packages;
production dependency directions are enforced by `tools/check_layers.py`.
Package extraction alone does not complete the responsibility redesign.

### A. App resources and initialization

- [x] **A1 — Initial configuration.** Configure theme, text engine and target
      conventions before creating/mounting windows. Give the shell the same initialization
      path; migrate Chat's deferred theme setup, deterministic text tests and screenshot tools.
      Delete runtime text-engine replacement/system-font switching APIs. Verify the first
      build and first output use the configured resources, including multiple windows.
- [x] **A2 — One resource mutation owner.** App owns legal runtime theme/font/environment
      changes. Check execution/lifetime permissions before mutation; invalidate all affected
      windows through the same owner. Remove public shared Resources and Kernel resource
      setters/manual cross-kernel notification protocols; use explicit private test assembly.
      Verify forbidden build/derive/observe calls, shutdown/failure, multi-window invalidation
      and callback/borrow boundaries without adding public permission primitives.
- [x] **A3 — Window coordination.** Move after-layout/scroll/size feedback coordination out of
      View into Window; View only registers required bindings/work. Preserve bounded feedback,
      cancellation and scope ownership. Delete the old coordination path.

A1 evidence (2026-11-17, Rust 1.100.0): workspace all-targets check and Clippy passed;
full workspace tests: **866 passed, 0 failed, 3 existing ignored doctests**. A dedicated test
checks two windows' initial build, theme/text/touch geometry, exactly one initial presentation
and no deferred task. Existing multiline, system-font, Chat and virtual-geometry tests migrated.
No-default-features core check, strict library rustdoc, layer checks, formatting and CLI vocab
passed; the exported skill is current. No real-window/OS validation was performed in A1.
`AppConfig` / `App::with_config` and shell `Options.app` replace runtime engine switching;
Chat supplies its theme before mounting.

A2 evidence (2026-11-17, Rust 1.100.0): runtime resource mutations now share
`app/resources.rs`; public Resources, Kernel resource setters/constructor and manual invalidation
hooks are removed. The current-App theme helper uses the same gate. Reduced Motion is retained
before any windows exist. Provider registration is detached from storage, untracked and denied
UI write permission; DropGuard restores engine ownership on panic.

Full workspace regression: **876 passed, 0 failed, 3 existing ignored doctests**. New tests cover
build/init/derive/observe rejection including cross-App handles, shutdown/failure, live window
cleanup versus shutdown cleanup, font invalidation of two windows with stable targets, provider
reentry/read isolation, native-query unavailability/recovery, and panic without partial frame
publication. Storage-borrow release and panic restoration also have a focused unit test.
Workspace all-targets check/Clippy, strict library rustdoc, no-default-features core build,
formatting, diff/layer checks and actual CLI vocab lookup passed. Skill export is synchronized.
No real OS/window validation was performed; native-query evidence uses the headless protocol.
A3 completion is recorded below; subsequent content contracts remain open.

A3 evidence (2026-11-17, Rust 1.100.0): `app/feedback.rs` now owns feedback
registration, automatic scope cleanup, geometry sampling/publication and release at window
close. View only requests size/tail signals or registers virtual-list/Canvas measurements;
its WindowState implementation and direct mutable feedback-table access are removed. Paced
text landing and handler registration moved to their Window modules. Scope cleanup replaces
manual callback removal in virtual lists and Canvas.

The closing-hook regression exposed a window-order RefCell borrow spanning callbacks; frame
and consistency traversal now snapshot IDs before invocation. Closing/shutdown stops subsequent
hooks and geometry publication. Registry removal releases captures outside borrows, and sampled
geometry publishes after releasing kernel/registry borrows. Existing 4/16 round limits remain.

Full workspace regression: **879 passed, 0 failed, 3 existing ignored doctests**. Added tests cover
scope retirement/re-registration, close and shutdown from feedback with no later callbacks or
frames, and reentrant callback-capture destruction. The prior hook-cancellation test moved from
task tests to the window feedback module. Existing finite/oscillating feedback, virtual geometry,
Canvas, multi-window, Chat and rendering tests passed. All-targets compilation/Clippy, strict
library docs, no-default-features core build and format/diff/layer checks passed. Public APIs and
the skill are unchanged. No real-window/OS validation was performed in A3.

### B. Document and content composition (in progress)

- [x] **B1 — Builder content ownership.** Separate application container children from atomic
      text/input/image/icon/canvas/native content. Keep ordinary Rust builders and shared styling;
      avoid generic state explosions. Reject incompatible composition before conversion/mount.
- [x] **B2 — Internal text structure.** Give rich text explicit ownership of runs and internal
      links, separate from application children. Preserve identity where continuity is known;
      reject stale replaced targets. Enforce the distinction at kernel structural-write entrances.
- [x] **B3 — Opaque composition.** Hide View's mount representation and converge structural updates on one private path.
      Validate illegal composition, keyed reorder, editor lifetime and link replacement.
      **B3a completed:** Inspector selection extraction now finishes before borrowing the tree
      and sampling targets, so consistency rebuilds cannot conflict with that borrow or mix
      pre/post-rebuild identities in one snapshot. Regression checks Chinese selection in ordinary
      mode, then repeated consistency-mode snapshots and agreement with the current window target.
      That mode deliberately rebuilds identities and clears document selection; target-continuity
      tests exercise ordinary retained execution separately. Opaque View and structural updates
      remain unfinished B3 work. No real-window validation in this slice.
      **B3b implemented:** View is now an opaque description; its enum and the public Mount
      wrapper are removed. Use element conversion, `View::empty()` and `View::fragment(..)`.
      Chat, Markdown, system prompts and content tests are migrated with no compatibility
      aliases. Compile-fail doctests guard private representation and removal of Mount.
      Ordinary mounting and list reconciliation still have distinct structural write call
      sites; unifying those remains B3c. No real-window validation in B3b.
      B3b validation: 890 workspace tests passed, 0 failed, 3 existing ignored doctests;
      all-targets compilation/Clippy, strict core rustdoc, formatting, diff/layer checks and
      CLI lookup of `View::fragment` passed. Existing keyed-list/editor and Chat regressions pass.
      **B3c implemented:** `view/structure.rs` now owns application placement, beginning exit
      and final destruction. Ordinary mounting, conditional branches, incremental/snapshot lists
      and virtual spacers use it; the list-only placement path is deleted. Placement inserts
      detached nodes or moves same-parent nodes, rejects foreign owners and skips already-correct
      positions. Final cleanup tolerates descendants already destroyed by a parent and preserves
      Kernel-owned exiting nodes. Scope disposal, interaction retirement and slot accounting
      stay with their owners, outside Kernel borrows; no transaction or public primitive added.
      B3c validation: 891 workspace tests passed, 0 failed, 3 existing ignored doctests;
      all-targets Clippy, strict core rustdoc, formatting and diff/layer checks passed. The new
      structure test verifies retained input value/identity under reorder, foreign-owner rejection
      before mutation and ancestor-first cleanup. Existing list, editor, exit-animation, Chat,
      HTTP and rendering regressions passed. No real-window validation was performed.

B1 evidence (2026-11-17, Rust 1.100.0): only `Content::Box(Vec<View>)` stores application
children; the shared `El.children` field is removed. Atomic builders reject child conversion
and iterator consumption, including empty iterators. Image replacement rejects occupied boxes
and declared empty views; valid image replacement preserves fit and semantics, with its caption
composed by an external container. No new public types, traits or compatibility paths were added.
Rune's ordinary El methods and Exact's separate content/measurement capabilities were checked
against the recorded local reference sources; their broader child acceptance is not copied.

Full workspace regression: **880 passed, 0 failed, 3 existing ignored doctests**. The content
tests cover every atomic builder and the valid external image/caption composition. Existing
dynamic lists, rich links, editing, overlays, native content, Chat and rendering tests passed.
Workspace all-targets Clippy, strict library rustdoc, no-default-features core build, formatting,
diff/layer checks, CLI vocab/diagnostic lookup and generated skill freshness passed. No real
OS/window validation was performed. Kernel child ownership/validation and opaque View remain
B2/B3 work, not claimed complete by the builder invariant.

B2 evidence (2026-11-17, Rust 1.100.0): Kernel content now owns structure too:
`Content::Box` owns application children and `TextContent.links` owns internal links. The generic
Node child field and positional link reuse are removed. Read traversals project owned descendants;
application structural methods cannot independently create/move/remove/destroy links, insert into
atomic content or create cycles. Content replacement rejects occupied containers (including exiting
children) and incompatible atomic kinds. Text-owned links never enter Taffy's child lists.

Rich replacement validates UTF-8 run ranges before mutation and retains only unchanged mapped
ranges with the same destination. A Chinese/emoji prefix test exposed byte-prefix ambiguity; both
prefix and suffix now stop at character boundaries. Plain replacement/splice clears annotations;
append preserves existing runs. The existing diagnostic and Agent skill describe these contracts.

Full workspace regression: **887 passed, 0 failed, 3 existing ignored doctests**. Six new kernel
tests cover atomic/occupied-content rejection, ownership/lifetime/index cleanup, continuity and
same-range replacement, plain edits, invalid runs with no partial mutation, and cycle rejection.
A headless window test covers retained focus, prefix/style changes, replacement retiring focus and
pending pointer input, stale Agent target refusal, new-target activation and link removal. Existing
rich rendering, accessibility, selection, virtual lists, Chat/HTTP and GPU regression suites passed.
Workspace all-targets Clippy, strict library rustdoc, no-default-features core build, formatting,
diff/layer checks, CLI vocab/diagnostic lookup and exported skill freshness passed. No real OS/window
validation was performed. B3 includes the separately recorded consistency-mode inspection issue.

### C. Editing, geometry and semantic facts

- [ ] Make drawing, hit testing, selection, IME and accessibility consume the same text
      geometry, with alignment/constraints accounted for once.
      **C1a implemented:** document text now has one named private TextBox result for content
      origin/width, wrapping and truncation constraints. Drawing no longer recomputes these
      independently; visual-line caching, link rectangles, selection painting/hit testing and
      accessibility use the same box calculation. This is a constraint-source consolidation,
      not completion of C1: input scroll/composition geometry and the visibility policy for
      truncated/clamped text still need review. No public primitive or new cache was added.
      C1a validation: 891 workspace tests passed, 0 failed, 3 existing ignored doctests.
      Existing rich-text/selection/accessibility, multiline/IME, Chat and renderer regressions,
      all-targets Clippy, strict core rustdoc, formatting and diff/layer checks passed.
      No real-window validation or new performance claim in this constraint-only slice.
      **C1b implemented:** `LaidText::present` now supplies document glyphs, visible source
      fragments/caret boundaries and synthetic ellipsis geometry together. Parley retains
      cluster/source intervals before extraction and clips complete shaping atoms; Mono provides
      the same projection. The old glyph API and independent document glyph/line caches are gone.
      Paint, link geometry, selection hit/paint and AX positional runs use one keyed presentation.
      Fully hidden links are rejected by shared shown/activation checks and omitted from AX;
      resize preserves allocation/Agent identity, including through hide/reveal cycles. AX child
      invalidation covers fixed-height texts too. `truncate` explicitly shows one line even with
      hard newlines; tiny widths omit an ellipsis that cannot fit. Logical whole-document copy
      remains complete, and synthetic ellipses have no source/action. See
      [text presentation](design/text-presentation.md) for exact policies and remaining work.
      C1b validation: **900 workspace tests passed, 0 failed, 3 existing ignored doctests**;
      new engine/integration checks cover CJK/emoji, real multi-grapheme ligatures, bidi/RTL,
      original Parley glyph positions, line limits/tiny widths, hidden/partly visible links,
      guarded refusal and resize identity, selection/full copy and incremental AX. Renderer,
      Chat queued long-paragraph/code bursts, HTTP and existing multiline/native-query tests pass.
      All-targets Clippy, strict core/text rustdoc, no-default-features core, format/diff/layer
      checks pass. Existing debug `bench --stream` completed with bounded work; no release/native
      timing claim. The full test run needed authorized local-loopback access for HTTP fixtures.
      **C1c implemented:** Kernel builds the input layout configuration before constructing
      Field. Committed input and native current-document queries share Field construction,
      the empty insertion line, reveal calculation and geometry conversion. Only Kernel
      persists the resulting scroll/keep key; native queries retain try-borrow refusal and
      do not enter handlers or overwrite the live input. Existing paragraph reuse and secure
      input masking stay with Kernel. Regression compares prediction with the subsequent
      published geometry after resize, manual scrolling, CJK/emoji composition selection,
      hidden composition cursor, cancellation, confirmation and clearing to an empty field.
      C1c validation: **901 workspace tests passed, 0 failed, 3 existing ignored doctests**;
      workspace all-targets Clippy, format/diff and dependency-layer checks passed. Full tests
      include existing native query refusal/recovery, secure input, multiline, HTTP and rendering.
      **Remaining C1 acceptance:** obtain separate real-window IME/scrolling/candidate evidence.
      Headless geometry agreement does not prove OS candidate placement. No real window/OS
      interaction or screenshot was performed in C1b/C1c.
- [x] Separate prompt, editable value and accessible label; consolidate node-local semantic resolution.
      **C2a implemented:** `kernel/semantics.rs` owns node-local name resolution for AX,
      Inspector, name lookup, diagnostic descriptions and missing-name checks. Explicit labels
      override document text, including an explicit empty string. Editable values never supply
      names. Tooltip fallback compares the resolved name. Name lookup uses AX visibility;
      hidden subtrees/internal links cannot be found by name. Empty/whitespace labels no longer
      satisfy missing-name checks; hidden text does not satisfy descendant-text checks.
      See [semantic resolution](design/semantics.md). Independent input prompt presentation and
      broader composite naming were left for subsequent slices; no new public primitive or retained cache.
      C2a validation: **902 workspace tests passed, 0 failed, 3 existing ignored doctests**;
      all-targets Clippy, format/diff and dependency-layer checks passed. The new regression
      cross-checks AX/Inspector/lookup/diagnostics and dynamic target retention.
      **C2b implemented:** `.placeholder(..)` is an input-only, reactive display property,
      stored separately from the editing document. It draws only when empty and not composing;
      single-line hints fit with ellipsis, multiline hints wrap under the field clip. It does
      not affect intrinsic height, selection, history, caret/native geometry or target identity.
      AX/Inspector report it separately from name/value; empty strings clear it. Secure fields
      retain value redaction. Chat's composer uses it. Builder misuse is rejected before property
      conversion. Skill/start-here copies are regenerated. See [text input](design/text-input.md).
      C2b validation: **904 workspace tests passed, 0 failed, 3 existing ignored doctests**;
      all-targets Clippy, format/diff, dependency-layer and skill freshness checks passed.
      The CLI resolves `El::placeholder`; Chat, HTTP and renderer regressions pass.
      Broader composite naming remains in phase D. Real-window acceptance is deferred at the
      user's request while away from the computer.
- [ ] Finalize and implement external-value/IME arbitration (same-value write, rejection,
      normalization, explicit reset, composition completion/cancellation and late callbacks).
      **C3 architecture review completed; R1/R2 implemented:** see
      [state-to-presentation review](design/editing-architecture-review.md), including a fresh
      review of A/B/C1/C2. Three temporary probes reproduced rejected-selection drift,
      normalized Unicode caret drift, and stale undo panicking after an external emoji value.
      The probes are now permanent passing R1 regressions.
      - [x] **R1:** one edit acceptance path and node-owned editing history. History validity
            follows actual value writes, independently of typing coalescing. Deleted the
            Window history map, fingerprints and pre-callback history commits. Typing, paste,
            IME commit, semantic Input, AX SetValue and Undo/Redo reconcile after Signals flush.
            Rejection preserves selection and redo; normalization maps Unicode selection and
            retires history. Reentry, close/replacement, panic, secure and same-value cases
            have headless coverage. No new public editing primitive.
            Validation: **915 workspace tests passed, 0 failed, 3 existing ignored doctests**;
            all-targets Clippy, format/diff, dependency layers and skill freshness passed.
            The CLI resolves `EDIT_PENDING`. Real-window validation remains deferred.
      - [x] **R2:** Field reads no longer write persistent scroll/keep state. Kernel layout
            settles those ordinary fields explicitly, including caret-only updates; native
            queries predict geometry with the same calculation without writing core state.
            Read-order, manual-scroll/no-op, resize-clamp and busy-engine tests cover the
            mechanism; Inspector/paint/AX/native geometry agreement is checked at Window level.
            Validation: **918 workspace tests passed, 0 failed, 3 existing ignored doctests**;
            all-targets Clippy, format/diff, dependency layers and skill freshness passed.
            No real-window claim; OS IME/candidate validation stays in R3.
      - [ ] **R3:** close same-value/external-replacement/IME/native-batch/lifecycle matrices,
            then real-window qualification when the user is available.
      The review recommends retaining immediate different-value replacement, without adding a
      pending external value queue or reset primitive. This supersedes the earlier deferred-write
      candidate; R1 retains immediate replacement. R3 lifecycle/platform acceptance remains
      unfinished, including real-window verification deferred by the user.

### D. Core composition and ui-kit

- [ ] Validate public foundation sufficiency with a no-kit custom button, labeled Chinese
      input with errors, and anchored suggestions. No kit-only focus/editing privileges.
- [ ] Share stable action roots while allowing different visual content. Make actual control,
      label and container configuration explicit for composite widgets; remove misleading bare
      El forwarding. Prefer parameters/builders before public Parts or new traits.
- [ ] Share option identity/navigation where semantics agree; remove Combobox/PopupList's
      redundant option mirrors. Preserve Menu versus suggestions versus selection differences.
- [ ] Separate component appearance from behavior using the existing style/theme machinery;
      migrate component-only policy out of the kernel without a second theme system.

### E. Integration and completion

- [ ] Migrate Chat and remove workarounds replaced by the new contracts. Update vocab, skill,
      architecture and examples together; remove every superseded public path.
- [ ] Run headless regressions and targeted performance checks; obtain separate real-window
      evidence for Chinese IME, selection, anchored/nested overlays and streaming Chat.
      Report missing OS evidence explicitly, never infer it from headless tests.
- [ ] Final review: dependencies still point down; facts have one mutation owner; no new state
      mirrors/VM/global transactions; custom components work without kit; API usage and code
      ownership are simpler. Preserve existing Signals, cancellation, failure/release and
      target-identity guarantees instead of reopening them without concrete evidence.

Each slice ends with migrated callers, deleted obsolete paths, appropriate verification and
an updated status here. The next slice starts from the resulting contract, not another patch
on the retired architecture. Reference the recorded Rune/Exact/W3C evidence selectively.

## Priorities

1. Core interaction and reusable components, exercised in Showcase and the Chat reference app:
   composed keyboard/IME behavior, focus, selection, overlays and real-window evidence.
2. Extend Agent feedback from real windows: target/build identity, sampled properties and native input.
3. The owned macOS backend beyond the implemented NSTextInputClient adapter and synchronized
   native-window resize in the current shell.
4. Native chrome: Windows/Linux menu integration, AppKit tooltips, toolbars and title bars.
   macOS menus and alert sheets, and rfd file dialogs are implemented in the winit shell.
5. Components: tables, trees, sheets and richer platform navigation.
   Lists, popovers, comboboxes, fields, radios, dialogs, tabs, segmented choices, editor-owned
   suggestions, sliders, number fields, progress and drawn command menus are implemented.
6. Images: JPEG, WebP, SVG, loading and caching.
7. [Native Island](design/native-island.md): focus, accessibility joining, lazy creation.
8. Windows: WebView2 in DirectComposition; `surface` textures; Linux: WPE as a texture.
9. Text depth: direct platform text protocols, incremental layout of long paragraphs.
10. iOS, then Android.

## Runtime and Agent loop

Basis: [runtime design](design/runtime-contract.md), [Agent context](design/agent-context.md),
[agent knowledge](design/agent-knowledge.md), and [the Exact comparison](research/exact-runtime-simplicity.md).
Build the loop with existing owners and inspector facilities, preserving Rust + Signals.

- [ ] Add reliable attached-build identity and stronger target validation across tree replacement,
      using existing handles and local generations where required.
- [ ] Expose useful property origins and sampled opacity/color: `visual_of` currently gives the
      target value. Semantic values, source, bounds, diagnostics and pending work already come
      from the live owners. Business-value adapters stay optional; arbitrary Rust values need no
      serialization bound.
- [ ] Qualify native focus, keyboard/IME and gestures with real OS or simulator input. Current live
      replies identify semantic/framework routes; framework dispatch does not exercise OS input injection.
- [ ] Add a live transport for Windows. Any future live clock-control operation needs explicit
      host/clock semantics; `advance` remains headless-only.
- [ ] Give pending requests an owner and honor host lifetime when window-bound; cancel or withdraw
      where supported, reject late replies, and validate content/measurement generations at asynchronous boundaries.
- [ ] Fix cross-window dirty propagation and cleanup ordering where feedback exposes stale output;
      revisit affected work rather than requiring an app-wide fixed point for every operation.
- [ ] Keep each window's native placements, drawn output and interaction geometry consistent when
      redraws coalesce; report the available capture/feedback stage honestly.
- [ ] Define reset/restart for broken internal invariants separately from ordinary unfinished work;
      live realtime settling already reports pending work when its bounded wait expires.
- [ ] Complete the public API docs, especially undocumented variants, fields and methods.
- [ ] `octara new` (calls `skill install`).

Existing diagnostics and Kernel cause information remain. Add deeper dependency tracing, generic
snapshot/watch or guidance evaluation only when a concrete debugging or development task needs it.

## To research and design

- [ ] Platform shells and remaining native chrome: Windows/Linux menus, toolbars (`NSToolbar`),
      title bar and traffic lights, sidebars (system material underlay with drawn content, as
      Rune), native tooltips; on iOS / Android, native navigation (navigation bars, edge-swipe back, tab
      bars): drawn to match the platform, or one Octara surface per screen inside the platform's
      navigation container; whether winit stays on each platform.

## Layout

Basis: [layout research](research/layout.md), [layout design](design/layout.md).

- [ ] A single very long paragraph (no line breaks) is re-laid out as a whole: 100 KB takes about
      16 ms / token. Rune has not solved this either; it tried "per-word caching + its own line
      breaking", which was only 1.5x faster on a cold start, and gave it up. For the append case,
      consider shaping in chunks split on whitespace within the paragraph and resuming line
      breaking; another route is for the component library's Markdown to split into multiple
      texts by block (Rune's main technique).
- [ ] Container queries (`.container()` + `.container_md(..)`) and styles by container size (the
      same mechanism as window size classes), with a feedback-loop limit (`LAYOUT_LOOP`).
- [ ] Tailwind palette constants (`colors::BLUE_500`, etc.), for direct use in the prototyping
      stage.

## Rendering

Basis: [rendering research](research/rendering.md), [rendering design](design/rendering.md).

- [ ] More deterministic fallback fonts: Korean, Traditional Chinese, emoji (color), Arabic, etc.,
      adding features as needed.
- [ ] Inset shadows.
- [ ] More than two color stops, radial gradients.
- [ ] Images: JPEG / WebP / SVG, async loading and caching (together with resources), mipmaps when
      scaled down, large images exceeding the atlas, re-layout after the shape changes.
- [ ] The GPU renderer converts every item to instances each frame: keep a part's instances
      and upload only the parts that changed. A layer covers only its extent (it covers the
      whole window).

## Input

Basis: [input research](research/input.md), [input design](design/input.md).

- [ ] Deferring the app's rewrites of the value during composition (Exact); touch long-press word
      selection and selection handles; the caret position within an IME composition.
- [ ] Accessibility `ScrollIntoView`, text fields' runs and selection (AccessKit); scroll into view
      before driving; multi-finger gestures, edge back swipe; direct platform text protocols
      (NSTextInputClient, etc.).

## Platform

Basis: [platform design](design/platform.md); done with the core's platform boundary step.

- [ ] Windows: native context menus (`TrackPopupMenuEx`, opened from a posted message as Rune's)
      and alerts (`TaskDialog`), and a menu bar; drawn menus and prompts until then. Linux: a
      menu bar.
- [ ] Close requests: Quit and a window's close ask the app first (`on_window_closed`), so it
      can confirm unsaved work; Quit ends the process now.
- [ ] Windows' frame clock: a thread waiting on `DwmFlush` sending frame ticks (GPUI's
      `vsync.rs`), when Windows has native views or a measurement shows the swap chain's
      present does not pace.
- [ ] Verify a direct native text adapter, then one mobile host pilot with real simulator input:
      composition, keyboard geometry, cancelled navigation and replies after scope disposal.

## Text

Basis: [text design](design/text.md).

- [ ] Markdown: images (with image loading), syntax colors in code blocks, a long code block's
      lines virtualized, inline code sized to its text (a span size relative to the text's), a
      task box pressable (`on_task`).
- [ ] An empty text has no height: a code block's empty line shows a space (copied as one).

## Async and motion

- [ ] Resource deduplication cache, deferred placeholders, forbidding real time in tests, stream
      backpressure and coalescing ([async](design/async.md)).
- [ ] Appearing and disappearing: same-key revival (Rune), `.transition(..)` (scale, slide in),
      exiting children follow the parent ([animation](design/animation.md)).
- [ ] The shell reads the system's reduced-motion setting (winit does not provide it;
      per-platform interfaces are needed).

## Known limitations

From section 9 of [Reactive core](design/reactive-core.md):

- [ ] Kernel children are a `Vec`; with a hundred thousand sibling nodes insertion and removal
      are O(n) (avoided by virtualization).
- [ ] After hot reload, virtual list rows are measured again.
- [ ] Hot reload preserves named text values and focus, but rebuilds the editor's selection,
      composition and undo history; the caret returns to the start. Live option updates keep
      the editor mounted and do not have this limitation ([Choices evidence](../examples/CHOICES.md)).
- [ ] On hot reload, a change of state type can only reset: add a migration function (serde or
      user-provided).
- [ ] Durability tiers (Salsa), for rarely changing state.
- [ ] Taffy 0.15.1: after a container removes its last absolutely positioned child, the hoisted
      list is not cleared, and rounding reads a deleted node. The kernel gives that container a
      fresh taffy node (`refresh_taffy_node`); report upstream and delete once fixed.
