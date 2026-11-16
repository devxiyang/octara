# Collection primitive consolidation: implementation and validation

2026-11-16. Collection consolidation committed as `af6fa20`; evidence below was collected before that commit.

## Implemented boundary

- `each` and `virtual_each` accept a closure returning `Vec<(K, T)>` or `ReadList<K, T>`.
  `IntoRows` is a sealed conversion trait; no writable snapshot adapter is exposed.
- Both renderers use the private `Rows` owner for construction, current row input, keyed
  retention and retirement. Virtualization retains its weighted index and residency policy.
- A row's `ReadSignal<T>` lazily resolves the current source and key. It neither waits for an
  effect to copy data nor flushes UI work during reads. Missing keys fail explicitly.
- Snapshot duplicates retire the region at structural propagation; a valid later snapshot
  mounts fresh targets. Incremental duplicate insertions remain rejected before mutation.
- `Derived` is removed. Incremental operators return `ReadList`, with scope-owned producer
  registrations and tracked lifetime dependencies. Escaped caches cannot keep producers running.
- Structural bindings precede properties and effects, including work queued during a batch.
  Newly queued effects wait for the next feedback round; finite fanout does not consume a
  runaway budget merely because structural/property work preempts it.
- Chat renders keyed parts directly. Part identity, enum branch and captured text-resource
  lifetime are separate; replacing a buffer keeps its reasoning/tool disclosure open.
- Tabs/Segmented render keyed options directly; the mirror list, watch, synthetic order and
  sorting stage are deleted. The prelude now enumerates its exports explicitly.

## Deterministic coverage

`crates/octara/tests/integration/collection_rows.rs` covers both render modes: source switching
with retained targets, read-after-write in the same handler, reorder/removal/reinsertion,
invalid snapshot retirement/recovery, transient delta keys, absent-key reads, structural
preemption and streaming without snapshot recomputation or row rebuilding.

`list_runtime.rs` checks that appending to 1,000 rows performs only the new row's content read;
1,100 independent effects settle without a false runaway; a keyed scope/watch feedback loop
is bounded, stays pending and can recover. Filter/group reads respect current membership.
`list_lifetime.rs` checks producer retirement, cached reads, cross-scope join dependencies,
final release outside App entry and release while another App is current.

Chat tests cover stable IDs across reorder/deletion/save/load, same-key variant and buffer
replacement, disclosure retention, old-resource detachment, status-only updates and streaming.
Existing 1k/10k long-session and responsiveness tests remain green.

Verification:

- `cargo test --workspace`: 707 passed, 3 existing ignored tests, no failures.
- `cargo clippy --workspace --all-targets -- -D warnings`: passed.
- After adding storage-format isolation, Chat lib/chat/lifecycle/long_session: 42 passed,
  including the new test proving old files survive new-format saves and deletes.
- `cargo clippy -p octara-chat --all-targets -- -D warnings`, formatting and diff checks passed.
- `octara check -p octara-chat`: clean at light/dark and phone/desktop sizes.
- Regenerated the shipped Octara skill after updating its source and start-here guide.

## Real macOS windows

Used local Inspector connections to live native windows and captured their GPU-rendered frames.
Inputs below use the framework event route, not native OS IME qualification.

- Interaction showcase: enter `保留草稿`, switch Draft -> Preview -> Draft, select Full in the
  Segmented control, resize to 700 x 760. Draft, editor target, tab targets and option target
  remain unchanged; selected/checked states are correct; issues are empty.
- Isolated Chat fixture with 1,000 saved messages: send 100 controlled chunks, enter a Chinese
  draft, start a bounded producer, resize to 820 x 720, stop. The draft remains, Stop retires,
  Stopped is visible, and issues are empty. Screenshots were visually inspected.

Session artifacts: `/tmp/octara-collection-live-9UW4KI/` (JSONL requests, replies, isolated store,
`kit.png`, `chat.png`). These are temporary local evidence, not checked-in fixtures.

## Storage and remaining boundaries

Stable part IDs and the allocator are stored in `<thread>.v2.json`. Old positional
`<thread>.json` files have no compatibility loader and are left untouched by both saves and
deletes. Validation used a temporary directory, not the user's normal Chat store.

The follow-up [bounded change-log protocol](change-log.md) implements log resynchronization.
The broader collection design still records future work such as virtual interaction residency pins. This implementation does not replace the incremental
operator evaluator or add a VM, public Region object, or general store language. Macro support
and representation types remain publicly reachable at explicit module paths where Rust API
boundaries require them; removing them from the prelude is not making those types private.
