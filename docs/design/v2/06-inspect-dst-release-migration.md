# Octara v2 Spec 06: Agent Protocol, Deterministic Simulation Testing, Signed Delivery & Zero-Baggage Migration

> Part of the [Octara v2 Clean-Sheet Architecture](../architecture-v2.md).

---

## 1. `octara-inspect`: Unified Agent Protocol & Transports

Cleanly decoupled from both `octara` runtime and `octara-host-native`.

### Module Structure

```text
crates/octara-inspect/src/
  lib.rs
  protocol.rs        # octara.inspect/2 JSON-lines schema: AgentCommand, AgentReceipt, TreeSnapshot, TargetRef
  driver.rs          # HeadlessDriver (App + NativeEngine + CpuRenderer + VirtualClock)
  replay.rs          # Deterministic Recording & Replay over 4-channel Input events
  dst.rs             # Deterministic Simulation Testing (DST) harness & randomized fault/interleaving generator
  conformance.rs     # Cross-host platform contract verifier
  transport/
    mod.rs           # InspectTransport trait
    unix.rs          # #[cfg(unix)] non-blocking Unix domain socket server (macOS, Linux, iOS simulator)
    web.rs           # #[cfg(target_arch = "wasm32")] browser window.__octaraInspect bridge
```

### 1.1 `octara.inspect/2` Protocol Contract
- **No `AgentCommand` inside core OS input enum**: `AgentCommand` (`Observe`, `Act { target, action }`, `Capture { source, settle_ms }`, `AdvanceClock { ms }`, `SetPicking`, `ListWindows`) lives exclusively in `octara-inspect`.
- **Execution translates `Act` into real `InteractionEvent` / `EditorEvent` / `HostReply`** using the window's exact `TargetGuard` validation — Agent actions traverse the same `InteractionArbiter` and `Turn` engine as real user input.
- **Structured Causal Receipt**:
  ```json
  {
    "protocol": "octara.inspect/2",
    "ok": true,
    "delivery": "delivered",
    "window": 1,
    "progress": {
      "observation_rev": 14,
      "tree_rev": 9,
      "settled": true,
      "committed_frame": 12,
      "presented_frame": 12
    }
  }
  ```
  Stale targets deterministically return `ok: false`, `code: "STALE_TARGET"`, `delivery: "refused"`.

---

## 2. Deterministic Simulation Testing (`octara-inspect::dst`)

Because Octara v2 has **zero `thread_local!`**, **explicit `VirtualClock`**, **serializable 4-channel inputs**, and **pluggable `HostEngine`**, an entire multi-window app is a pure deterministic state machine.

### `SimHarness` Design
```rust
pub struct SimConfig {
    pub seed: u64,
    pub steps: usize,
    pub windows: usize,
    pub inject_resizes: bool,
    pub inject_ime_interleavings: bool,
    pub inject_list_reorders: bool,
    pub inject_async_delays: bool,
    pub inject_focus_losses: bool,
}
```
- Drives random valid & adversarial sequences (pointer down/move/cancel, rapid CJK composition + mid-composition list reorder + row deletion, modal open/close + Escape + outside press, window resize + scroll reveal, delayed async replies) under a deterministic PRNG (`seed`).
- Asserts global invariants after **every single step**:
  1. `Tree` topological integrity (no cycles, `test_id_index` exact match with attached non-exiting nodes, `row_children` exact match).
  2. `NativeEngine` cache equivalence (`check_paint_cache = true` — cached `DisplayList` bit-identical to uncached rebuild).
  3. `AccessKitProjector` incremental update exact match against full-tree rebuild.
  4. Zero leaked `ScopeId`s, zero dangling `ReactiveId`s, zero orphaned handlers after window close / shutdown.
- Runs **in parallel across all CPU cores** via standard `cargo test` (impossible in v1 without TLS contention). Any failure prints its exact `seed` + minimal event trace for instant deterministic reproduction.

---

## 3. `octara-release`: Clean Integration with v2

`octara-release` keeps its zero-UI-dependency Ed25519 signature verification, anti-replay sequence checking, static channel pointer protocol, and crash-safe first-frame confirmation store, and gains two clean v2 hooks:
1. **Persisted State Handoff**: `App::export_persisted()` / `App::import_persisted(RestartState)` serializes **only** explicit `cx.persisted("key", init)` entries (tagged by `state_schema`), cleanly dropping transient anonymous `cx.signal()` UI state across process restarts and release switches.
2. **Declarative UI Packets**: Signed declarative layout/theme packets compile directly into `octara_tree::Transaction` + `StyleProp` + named `Command` references — using the exact same `Tree::apply()` preflight & rejection path as compiled Rust views.

---

## 4. Zero-Baggage Refactor Execution & Cutover Plan

**Hard Rules for Execution**:
- Work in a clean isolated worktree (`branch: arch-v2`, based on `origin/init`).
- **NO v1 compatibility shims, NO `#[deprecated]` aliases, NO dead v1 files left behind.**
- Every phase MUST pass its exit gate before starting the next phase.
- Follow `AGENTS.md`: reuse workspace Cargo build cache, clean up temporary files/processes, never touch `archive/*` branches, keep inline tests in `#[cfg(test)] mod tests` at file ends, regenerate `skills/octara/` when docs/skill change.

### Phase-by-Phase Execution Checklist

| Phase | Scope | Mandatory Automated Exit Gate |
| --- | --- | --- |
| **Phase 0: Scaffolding & Layer Gate** | Update root `Cargo.toml` workspace members and `tools/check_layers.py` to v2 `ALLOWED` DAG. | `python3 tools/check_layers.py` runs cleanly. |
| **Phase 1: `octara-model` + `octara-tree`** | Implement `octara-model` (zero forbidden deps) and sparse ECS `octara-tree` (`NodeCore ≤ 48B`, all 12 `SecondaryMap` side tables, `Transaction` Preflight, `TreeDelta`, `RuntimeOp`, `StyleResolver`, `InteractionArbiter`, logical `EditorSession`, `SemanticScope`). | `cargo test -p octara-model -p octara-tree`; assert `size_of::<NodeCore>() <= 48`; `cargo clippy -p octara-model -p octara-tree --all-targets`; verify `cargo tree -p octara-tree` has zero `taffy`/`parley`/`accesskit`/`png`/`zeno`/`roxmltree`. |
| **Phase 2: `octara-runtime` + `octara`** | Implement `TypedSlab`, all-`Copy` handles (`Signal`, `ReadSignal`, `StateList`, `ReadList`, `TextBuffer`, `Watch`, `ScopeId`), 5 capability contexts (`BuildCx`/`ReadCx`/`WriteCx`/`CleanupCx`/`TaskCx`), unified Pull-on-Demand Value+Delta topological graph + modular `reactive/list/`, `VirtualClock` + `TaskScheduler`, 5-phase `Turn` engine, `HostEngine` trait, and `octara` authoring facade (`SmallVec` `El`, atomic subtree `Transaction`, value-polymorphic `Styled`, `each`/`virtual_each`). | `cargo test -p octara-runtime -p octara` (including **compile-fail doctests** proving `WRITE_IN_DERIVE`, `WRITE_IN_BUILD`, `STRICT_READ_UNTRACKED`, `SIGNAL_BORROW`, `VIEW_READS_STATE`, `REGISTER_IN_CLEANUP` fail to compile); test asserting 200-node subtree mount executes **exactly 1 `Transaction`**; grep confirms **0 `thread_local!`** in `octara-tree`/`octara-runtime`/`octara` (outside strictly scoped `TaskCx` poll bridge if used); `clippy` clean. |
| **Phase 3: `octara-pipeline` + `octara-render` + `octara-gpu` + `octara-inspect`** | Implement `NativeEngine` (Taffy, Parley, `MotionEngine`, incremental `PaintCompiler` with opt-in `check_paint_cache`, `AccessKitProjector`), modular thread-agnostic `octara-render` (`plan`/`batch`/`atlas`/`pipeline`/`encode`/`custom`/`cpu`), native event-driven `octara-gpu` (`DeviceExecutor`, zero 1ms spin-poll, background PNG), and `octara-inspect` (`octara.inspect/2`, `HeadlessDriver`, `Replay`, `SimHarness` DST). | Full deterministic headless regression suite passes (file rename + CJK IME + reorder, nested panel+list touch scroll, modal focus & overlay dismiss, virtual list reveal & offscreen selection copy, CPU vs GPU pixel parity, multi-threaded DST fuzz tests); `clippy` clean. |
| **Phase 4: `octara-host-native` + `octara-host-web`** | Implement pure-Rust `octara-host-native` (`shell.rs`, `window.rs`, `OsHost`/`OsWindow` traits, `os/macos/`, `os/ios/`, `os/desktop/`) and `octara-host-web` (Delta DOM Patcher, `DomUndoLog` rollback + `moveBefore` parking, `measure.js` batched `Float32Array` bridge, `<canvas>`/`Blob` images, controlled Web IME, optional `feature = "webgpu"`). | `cargo check --target wasm32-unknown-unknown -p octara-host-web` AND `--features webgpu` pass; verify `cargo tree --target wasm32-unknown-unknown -p octara-host-web` has **0** forbidden crates (`taffy`/`parley`/`accesskit`/`wgpu`/`swash`/`zeno`/`png`/`roxmltree`); verify **0 `#[cfg(target_os)]`** inside `octara-host-native/src/{shell.rs, window.rs, presentation.rs}`; native macOS/host tests pass. |
| **Phase 5: Full Migration, v1 Deletion & Final Audit** | Migrate `octara-kit`, `octara-cli`, `apps/chat`, `examples`, docs (`README.md`, `docs/ARCHITECTURE.md`, `docs/start-here.md`), and regenerate `skills/octara/`. **Delete all v1 crates** (`octara-primitives`, `octara-style`, `octara-text`, `octara-kernel`, `octara-shell`, `octara-web`) and obsolete compat code. | `python3 tools/check_layers.py`, `cargo test --workspace`, `cargo test -p octara-cli --doc`, `cargo clippy --workspace --all-targets`, `cargo check --target wasm32-unknown-unknown -p octara-host-web`, and final metrics comparison report (`size_of::<NodeCore>()`, `RefCell` count, subtree tx count, `.wasm` size, core `#[cfg]` count). |
