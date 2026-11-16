# Retirement and wake admission after 50121d4

Source review and temporary probes, 2026-11-16. Baseline: 824 tests passed,
3 ignored, workspace Clippy passed. No runtime changes are made in this audit.
Reference clones remain Rune `3686e94ae09545fd5f2a61e22aabfbeba1cd896f` and
Exact `08c889a9de21a30b7c3b308422fc588cf1a480d3`; not asserted latest upstream.

## 1. Close completion-failure admission together with execution

Confirmed defect: Background may physically finish after App shutdown. If destruction
of its unclaimed result panics, `Shared::complete` appends the payload to `failures`
without checking retirement. Scheduler shutdown cleared the queue earlier. App teardown
is now Complete, so repeated shutdown returns without draining that new entry. Normal
polling also returns after shutdown. `pending()` reports a permanent Update although
the background in-flight count is zero.

Probe: hold a background closure on a channel, drop its receiver, shut down the App,
then release a result with a panicking Drop. Wait for the recorded failure and repeat
shutdown. The assertion that pending is empty fails with `background completion panicked`.
This is a terminal-state/observability defect, not evidence that the result changed UI.

Design recommendation: give the completion-failure sink an explicit Open/Retired admission
boundary synchronized with queue insertion. Separate post-retirement reporting from work
that a live App must execute. Choose and document the late-panic reporting policy; do not
silently discard failures just to clear pending. Extract any rejected payload before
destroying it, and preserve shared-worker survivability. A standalone atomic check followed
by an unrelated queue lock leaves a check/insertion race.

Exact's executor retirement sets `retired`, clears queued completion state and removes its
wake callback under its state lock; completion paths check that state. Physical workers
are not joined on the UI thread and their resource charges survive until return. Borrow
the terminal admission boundary, not its HTTP executor structure or VM.

## 2. Coalesce wake requests at admission, not only at consumption

Confirmed current cost: 10,000 calls to the same TaskWaker before a drain produce 10,000
ready queue entries and 10,000 host callback invocations. `run_ready` later filters,
sorts and deduplicates. Its poll allowance does not bound these allocations or callbacks.
The probe used a synthetic TaskId to isolate wake admission, so it measures admission
mechanics rather than end-to-end UI throughput. No latency benchmark was performed and
this is not a demonstrated lost-wake correctness bug.

Rune's `live::Wake::notify` sets an atomic pending bit and calls the host only on the
false-to-true transition. `runtime/motion/live_latch.rs::take_live_wake` clears before
sampling, explicitly avoiding a lost concurrent publication. Rune's general App shared
wake path does not use that same latch: do not generalize the live-value policy to all Rune.

Exact's `executor_core.rs::wake` likewise checks `!retired && !notified`; after pumping,
it clears the flag and rechecks ready work. Its callback runs under a mutex under a narrow
enqueue-only platform contract. Octara accepts arbitrary callbacks and must retain its
outside-lock callback/destructor boundary instead of copying that implementation.

Recommended design: per-task pending notification ownership, plus a coalesced host wake
edge if warranted. Clear task notification eligibility before polling so a wake during
poll schedules the next poll. Preserve spawn order, deferred-batch fairness, generation
validation, cancellation and unwind behavior. Test races around drain/poll and stale wakers,
not only repeated wake counts. Keep every channel message: a wake is a readiness hint,
while a channel item is business data. No new public primitive is needed.

## Scope and priority

First fix terminal completion admission and its diagnostic policy. Then consider wake
coalescing as a bounded scheduler improvement, with queue/callback-count acceptance and
lost-wake tests. Do not mix this with a bounded-channel API, generic backpressure framework,
or wholesale graph rewrite. Existing Signal, memo/watch, task and scope ownership boundaries
remain appropriate; these findings concern admission into their shared runtime.

Evidence: `/tmp/octara-retirement-audit.log`, source snapshot
`/tmp/octara-retirement-audit.probe.rs`. Two temporary tests ran: one confirmed duplicate
admission counts; the terminal-state assertion failed. Probe edits were removed afterwards.
No native OS acceptance was performed.

Reference source locations:

- Rune `crates/rune/src/live.rs::Wake`,
  `crates/rune/src/runtime/motion/live_latch.rs::take_live_wake`,
  `crates/rune/src/app.rs::Shared::wake`.
- Exact `host/apple/src/executor_core.rs::Core::retire`, `wake`, and
  `host/apple/src/executor_tests.rs::retirement_aborts_held_http_discards_queued_effects_and_stops_wakes`.

## Implemented follow-up

The failure inbox now has Open and Retired states under one mutex. Retirement extracts
previous payloads for destruction outside the lock; later failures increment a saturating
count exposed through registered LATE_COMPLETION_PANIC diagnostics, never pending work.
Repeated shutdown preserves the diagnostic count. Original panic hooks remain the detailed
reporting path; no unbounded post-shutdown payload history or new execution path is added.

Wake admission now tracks each live full TaskId and its pending bit. A set holds admitted
ready IDs, supporting removal on retirement without a linear scan of all other queued
jobs. Drain still sorts by spawn order. Detached/deferred batches retain the pending bit;
polling clears it before user code, so notifications during poll schedule another turn.
Cancellation/completion remove eligibility before dropping user objects. Notifications run
outside locks and channel messages remain untouched. No separate global host-wake latch
was added: duplicate wakes of the same pending task no longer reach the host, while distinct
newly ready tasks still can.

Permanent tests cover a late failing result after shutdown (more times than worker count),
failure/retirement races and lock-free payload disposal, 10,000 wakes from four threads,
poll-time wake delivery, deferred-batch coalescing, cancellation/slot reuse, and racing
cancellation/shutdown. Previously admitted callbacks can finish after retirement; they are
not joined or run under scheduler locks. This is distinct from admitting new retired work.
