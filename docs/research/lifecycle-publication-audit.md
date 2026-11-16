# Lifecycle publication audit after 67a500d

Reviewed 2026-11-16. The committed baseline passed 818 tests with 3 ignored,
workspace Clippy and formatting. This follow-up changes no runtime code.

## Findings reproduced with temporary probes

### Background completion must retire bookkeeping before arbitrary notification

`task/background.rs` catches the work closure's panic, then calls
`Oneshot::deliver`, which invokes the stored Waker. Only afterwards does the job
decrement `Shared::inflight`. A Waker panic skips that decrement and escapes the
worker loop. The job has physically completed, but `wait_background` still times
out; this also loses one of the four workers. The same notification boundary can
reach a platform wake callback through TaskWaker. Repeated failures can exhaust
the shared worker pool; that repeated case was not executed by this audit.

The probe registered a panicking Waker on a Background future, released a blocking
work closure, observed the wake, and asserted completion accounting retired.
The assertion failed, and the worker panic was observed.

Suggested design: separate delivery state commitment from its notification effect;
give in-flight accounting an unwind-safe owner; settle its bookkeeping before
external notification/destruction. Define the panic policy at the worker boundary
explicitly rather than silently converting notification failures into successful
application work. Simply catching everything without explaining the failure is not
an adequate design.

### AsyncMemo internally destroys previous results under its state borrow

`task/async_memo.rs` uses `state.update` for both refresh and result publication.
On successful replacement, `previous` is destroyed inside the closure, while the
Signal storage is still mutably borrowed. Clearing a previous Failed state can
likewise destroy its generic error inside `take_value` under that borrow (the error
variant was source-reviewed, not separately reproduced).

The probe made a successful result's Drop read the same AsyncMemo, refreshed it,
and caught the read failure. The read produced SIGNAL_BORROW. This is framework
implementation behavior, not an application explicitly choosing conflicting access
inside Signal::update.

Suggested design: retain the explicit borrowing semantics of public with/update,
but allow internal state transitions to return retired values for destruction after
the borrow and invalidation have completed. Adopt that ownership pattern in both
refresh and completion; do not expose a second state primitive or clone old values.

## Architecture assessment

Keep the current separation of reactive graph, scope ownership, task scheduling,
App execution/failure gate, resumable teardown, and window publication. They describe
different lifetimes; a universal lifecycle enum would obscure their interactions.
Keep Signal/ReadSignal, memo/watch, scoped task, channel and AsyncMemo as distinct
capabilities. This audit demonstrates gaps in their internal composition, not a need
for more public primitives, a VM, or a second executor.

The next coherent unit of work is **state commitment, bookkeeping retirement, then
external effects**, applied to Background and AsyncMemo first. Tests should cover
notification panic, stale result disposal, success/error replacement, and destructor
reentry, while retaining the existing cancellation and presentation regressions.

## Evidence and limits

Two temporary tests were run with `cargo test -p octara --lib audit_`; both failed
on the asserted contracts. Their temporary edits were removed after reproduction.
Local evidence: `/tmp/octara-core-audit-probes.log`; probe source snapshots:
`/tmp/octara-audit-task-tests.probe.rs`, `/tmp/octara-audit-async.probe.rs`.
No native OS test was run. Rune/Exact reference comparisons remain pinned to the
snapshots documented in `future-lifetime-references.md`, not latest upstream claims.

## Implemented follow-up

Both reproduced findings are now addressed. One-shot commitment returns its notification
as data; background in-flight accounting has an unwind guard and precedes external effects.
A scheduler-owned completion-failure queue transports notification/destructor panics to
normal App admission and task drains, while shutdown remains available. Native replies
share that completion boundary. The original work panic/result semantics remain distinct.

AsyncMemo now has one transition implementation which retires generic values/errors after
Signal update and invalidation. Permanent regressions cover repeated wake failures beyond
the pool's worker count, cancelled-result destructor failure, native notification failure,
unobserved failure shutdown, successful-value destructor reentry, and error refresh with
previous-value retention. The temporary probes above describe the pre-fix evidence.
