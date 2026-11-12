# Async: Tasks, Timers, Channels and Resources

Based on [async research](../research/async.md). Confirmed: a self-written single-threaded executor (the core does not depend on Tokio); typed
channels, optionally named; a subscription is a scope-owned handle.

## Tasks

- `spawn(async { .. }) -> Task`: runs on the UI thread and belongs to the current scope; cancelled when the scope is destroyed (the future and
  everything it holds are dropped). `Task::cancel` cancels immediately, `Task::detach` lets it outlive the scope. Dropping the `Task` handle does
  nothing (the scope owns it).
- A task runs like a handler: it may write signals, within its own scope (newly created signals and cleanups belong to it). After spawn, it runs to
  its first `await` before the end of the current frame.
- At the end of each frame (`settle`), woken tasks are polled until none is ready (with a limit); what they change is laid out together with the rest.

## The single loop

- Other threads cannot touch signals. They can only wake tasks: the result of `background(..)`, `Sender::send`. A wake puts the task id into the ready
  queue and calls the app's wake function (the shell sends `Wake` through the event loop proxy, then `App::poll`).
- Each batch of ready tasks is polled in spawn order, so the order in which threads finish does not change the result (in tests, `settle` first waits
  for all background work to finish, then polls).
- Stale results naturally do not land: a cancelled task is dropped together with its receiver, and a channel whose scope has been destroyed rejects
  messages (`send` returns the original value).

## Time

- The only clock is the app clock, kept by the scheduler (tasks and `sleep` both read it). Before each event and frame the shell
  calls `advance_to(time since launch)`; tests call `advance(d)`.
- `sleep(d)`, `sleep_until(t)`, `now()`, `interval(d, f)` (each firing time is computed from the start, no drift).
- The clock's resolution is a nanosecond (`Duration`'s): the clock and every due time are whole
  nanoseconds, so moving the clock to a deadline handed out reaches it exactly, and
  `next_deadline` is always after now (a deadline reached means "as soon as possible").
- `clock(period)`: the app's clock as a tracked value moving in steps of `period` (SwiftUI's
  periodic `TimelineView`), for what shows time (a spinner's step, "5 minutes ago"). Readers in a
  window with one period share one timer; it stops when the last reader's scope goes, and as a
  subscription it does not keep the app busy.
- `advance_to` handles everything that expires along the way (timers, long presses) in time order, each at its own time; `next_deadline` aggregates
  timers, long presses and inertia. Time-related code is in `app/time.rs`; gesture recognizers only supply their own deadlines (`pointer_deadline` /
  `pointer_time`).

## Background work

- `background(|| blocking work) -> impl Future`: runs on 4 shared threads (Rune's `Latest` thread pool) and is `await`ed in a task.
- Network and other I/O can be done on background threads with any library and any runtime (including Tokio), with results coming back through
  `background` or a channel; the core does not depend on them.

## Channels

- `channel::<T>() -> (Sender<T>, Receiver<T>)`: `Sender` is `Send + Clone` and can be taken to any thread; `Receiver` is on the UI thread, with
  `rx.recv().await` or `rx.on(|v| ..)` (a subscription: owned by the scope, stops when the scope is destroyed, after which the sender gets its value back).
- `rx.named("token")` (`T: FromStr`): tests and agents feed text by name (`App::send(name, payload)`, `Event::Send`); the name itself counts as a
  sender, and the channel stays open while it is registered.

## Resources

- `resource(key, |k| async { .. }) -> Resource<T, E>`: loads now, and reloads when `key()` changes or on `refetch()`; the same key does not load again.
  A new load cancels the old one, and an old answer never overwrites a new answer.
- State is signals: `loading()`, `value()` (keeps the last value during a reload and after an error), `error()`, `get() -> Load`.

## Tests and agents

| Call | Effect |
| --- | --- |
| `poll()` | Run tasks woken by other threads (the shell calls it on receiving a wake) |
| `advance(d)` / `advance_to(t)` | Advance the clock; whatever expires along the way is handled in time order |
| `settle()` | Without moving the clock: wait for background work to finish (real time, with a limit) and run all ready tasks; returns the work still pending |
| `run_until_idle(limit)` | Repeatedly settle and advance to the next deadline until there are no tasks (subscriptions do not count); past the limit, returns the remaining work |
| `pending()` | Tasks, subscriptions, background work: kind, label (`task`, `resource`, `interval`, `channel "token"`), scope path, source location, timer expiry time |

## Later

- A global deduplicating cache for resources (shared by key, GPUI's resource cache); show the placeholder only after loading for 200 ms (GPUI).
- Record at the single loop (virtual time, source, content) for replay, including LLM token streams.
- Forbid real time in tests (GPUI's `dispatch_after` panics in tests); seeded random scheduling.
- Backpressure and coalescing for streams (Exact: 4096 items / 64 MiB).
