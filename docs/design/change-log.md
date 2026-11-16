# Bounded change history and snapshot recovery

Implemented 2026-11-16 after the collection consolidation in `af6fa20`.

The private `reactive::log` module is shared by incremental collections and text. History is
an acceleration cache; the collection's keyed state or TextBuffer's string remains authoritative.
The former slowest-reader trimming rule was insufficient: one stalled reader could retain an
unbounded stream of edits.

## Protocol

Each log keeps at most 4,096 entries and 1 MiB of accounted event/payload storage. Text accounts
for owned String capacity, so one oversized append cannot stay in the history indefinitely.
Generic collection keys have no universal deep-size operation: their retained count is bounded,
not an advertised byte bound on arbitrary user-owned key graphs. Authoritative model data,
allocator capacity and short-lived replay/snapshot copies are outside this history budget.

Readers receive one of two private results:

- `Delta(events)`: every event since the reader's cursor is present and may be replayed in order.
- `Reset`: some required history was discarded. Advance the cursor to the current revision and
  read the current snapshot instead. Never replay the retained tail against stale state.

Readers consume synchronously on the UI thread, so there is no write between advancing the
cursor and reading the snapshot. Active readers continue to receive deltas even when another
reader stalls. Consumed entries and dead reader registrations are reclaimed; creating and
retiring subscriptions without writes does not accumulate weak cursor handles.

No new public primitive, user-managed version or configurable tuning interface is introduced.
The limits are private storage policy, not a performance target for applications.

## List recovery

Both `each` and `virtual_each` reuse their existing full-snapshot reconciliation path. Reconcile
by business key: retain surviving row scopes and targets, update their current inputs, retire
absent rows, and build only new keys. Virtual lists also rebuild their weighted index from the
current data; ordinary viewport residency still determines which rows are mounted.

The identity contract is published membership. An intermediate absence that was never accepted
by the row coordinator does not promise a remount. Once a removal has been accepted, a later
reappearance creates a fresh row and target. A logically different entity requires a different
business key; edit history is not a hidden incarnation/tombstone protocol.

This keeps a history gap from indiscriminately resetting focused controls or local row state.
Available deltas still apply structural operations directly, without scanning the full source.

## Text recovery

The text binding reads the authoritative string. When the existing kernel text is an unchanged
prefix, it appends only the missing suffix, preserving selection in that prefix. Otherwise it
replaces the text. Existing nodes stay mounted; later edits resume the usual incremental path.

Paced text treats a history gap as an edit: it lands the current authoritative text immediately,
using the same prefix check to preserve selection when possible. It must not assume that a gap
contained only appends: a dropped splice can invalidate both byte offsets and pacing's shown
prefix. Future bursts resume normal pacing, including UTF-8 boundary handling and motion cleanup.

## Verification

- Shared log tests: a stalled reader versus an active reader across three history windows;
  entry boundary; aggregate byte budget and one oversized payload; resume after reset; weak
  reader reclamation without writes.
- Collection tests in both render modes: overflow after a same-key replacement, final order
  and values, transient keys never mounted, surviving targets retained, published removal and
  fresh reinsertion, and subsequent incremental updates.
- Text tests: 5,000 Unicode appends with selection retained, a dropped splice followed by
  appends, two readers, a payload exceeding 1 MiB, and edits after recovery.
- Pacing tests: reset with an existing backlog, missed Unicode splice, unchanged-prefix
  selection, and later bursts pacing normally.

These are deterministic runtime/rendering tests. This follow-up does not claim a new native OS
input or IME qualification run.

Validation results: `cargo test --workspace` passed 718 tests with 3 existing ignored tests;
`cargo clippy --workspace --all-targets -- -D warnings`, formatting and diff checks passed.
The final additional row-order assertion was also rerun successfully in the targeted recovery test.
