# Research: Async, Resources and Subscriptions

2026-11. Compares Rune (source and `research/09`), GPUI (Zed `main` 985bbf4 source; the `scheduler` crate is not in the sparse clone,
so related conclusions are inferred), Expo's Exact (docs read) and Leptos, SolidJS, SwiftUI, Compose, Flutter, Elm, Iced (compiled from memory of the official docs).

## Tasks

| | Rune | GPUI | Exact |
| --- | --- | --- | --- |
| Model | No async runtime: `Latest<T>` runs blocking closures on a pool of at most 4 threads; `ImageCache` decode thread pool (`latest.rs:25-31`, `image.rs:157`) | Foreground executor (`!Send`, main thread) and background executor (thread pool); `Task<T>` (`executor.rs:19-50`) | The UI waits for nothing; I/O happens in data modules (TS or Rust), and the runner lands results as commits |
| Back to UI | Result goes into a mutex slot and wakes up; taken on the main thread in `poll`; `AppHandle::update(|s, tx| ..)` is a cross-thread closure queue (`app.rs:748-769`) | Foreground task `await`s a background task, then `WeakEntity::update`; notifications go through an effect queue, flushed once when the outermost `update` ends (`app.rs:2138`) | Landing is one commit |
| Cancellation | Cancelled when the owning object is dropped (`Latest`); no view scope, so the app must `cancel` itself when closing a window | Dropping the `Task` cancels; `detach()` explicitly lets go; closures hold weak references, and `update` returns `Err` after the entity is released | Aborted when replaced by a new request; only the root component can declare resources, with an app-wide lifetime |
| Stale results | Fence tokens `Attempt` / `Seq`: results of old requests change nothing (`machine.rs:5-9`) | Exits when the weak reference fails to upgrade | "The current request owns its answer"; an old reply cannot overwrite a new request |

## Resources (async loading)

| | Rune | GPUI | Exact |
| --- | --- | --- | --- |
| State | Only `is_pending()`; images have pending / ready / error | `Option<Output>`, errors live inside `Output`; completes only once, no refetch | `pending(r)`, `failed(r)`; keeps the previous value on failure; `refresh` |
| Refetch by key | `request_if_changed(&key, ..)`: same key does not restart; `debounce` | None | Arguments are the request identity; same arguments are deduplicated |
| Dedup cache | Images by content hash | Resource cache by `(TypeId, hash)`, comparing hashes only | One request per resource and arguments |
| Loading UI | — | Placeholder shown only after 200 ms of loading (`img.rs:28`), fallback on failure | Views read `pending` / `failed` |

Native frameworks: SwiftUI's `.task(id:)` and Compose's `LaunchedEffect(keys)` are coroutines that "live with the view and are cancelled and restarted when the key changes"; Leptos's
`spawn_local` is not cancelled with the scope (a recognized pitfall); Flutter's `FutureBuilder` only ignores stale results and does not cancel.

## Subscriptions and timers

- **Rune**: `tx.wake_after(d)` / `wake_at(due)`, coalesced within a frame, with no timer threads; timers are events carrying an `Attempt` (`machine.rs`).
  There are no subscription types for streams or file watching; streams are pushed by app threads through `AppHandle::update`.
- **GPUI**: `executor.timer(d)` goes through the scheduler clock (controllable in tests); no first-class interval, use a task loop; subscriptions are `#[must_use]` handles, dropping unsubscribes,
  and a subscription dropped midway through a callback no longer fires (`subscription.rs:29-40`).
- **Exact**: `task every(ms) / after(ms)` with a `when` condition; streams are `fetch(url, { exactStream })`, where each SSE / WebSocket message becomes a new answer,
  bounded (4096 messages / 64 MiB) and coalescing messages that arrive too fast.
- **Elm / Iced**: subscriptions are data, which the runtime compares by identity (hash) each time; those that disappear are cancelled.

## Determinism and visibility

- **Exact**: the virtual clock is still between steps; `clock +N` (advance and fire timers), `clock settle` (land in-flight work and run animations to completion), `clock data`
  (land data only, without moving the clock); long connections and streams do not take part in settle; `fail fetch "<prefix>"` fakes failures; `state` returns a `pending` list, and the `clock`
  reply lists in-flight requests. No network record and replay.
- **GPUI**: `TestDispatcher` with seeded random scheduling, `advance_clock`, `run_until_parked`; tests by default forbid parking with unfinished work; `dispatch_after`
  panics directly in tests, forcing all timing onto the virtual clock. No public pending-task list.
- **Rune**: headless running and a virtual clock are first-class, but `Latest` uses real threads and `Instant::now()` and is not controlled by the virtual clock; `research/09` plans
  "settle with a deadline and list remaining work" and "release recorded completions in order", not yet implemented.

## Conclusions

1. **Tasks belong to scopes**: a scope owns its tasks and cancels them when destroyed; `detach` must be explicit (GPUI's `Task`, the intent of Rune `research/09`; avoids Leptos's pitfall).
2. **Results have a single return path**: across threads, results can only be put into one inbox, which the UI thread drains at a frame boundary and writes into signals as one batch (Rune's
   `AppHandle::update`, GPUI's effect queue).
3. **Every result carries a generation**: results whose scope is destroyed or whose generation is stale are dropped (Rune's `Attempt`, Exact's "the current request owns the answer").
4. **Resource = refetch by key + state signals**: loading / ready / failed, keeping the previous value on failure and refetch; the same key does not restart (Rune `request_if_changed`, Exact).
5. **All timing goes through the virtual clock**: `sleep`, timers and debounce inside tasks all use the app clock, and `next_deadline` covers them; no real time is allowed in tests (GPUI).
6. **Settle comes in three kinds**: land only results that have arrived, advance the clock, run to idle with a limit and list remaining work; streams and long connections do not take part (Exact, Rune's plan).
7. **Pending work is visible**: tasks, timers, resources and subscriptions are all registered on the scope tree (location, key, generation, deadline) and readable by agents and on test failure (none of the three does this fully).
8. **Record and replay**: all results pass through the same inbox, where recording (virtual time, source, content) enables replay, including LLM token streams (a gap in Exact).
