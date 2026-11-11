# Research: Exact (Expo)

Status: research notes. Based on the README, `docs/` and `runner/` source of the [expo/exact](https://github.com/expo/exact)
default branch `bb85428a` (2026-11-10). Exact is led by Expo co-founder Charlie Cheever and was open-sourced under the
MIT license in 2026-11. It is pre-alpha: the repository states there is no API stability, backward compatibility or
migration guide. The design documents (the LLP series) are in a private repository; this note cites public content only.

## Positioning

"Write an app once, run it natively on the Web, macOS, iOS and Linux, and have an AI agent build, run, observe and test it
on every platform."

Exact belongs to the same "Agent Native" family of frameworks as Octara and Rune, but its approach differs the most:

- The UI is written in a custom declarative language, **Contract**; data logic is written in TypeScript or Rust;
- On Apple platforms it uses **real native controls** (AppKit / UIKit), and on the Web it drives the DOM directly;
- The Web is the main development loop and the reference for layout consistency ("Write the web, ship the platform").

## Architecture

```text
app.contract (views, state, actions) ─▶ Contract compiler ─▶ plan (tables + bytecode, constant data baked at build time)
                                                        ├─▶ Web: one ES module + ~20 KB runtime, drives the browser DOM and CSS
                                                        └─▶ Rust runner (plan VM) + kernel (Taffy layout)
                                                              ├─ macOS: AppKit + CoreText
                                                              ├─ iOS: UIKit + CoreText
                                                              └─ Linux: vello GPU / tiny-skia CPU self-drawn
app.ts or Rust crate (data source) ─▶ replies validated against the shape declared in the Contract
```

| Layer | Exact's choice |
| --- | --- |
| UI language | Contract: `shape`, `component`, `state`, `derive`, `resource`, `mutation` / `send`, `action`, `task`, `view`, `style` |
| Data logic | TypeScript (on native, runs on Hermes, compiled to bytecode at build time and loaded only after the first frame) or Rust (hot-swappable as a dynamic library or Wasm) |
| Layout | kernel: columnar node arena, patched Taffy, laid out with CSS semantics (a bare node is equivalent to `<div>`); on the Web, layout is left to the browser |
| Text | Measured by the host: CoreText on Apple, cosmic-text on Linux |
| Rendering / presentation | Native views and native controls on Apple; self-drawn on Linux; Windows uses the ink painter on DirectX 11 or the CPU (early); Android is early |
| Native extension | A native module is a hyphenated tag (such as `exact-map`), implemented in Swift / Kotlin / Rust and laid out uniformly by the kernel; `hatch="word"` hands the platform object that Exact created to native code for configuration |
| Permissions | Grants control network, files, SQLite and secrets |
| Delivery | Signed plan and resource update bundles, which the app fetches after the first frame |

## State and update model

Contract rules:

- `state` is a value owned by the component; child state inside a `for` belongs to the row of the corresponding key;
- `derive` is a derived value that is recomputed when its dependencies change;
- `action` is the only place that can modify state, and the compiler infers which state it writes from the function body;
- View expressions have no side effects; actions have no loops and no `await`, and time-consuming work is handed to the data source;
- Lists must have keys.

The runner's update mechanism (`runner/src/instance.rs`, `runner/src/instance/deps.rs`):

- **Static dependency table:** at startup the runner scans the plan once and learns which inputs each binding, site, derive and
  resource parameter reads; each input occupies one bit in a bitmap.
- **Only invalidated bindings are recomputed:** on update, only sites whose inputs changed are visited, binding operations are
  issued only for changed values, and instances are created or destroyed only for added or removed keys.
- **No tree diff:** row identity is preserved by key, independent of position.
- **Full reference:** `Update::full` evaluates all bindings and serves as the correctness reference for incremental updates.

This amounts to the update precision of Signals, but the dependencies come from the compiler's static analysis rather than runtime
tracking. As a result there are no silent errors such as "read outside a tracking scope": the read relationships are fixed at
compile time, and incorrect code is rejected by the compiler. The cost is that a closed DSL is required.

## Agent tooling

- **Ten operations, consistent across hosts:** `tree`, `screenshot`, `tap`, `type`, `state`, `layout`, `logs`, `clock`, `prefer`,
  `perf`, driving Web, macOS, iOS (simulator or device) and Linux through the same script. Keeping exactly ten was deliberate:
  the previous generation's agent API had bloated to eighty.
- **The clock belongs to the agent:** nothing moves between two operations; time is advanced with `clock +60000` or
  `clock settle` instead of waiting for animations to finish.
- **A test is an operation script:** `app.test.contract` runs unchanged on any host.
- **Machine-oriented tool output:** `contract build --json` gives diagnostics with stable ids and source ranges, reports up to
  twenty independent errors at a time, and offers fix suggestions (for example, suggesting the Web property name instead);
  `contract symbols` provides definitions and references; a development-time source map can locate, from any node on screen, the
  source line that declares it; the driver refuses to drive a build older than the source.
- **The language is deliberately kept small:** a whole app fits in one context window; every write is declared, every list has a
  key, and every value has a type.

One example: the README records that a new agent, on a clean clone and following the prompts, completed a Todo app that passed its
tests on three platforms in about 57 minutes, about 25 of which were the first macOS and iOS builds.

## Performance budget

The budgets in `rules/RULES.md` are tracked on every commit, and regressions are treated as P0:

| Budget | Target |
| --- | --- |
| Cold start to interactive first frame | p50 100 ms |
| Edit to presentation during development | p50 100 ms |
| Application JavaScript executed before the first frame | None |
| Rebuild that crate after changing one line | 30 s |
| All blocking checks | 65 s |

## Comparison with Rune and Octara

| Dimension | Exact | Rune | Octara (current plan) |
| --- | --- | --- | --- |
| UI authoring | Contract DSL | Rust builder, no macros | Rust (main API shape to be decided) |
| State updates | Compile-time static dependencies + recompute only invalidated bindings | Re-run the whole view + manual memo | Fine-grained Signals + incremental collections, see [Reactive core](../design/reactive-core.md) |
| Appearance | Platform-native controls and default styles | Own design language, consistent across platforms | GPU by default, supplemented by Native Islands |
| Rendering | Apple native views; Linux uses vello / tiny-skia | In-house wgpu SDF renderer | wgpu |
| Layout | Taffy, strictly following CSS semantics | Taffy | Taffy |
| Text | CoreText / cosmic-text | Parley + HarfRust | Parley |
| Web | First-class target, drives the DOM | wasm + WebGPU | Not supported |
| Application logic | TypeScript (Hermes) or Rust | Rust | Rust |
| Agent interface | Ten fixed operations, tests are scripts | Inspector socket protocol, Scenes | Semantic Inspector, causal tracing |

## Implications for Octara

- **Static dependencies are a third path.** Exact shows that "Signals precision + compile-time errors" can be had together,
  provided the language is closed enough for static analysis. `ARCHITECTURE.md` already rules out a Contract DSL; in Rust,
  arbitrary closure code cannot be statically analyzed and can only be approximated (runtime tracking plus diagnostics, or a macro
  that analyzes restricted binding expressions). See [Reactive core](../design/reactive-core.md).
- **Validate incremental updates against a full reference.** `Update::full` as the reference for incremental updates is the same
  idea as Rune's "reference implementation + differential testing of the fast implementation". It should be adopted whichever
  state model is chosen.
- **Limit the number of agent operations.** Ten operations with a stable version are easier for an agent to master than a growing
  API. Octara's agent protocol should control the size of the operation set from the start.
- **The clock belongs to the agent:** time stands still between operations, and advancing it is initiated explicitly by the agent.
  This is consistent with Octara's requirement of deterministic replay.
- **Tests are operation scripts, consistent across hosts:** headless and real-window runs execute the same test.
- **Diagnostics are machine-oriented:** stable error ids, source ranges, several errors reported at once, fixes suggested, and
  stale builds refused.
- **Writes must be explicit:** an action is the only write point, and the write set is inferred by the compiler. Whichever model
  Octara chooses, it can borrow the constraint that "writes happen only in handlers".
- **The trade-off of the native-look approach:** Exact apps look like native apps on every platform, at the cost of an appearance
  bound by platform controls; Rune chose the opposite path. Octara's GPU First plus Native Islands sits between the two and needs
  an explicit stance on the default appearance.
