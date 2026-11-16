# Future notification, retirement and finite identities

Source review, 2026-11-16. Inspected local snapshots under
`/Users/devxiyang/code/archive/reference`: Rune `3686e94ae09545fd5f2a61e22aabfbeba1cd896f`,
Exact `08c889a9de21a30b7c3b308422fc588cf1a480d3`. These are not claimed to be latest upstream.
This review changes no runtime code and does not execute the reference projects.

## Rune: explicit requests and host notification

`crates/rune/src/runtime/cx.rs` defines its public Waker as
`Arc<dyn Fn() + Send + Sync>`, not `std::task::Waker`. `latest.rs` runs blocking jobs
on a shared pool, admits an answer only if its Attempt is still Running, then lets
the UI consume it with `take`. Its request states are Settled, Waiting and Running.
Cancelling invalidates the request; already running blocking work can finish, but
its answer no longer lands. Window closure is not implicitly state destruction:
the documentation tells applications to cancel from on_close when required.

`Latest::set_waker` replaces the callback under the mutex and drops the old callback
after the guard expires. `Inner::land` similarly invokes the callback after releasing
the lock. This directly supports Octara's lock-outside notification/destruction rule.
It is not proof that every generic destructor in Rune runs outside locks: request
replacement and cancellation also replace user-owned values under the slot lock.

Timers in `machine.rs` are explicit events carrying an Attempt and a due time;
the caller schedules `Tx::wake_at` and feeds the event back from poll. The wasm
Latest debounce uses a JS Promise timeout. Neither is a replacement for a composable
Rust Sleep Future. Rune does bridge std Future in `file_dialog.rs::Presenter`, using
a std Waker adapter to request a host turn.

The pool catches a job panic and settles it without an answer (`latest/pool.rs`,
`Slot::land`). Octara deliberately transports a Background panic back to the awaiting
task and applies App failure policy; silently turning a panic into no answer is not
appropriate for that contract.

## Exact: ownership of replies versus physical work

`runner/src/runner/waits.rs` describes deadline/admission scheduling, including frame,
update, scroll-end and idle waits. Host observations admit work, and batches can report
continuation. This is a plan/host protocol, not a general Rust Future executor.

`runner/src/runner/commit.rs::holds` and `fulfill` separate a ticket still wanted by the
runner from a reply that happens to arrive. Forgotten replies produce no commit.
`host/apple/src/executor_forget.rs` distinguishes forgetting from explicit abort:
safe reads can be cancelled, while an already sent write or begun module turn cannot
be undone. Work still running retains its physical resource accounting until it ends.
This supports separating result admission, cancellation requests and actual release.
It does not imply Octara needs Exact's HTTP lanes or ticket protocol in its core.

`kernel/src/kernel/document.rs` refuses completion after a failed document phase.
Earlier writes may remain, no rollback is promised, and the owner must poison/reset
instead of presenting the partial tree. This supports Octara's execution gate, but
does not establish resumable arbitrary Rust cleanup in Exact. Octara's teardown cursor
and consume-before-call policy address its own user callback/destructor boundary.

## Identity: borrow validation, not wrapping policy

Rune `machine.rs::Seq` wraps a u32 Attempt and explicitly documents the 2^32 limit.
Exact `kernel/src/arena.rs` validates node keys using slot index and generation,
preserves generation metadata when trimming/resetting storage, and returns
SlotSpaceExhausted for new-slot exhaustion. Its reused-slot generation wraps and
skips zero. These observations concern these particular identity types, not every
counter in either project.

Octara should retain generation-checked identities and persistent invalidation
metadata, but keep its checked exhaustion policy. Wrapping is not a proof that an
ancient handle cannot become valid again. UUIDv7 would not replace owner/lifetime
validation and is unnecessary for bounded internal slot allocation.

## Resulting design choices

- Keep std Future composition: Sleep must notify the latest polling Context Waker.
- Keep wake notification distinct from permission to run or publish a result.
- Extract callbacks/wakers under bookkeeping guards; invoke or destroy them outside.
- Make cancellation invalidate delivery immediately without promising that blocking
  work or external side effects have physically stopped.
- Keep failure gating distinct from resumable framework resource teardown.
- Preserve checked identity exhaustion; do not copy wrapping from the references.
- Do not add a VM, a second scheduler, or a public attempt/effect framework for these fixes.

Follow-up audit candidates are generic value/Future destruction under locks or RefCell
borrows, and late completion after cancellation/shutdown. These are audit candidates,
not newly demonstrated Octara defects.
