# Research: Signals implementations

Status: research notes (2026-11). This is the source-level research done before choosing fine-grained Signals (conclusions in
[Reactive core](../design/reactive-core.md)). It answers whether that approach is viable in Rust native UI, and whether Rune's
criticisms hold in existing implementations.

Source reading (read-only, nothing built or run):

| Project | Version / commit | Date |
| --- | --- | --- |
| Leptos `reactive_graph` | 0.3.0-rc1, `5a6d608` | 2026-11-09 |
| Floem `reactive` | `1351ffb` | 2026-09-23 |
| Sycamore `sycamore-reactive` | 0.10.0-beta.1, `1574d5d` | 2026-11-10 |
| Dioxus `signals` / `generational-box` / `subsecond` | `17d9a59` | 2026-11-09 |
| SolidJS 2.0 (`next` branch of `solidjs/solid`, `packages/signals`) | 2.0.0-rc.21, `aaf528d` | 2026-11-10 |
| alien-signals | 3.2.1, `113f581` | 2026-11-04 |
| Preact Signals | core 1.14.5, `30a797e` | 2026-10-27 |
| TC39 proposal-signals / signal-polyfill | `9124ed9` / `1c33f91` | 2025-08 / 2025-02 |
| Reactively | `9081fa4` | 2023-10 |

Every `path:line` cited in the text refers to the commits in the table above. Conclusions marked "inferred" come from reading the
source and have not been verified by running it.

## 1. Summary of conclusions

1. **The algorithms are mature.** The mainstream implementations have converged on "push marking + pull evaluation"
   (Reactively's three-color marking and its variants), which gives no intermediate states and no memory allocation in steady
   state. alien-signals is the smallest correct implementation and is well suited to porting to a Rust arena.
2. **"Reads outside a tracking scope" can be diagnosed, but only in limited scenarios.** Untracked reads in event callbacks are
   legitimate, so they cannot be detected globally without false positives. SolidJS 2.0 only checks in scenarios where a one-off
   read is almost certainly a bug (component bodies, Effect callbacks), and emits numbered, machine-oriented structured diagnostics
   that carry the component path. This is already implemented in production-grade code.
3. **Some cases are still not covered by diagnostics:** reads under the wrong observer (silently subscribed by another, unrelated
   Effect), copying a value into a plain variable, and reads in async code.
4. **Cycle and disposal handling is generally weak:** most Rust implementations have no cycle detection; accessing a disposed
   Signal panics, while writing to it silently has no effect. These are implementation-quality issues, not flaws of the model.
5. **Hot reload is the real weakness of Signals.** None of the existing Rust implementations can reliably preserve component
   state. The claim in the Rune notes that "Dioxus 0.7 can preserve state after a hot patch" is inaccurate: after every patch,
   child components are remounted and lose their state.
6. **Fine granularity breaks at the UI tree.** Floem's Signals are precise down to the Effect, but after that the whole view's
   style is recomputed in full, and painting is per window. Property-level invalidation classification does not exist in any of
   the implementations surveyed and has to be designed by Octara.

## 2. Propagation algorithms

| Implementation | Algorithm | Glitch-free | Execution order | Batching |
| --- | --- | --- | --- | --- |
| Reactively | Three-color marking: mark Dirty/Check → evaluate on demand when read | Yes | Determined by evaluation order | — |
| alien-signals | Variant of three-color marking, edges stored in intrusive doubly linked lists, 6-bit state flags | Yes | Outer Effects before inner ones | Effects deferred during `startBatch` |
| Leptos | Three-color marking; Memos evaluate on demand, Effects are async tasks | Memos are | Order between Effects depends on the executor queue, **undefined** | Implicit coalescing (an Effect's dirty mark is a single bit) |
| Sycamore | DFS topological sort from the changed node, dirty nodes run in topological order; reads also evaluate on demand | Yes | Topological order; an Effect is just a Memo returning `()` | Counter, nestable |
| SolidJS 2.0 | Heap bucketed by topological height + Check/Dirty marks | Yes | The compute part of all Effects before the callback part of all Effects | `batch` removed; applied together in a microtask or on an explicit `flush()` |
| Preact | Push notification + version-number comparison on pull, global version number as the fast path | Yes | Effects run **last in, first out**, a hidden pitfall | `batch` |
| Floem | Pure push, runs immediately; Memos are in fact high-priority Effects | **No** (inferred: under a diamond dependency an Effect may see a Memo's old value) | Sorted by priority and id | A single boolean flag |

References: alien-signals `src/system.ts:9-250`, Leptos `graph/node.rs:19-27`, Sycamore `root.rs:198-236`,
Solid 2.0 `core/heap.ts:19-134`, Preact `packages/core/src/index.ts:504-529, 940-946`, Floem `runtime.rs:162-185`.

**Trade-off:** whether a write is readable immediately (alien-signals, Preact, TC39) or deferred until flush (Solid 2.0: after
`setX(2)`, `x()` still returns the old value until `flush()`). The latter is unintuitive for imperative code; the former requires
bindings to be applied together at the frame boundary.

## 3. Ownership and disposal

All Rust implementations use an arena plus `Copy` handles, which avoids lifetimes:

- **Leptos:** a process-wide global `SlotMap` wrapped in an `RwLock` (`owner/arena.rs:22-25`); an `Owner` disposes child Owners,
  cleanup functions and nodes in cascade.
- **Sycamore:** a single `SlotMap` plus `RefCell`; every `create_root` leaks a `Root` (`root.rs:59-73`).
- **Floem:** a monotonically increasing `u64` id plus a `HashMap`; ids are not reused, so an old handle never points to a new Signal.
- **Dioxus:** `generational-box`, where a `Copy` handle is made of "slot + generation", and slots are reused through a
  thread-local free list.

When a disposed Signal is accessed:

| Implementation | Read | Write |
| --- | --- | --- |
| Leptos | panic; debug builds give the definition location and access location (`traits.rs:674-689`) | **Silent no-op** |
| Sycamore | panic; debug builds give the creation location | — |
| Floem | `unwrap()` panic with no information (`read.rs:89`) | **Silent no-op** |
| Dioxus | `BorrowError::Dropped`; debug builds include the creation location | — |

Floem also has a hazard: disposing a Signal also disposes every Effect subscribed to it (`id.rs:103-110`). If such an Effect
also depends on other Signals, it silently stops updating.

## 4. Diagnostics for "reads outside a tracking scope"

This is Rune's main criticism of Signals, and the problem with the greatest impact on agents.

| Implementation | Detected | Detection scope | Form | Gaps |
| --- | --- | --- | --- | --- |
| SolidJS 2.0 | Yes | Only in scenarios marked strict: component bodies, Effect callbacks, control-flow callbacks | Structured diagnostics: fixed code `STRICT_READ_UNTRACKED`, severity, component path, source location; a dedicated diagnostics package for agents to read | `untrack()` is an explicit exemption; reads after an async boundary are detectable only on V8 |
| Leptos | Yes | There is currently no observer and the read is not in an exempt zone (event callbacks, `untrack`, async) | A one-line log in debug builds, not deduplicated | Reads under the wrong observer are not reported |
| Sycamore | Partial | Only inside component bodies (`trapping_scope`); panics under the `strict` feature | Log or panic | The documentation claims a wider scope; the code only checks component bodies |
| Preact | No (runtime) | Statically checked by ESLint rules, such as `no-value-after-await` | lint | Covers only the patterns lint can recognize |
| Floem, Dioxus, alien-signals | No | — | — | Reads in async code silently do not subscribe (Dioxus) |

Solid 2.0's related diagnostics (`packages/signals/src/core/dev.ts`):

- `STRICT_READ_UNTRACKED` (warning): a reactive value is read directly in a strict scenario; it says "will not update" and gives a
  location for the fix;
- `REACTIVE_WRITE_IN_OWNED_SCOPE` (throws): reactive state is written during a component or computation; wrapping it in `untrack`
  does not exempt it;
- `NO_OWNER_EFFECT`: an Effect created outside a reactive context will never be disposed;
- `UNTRACKED_READ_AFTER_AWAIT`: a read after `await` is not tracked (V8 only, "may miss cases but never reports falsely");
- In addition, an attribution engine can prove a feedback loop from the dependency graph and report `EFFECT_WRITES_OWN_SOURCE`.

**Conclusions:**

- Globally detecting "reads with no observer" necessarily produces false positives, because such reads in event callbacks and
  imperative code are legitimate. The workable approach is to detect only in limited scenarios and require intentional one-off
  reads to be marked explicitly (`untrack` / `get_untracked`).
- What the diagnostics cover is exactly the typical case Rune worries about: reading directly in a component body, so the UI does
  not update.
- Cases the diagnostics do not cover must be avoided through API design, see section 10.

## 5. Cycles and re-entrancy

| Implementation | Behavior | Error |
| --- | --- | --- |
| Preact | Detects a Computed reading itself, an Effect re-entering itself, and an Effect re-running more than 100 times within a batch | `"Cycle detected"` |
| Sycamore | Detects cycles in DFS; disconnects its own dependencies before running, so an Effect writing its own dependency does not form a cycle | `"cyclic reactive dependency"` |
| SolidJS 2.0 | Stops after a single flush exceeds 100,000 rounds | `"Potential Infinite Loop Detected…"`, with the last node attached |
| TC39 polyfill | Throws when a Computed reads itself | `"Detected cycle in computations."` |
| Leptos | When an Effect writes a Signal it reads, it re-runs forever on the next round (treated as expected behavior in tests) | None |
| Floem | A write inside an Effect re-runs that Effect synchronously; an unconditional self-write recurses until stack overflow (inferred) | None |
| alien-signals | When a Computed reads itself it silently returns the cached value; flush has no round limit | None |

## 6. Threading model

- **Leptos** is thread-safe by default (`Arc<RwLock>`, values in the arena must be `Send + Sync`), on the grounds of not binding
  to a specific async runtime and supporting SSR; the cost is acquiring a global `RwLock` on every arena access.
- **Floem**'s runtime is thread-local and Signals are `!Send` by default; there is also `SyncStorage`, which queues writes from
  other threads and wakes the UI thread. Floem made Signals `!Send` after a panic from cross-thread `set()`
  ([#534](https://github.com/lapce/floem/issues/534)), and a multi-threaded runtime is marked "not planned"
  ([#686](https://github.com/lapce/floem/issues/686)).
- **Sycamore** and **Dioxus** are both thread-local and `!Send`.

For native UI with no SSR requirement, a single-threaded runtime with `!Send` handles, with background results sent back to the UI
thread through a scheduler, is the simpler and faster choice.

## 7. Hot reload

- **Leptos** integrates subsecond: each `RenderEffect` records its own function pointer, and after a patch only Effects whose
  pointer changed are re-run (`effect/render_effect.rs:175-212`). Signals created inside a re-run closure are cleaned up, so only
  state outside the closure is preserved. The maintainers admit in a code comment that the mechanism is "not fine-grained enough
  and of limited practical use" (`render_effect.rs:139-141`).
- **Dioxus** keeps state in each scope's hook list, outside the component body, and re-runs the component after a patch. But a
  component's identity is determined by its function address, and after a patch every function gets a new address, so the diff
  takes the replace path and child components are destroyed together with their state (`vdom/src/diff/component.rs:167-168`).
  Only the root component preserves state. The subsecond documentation also admits that Dioxus apps "simply discard the old state
  and rebuild from scratch" (`subsecond/src/lib.rs:108-109`). In addition, hooks are looked up by index: inserting a hook of the
  same type earlier mismatches state onto another variable, with no error.
- **Floem, Sycamore and the JS libraries** have no hot-reload mechanism in their core.

**Implications for Octara:** preserving state in run-once components requires at least:

1. State lives in a store outside the closures, indexed by "component instance identity + explicit key", and closures hold only
   `Copy` handles;
2. After a patch, enter a "re-run the component body" mode: rebuild Effects, bindings and closures, but recover existing Signals by
   key;
3. Component identity cannot use the function address. Source location is also unreliable, because patches move line numbers (Rune
   rejected `#[track_caller]` identity for this reason). Candidates are an explicit key, or "component name + position in the
   tree";
4. Every slot is type-checked (`TypeId` or a layout hash), and on mismatch it is reset and reported rather than silently
   mismatched.

This mechanism is essentially the same as Rune's `Model<T>` store. The difference is that Signals re-run a component only on hot
reload; the rest of the time it remains run-once.

## 8. Connection to the UI tree

Floem is architecturally the implementation closest to Octara (Signals + retained tree + Taffy + GPU), and its current state shows
where fine granularity breaks:

- A change to any Signal in `.style(closure)` triggers a full recomputation of that view's style (`views/decorator.rs:80-94`);
- Whether layout is invalidated can only be known by comparing the regenerated Taffy style (`style/cx.rs:577-590`);
- Paint invalidation is per window, and a code comment admits the id is "not used currently, may be used later to compute the
  dirty region" (`window/state.rs:1229-1232`);
- Animations are driven frame by frame in the style phase on the main thread;
- The Inspector can show the view tree, layout and style timing, but **does not show the reactive dependency graph at all**.

Keyed lists (`dyn_stack`) maintain identity by key, with one child scope per row that is disposed on removal; but in
`virtual_stack`, rows scrolled out of the viewport are destroyed together with their state, losing selection and focus
([#867](https://github.com/lapce/floem/issues/867)).

**Conclusion:** Signals only solve "which bindings need to re-run"; they do not solve "does this change affect layout, paint, text
or semantics". Property-level invalidation classification needs a separate design on the UI tree side. Floem's
`StyleReasonFlags` is the closest starting point.

## 9. Re-evaluating Rune's criticisms

| Criticism | Assessment | Basis |
| --- | --- | --- |
| Reads outside a tracking scope silently get stale values | **Partly true.** The typical case (reading directly in a component body) can be diagnosed; Solid 2.0 already has a structured solution and Leptos also warns; but reads under the wrong observer, copies into plain variables and async reads are still missed | Section 4 |
| Hot reload loses state | **True.** No existing Rust implementation can do it; it needs an external store + re-run after a patch + stable identity, and no implementation yet proves it feasible | Section 7 |
| Cycles, batching order, panics after disposal | **True, but implementation-quality issues.** Sycamore, Preact and Solid 2.0 all report cycles; execution order can be specified explicitly | Sections 3, 5 |
| Property writes still trigger style, layout and paint on the main thread | **True, and independent of the state model.** Needs property-level invalidation classification and render-thread animation | Section 8 |

## 10. If Signals are adopted: draft of Octara's reactive core

The following is design input for a validation prototype, not a final decision.

**Algorithm and runtime:**

- Port alien-signals' push-pull three-color marking: nodes and edges live in an arena, indexed by `u32`; edges are reused in
  order; no recursion, traversal with an explicit stack;
- Single-threaded runtime, `!Send` handles; background results are sent back through the UI scheduler;
- A write is readable immediately; property bindings and Effects are flushed together at the frame boundary;
- Specified execution order: Memos evaluate on demand, property bindings run before user Effects, Effects run in creation order,
  outer before inner;
- Handles carry a generation; accessing a disposed node returns a structured diagnostic instead of panicking or silently failing.

**Diagnostics (all with a fixed code, component path and source location, emitted to the Inspector as structured events):**

| Diagnostic | Level | Condition |
| --- | --- | --- |
| Untracked read in a strict scenario | Warning, error in strict mode | During component build or in an Effect callback, a read outside a binding closure |
| Write during build | Error | A Signal is written during component build or computation |
| Cycle | Error | A Memo reads itself; the full node chain is reported |
| Runaway flush | Error | A single flush exceeds the round limit, with the last few nodes attached |
| Access to a disposed node | Error | Both reads and writes are reported, with the definition location and access location |
| Effect without an Owner | Warning | An Effect is created outside a scope |

**Avoiding, through the API, the cases diagnostics do not cover:**

- Property APIs accept only fixed values or closures, and binding closures always run under tracking, which removes "forgot to wrap
  in a closure" by construction;
- A one-off read must be written as `get_untracked()`; there is no implicit untracked read;
- User Effects are restricted, and derived relationships are encouraged to be expressed through property bindings and Memos;
- Async code can only write results back to Signals through the scheduler, and does not read Signals in async code.

**Hot reload:** the four requirements in section 7.

**Causal tracing:** every write carries the id of the event that triggered it, propagated through flush to Memos, bindings and
properties, forming an "interaction → write → binding → property" causal chain. The dependency graph itself can also be exported
as a whole for the Inspector to view. None of the existing implementations has this, and Octara plans to provide it.

**Questions the prototype needs to answer:**

1. How many injected errors the section 10 diagnostics catch across the four test apps, and how many false positives they raise;
2. Whether an external store + re-run after a patch can preserve state without modifying application code;
3. Whether property-level invalidation classification can confine a Signal write to the necessary subsystems;
4. How the agent's task success rate and number of repair rounds compare with candidate B.
