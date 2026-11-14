# Streaming interaction

This slice checks ordinary Chat interaction while model data arrives: native Chinese
composition, text selection, scrolling and Stop. It uses the unchanged product root
and Model channel. Long paragraphs, open code fences and aggregated network bursts
are separate workloads, because they have different costs.

## Scope and source lessons

The goal is useful interaction under representative load. Workload sizes are test
choices, not framework capacity limits or a promise of constant cost for any document.

- Exact's [executor pump](https://github.com/expo/exact/blob/4c16fa59f74b1920af7dbc861a15a8105ed0ad13/host/executor/src/pump.rs#L25)
  limits a wake to 32 completions or 4ms, checking between transactions. It does not
  preempt a large synchronous transaction.
- Exact's [mapper failure and fix](https://github.com/expo/exact/blob/4c16fa59f74b1920af7dbc861a15a8105ed0ad13/issues/closed/20261023-a-stream-mapper-cannot-yield.md#L12)
  use cooperative continuations, FIFO delivery, cancellation and bounded waiting.
  Token deltas must retain order; replacing them with only the latest delta loses text.
- Rune's [Markdown cache](https://github.com/anomalyco/rune/blob/f1f54ce8bf485ee3a95804deace315f53a8970f6/crates/rune-kit/src/markdown/cache.rs#L56)
  reuses completed blocks only after proving an append. A long paragraph or open
  code fence can still be one large unfinished block.
- Rune's [syntax handling](https://github.com/anomalyco/rune/blob/f1f54ce8bf485ee3a95804deace315f53a8970f6/crates/rune-kit/src/syntax.rs#L124)
  incrementally extends supported lexers and limits coloring to 128KiB / 32,768
  ranges. Those are coloring limits, not body-text capacity limits.
- Exact also has a [4MiB document stress app](https://github.com/expo/exact/blob/4c16fa59f74b1920af7dbc861a15a8105ed0ad13/apps/markdown-stress/README.md#L24),
  with paging and explicit limitations. It does not establish instant full Chat
  rendering of a 4MiB response.

These are pinned source observations, not tests executed against those frameworks.
Octara keeps Rust + Signals and applies small corrections at existing boundaries.

## Changes

Code blocks previously assumed every update appended to the last line: rewriting
an earlier line left its old text on screen. The renderer now proves append before
reusing preceding lines; rewrites and truncations update all affected lines. The
append path splits only the previous final line and the new suffix. Prefix checks,
source copying and parsing of a large unfinished block still have costs.

A channel listener previously consumed all queued values in one Future poll before
another ready cancellation could run. `Receiver::on` now cooperates after 64
callbacks while preserving FIFO. The scheduler can poll it again within the same
frame; this is task fairness, not an OS input deadline guarantee.

Switching Reduce Motion on with an existing paced backlog now lands the text at its
next pacing tick even if no new text arrives.

An attempted 16KiB / 128-newline pacing budget was withdrawn. The initial mount
copies existing text, so that budget did not protect the first large reply. A 4MiB
diagnostic still mounted 68,201 nodes and took about 0.38s before and after. Those
measurements did not justify another initialization API or a virtualization system.
The raw attempted experiment is retained separately from final validation.

## Reproduce

```sh
CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0 cargo test -p octara --test integration
CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0 cargo test -p octara-kit --test integration
CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0 cargo test -p octara-chat --test responsiveness
CARGO_INCREMENTAL=0 CARGO_PROFILE_RELEASE_DEBUG=0 cargo build --release \
  -p octara-chat --example long_session --example stream_bench -p octara-cli --bin octara
target/release/examples/stream_bench --samples 20 --output /tmp/chat-stream-cpu.json
```

`stream_bench` defaults to long-tail increments and a 64KiB code burst. `--overload`
adds the optional 4MiB diagnostic. It measures enqueue, App polling, explicit paced
landing and display-list preparation separately, with raw samples and quantiles.
It does not measure a native event-loop deadline, GPU execution or display FPS.
Quantiles include sample counts; 20 samples do not establish a reliable tail-latency
estimate. The optional overload case has at most three samples.

Use a new empty isolated store to open `long_session` as described in
[LONG_SESSION.md](LONG_SESSION.md). Before a new reply, `long.scene.mixed`,
`long.scene.paragraph` or `long.scene.code` selects the payload. Then
`long.bytes.64` / `.256` / `.1024` / `.4096` / `.16384` selects chunk size.
At 256 bytes, `long.burst.32` / `.256` aggregate 8KiB / 64KiB for fixed-size
paragraph/code payloads. Mixed Markdown uses its existing variable chunks.

Explicit `long.rate.50` / `.200` / `.1000` followed by `long.run` uses a bounded
wall-clock producer thread and the actual Model Sender. The worker never owns Chat
or UI objects, and the UI does not join it. `long.pause`, cancellation and scope
cleanup retire it. Production is capped at 30 seconds, 8MiB and 8,192 chunks per
reply to bound the fixture's resources. These are fixture limits. Without an explicit
rate, `long.run` retains its earlier app-clock behavior; it is not the native high-rate
test. `long.run.short` uses the same wall-clock worker with a five-second cap; set
an explicit rate first. Pause is asynchronous: check `workers == 0` before asserting
production has stopped. Rate is a producer target, not a measured window frame rate.

`long.summary` reports sent chunks/bytes, consumed source bytes, answer status,
draft, producer clock and active workers. `long.late` actually attempts text and Done
after cancellation, with separate attempted/accepted counters. Already accepted paced
text can finish appearing after Stop; that must not be mistaken for accepting new
late model events.

## Real-window measurements

Set `OCTARA_TRACE_INPUT=1` when starting the window and direct stderr to a log file.
The opt-in trace records no input content and retains only the oldest pending input
with coalesced dispatch counts.

- `octara.update_trace/1` times Shell `advance_to` and wake `poll`: task, timer,
  reactive and layout work within each call. It is app-wide, not attributed to a window.
- `octara.frame_trace/1` times display-list preparation, surface acquisition,
  renderer preparation/encoding/submission, native placement and the present call.
- Input starts at Shell dispatch or the owned native text callback. Native callback
  queue time is recorded; hardware-to-dispatch queue time is not measured. Coalesced
  counts include nested native edits and are not physical keystroke counts.

The timings are main-thread elapsed time and include blocking waits. Present returning
does not establish GPU completion or physical display; those fields are null. Update,
input-handler and frame durations can overlap and must not be added. Trace logging
can affect later samples, so native results describe a trace-enabled diagnostic run.

Use read-only snapshot `time_ms` values to select a steady workload interval, excluding
application startup and unrelated phases, then summarize the log offline:

```sh
python3 apps/chat/analyze_stream_trace.py /tmp/chat-window.log \
  --from-ms 10000 --until-ms 15000 --require-visible --output /tmp/chat-stream-trace.json
```

Frame visibility/occlusion filters are explicit. Unknown visibility is counted, not
silently treated as proven foreground. App-wide update records are selected by clock
only and remain separate from window-frame statistics. The nearest update before and
after each boundary is retained separately for review; a logical clock plus elapsed
time cannot establish exact wall-clock overlap. JSON requests and CLI startup
durations are not used as frame or input latency.

## Observed results, 2026-11-14

The release headless run used 1,000 saved messages and 20 samples per default case.
All durations below cover enqueue, polling, optional paced landing and display-list
preparation together; setup is excluded. They are not native input latency.

| Workload | Total p50 | Total p95 |
| --- | ---: | ---: |
| Mixed Markdown tail | 0.188ms | 0.207ms |
| Single paragraph, 64KiB prime then 1KiB increments | 15.582ms | 17.222ms |
| Open code fence, 64KiB prime then 1KiB increments | 0.886ms | 0.959ms |
| Fresh 64KiB code burst, one poll without paced landing | 7.548ms | 7.758ms |

Mixed chunks have variable size despite the configured 1KiB size, so that row is not
a byte-for-byte comparison with paragraph/code. These results do not demonstrate a
significant general speedup; the paragraph's unfinished tail remains a measurable cost.

The macOS display run used the same product root, actual Model Sender, system Pinyin
and native input through CUA. Inspector observations and display captures were saved
after actions, not substituted for OS input.

- A `ni` preedit survived a new production run. Subsequent Chinese commit produced
  `你好`; Shift+Left selected `好` (UTF-8 `[3,6]`, UTF-16 `[1,2]`). The early five-second
  runs alone did not prove every new key overlapped production.
- The final run removed that ambiguity: a 30-second-capped, 50Hz producer sent 64-byte
  paragraph chunks while native `nihao`, Space, Shift+Left and Command+Period were
  delivered. It stopped at 375 chunks / 24,000 bytes, before the cap. Readback confirmed
  `Stopped`, draft `你好`, selection of `好`, zero workers, and two late event attempts
  with zero accepted. The input source remained Pinyin before closing the test app.
- On an open code fence, real pointer dragging selected `et item_000508 = 5`.
  After a real wheel scroll and a further 2,048-byte append, that selection and the
  reading anchor's relative Y stayed unchanged (0px delta); Jump remained visible.
  Double-click attempts without a recorded selection were not counted as passing.
- A separate 200Hz, five-second run sent 64,000 bytes and was stopped successfully,
  but its producer had already ended. It proves the resulting Stop/late-event state,
  not interruption of an active 200Hz producer.

For the final run's observation interval (`app_clock_ms` 1,890,588–1,914,334),
512 foreground frames had total p95 10.398ms / max 18.042ms; 1,413 `advance_to`
calls had p95 3.648ms / max 7.568ms. Six input-bearing frames recorded five native
compositions and one commit; dispatch to present-call return had max 24.616ms.
This interval includes preparation and post-Stop inspection. The input sample is
small, and no trace is attributable specifically to Stop, so there is no Stop
latency or physical-display claim. Adjacent update records were retained separately.

The wider diagnostics exposed limits, which remain open:

- An early 30-second single-paragraph run unintentionally grew to 416,768 bytes
  during tool waits. Native key delivery timed out; no new composition/commit was
  recorded. An app update took 10.458 seconds. This is a failed pressure observation,
  not a passing Chinese-input test, and it is not attributed solely to the driver.
- Mixed Markdown later reached 125,748 / 223,659 bytes, with app updates as long as
  277.535 / 669.289ms. The failed Stop attempt there left the answer Streaming;
  a later successful small-paragraph run does not erase that observation.
- A code-window frame took 1,002.721ms, almost entirely in surface acquisition
  (1,001.689ms). Low p95 drawing values do not establish smooth physical presentation.

The raw evidence lives in `/private/tmp/octara-stream-response-20261114/`: CPU reports
`baseline.json`, `after.json` (withdrawn pacing experiment), `final-cpu.json`; native
logs and named snapshots/captures under `native/` and `normal/`. Temporary launch
bundles are removed after exit. Reproduction should create a new isolated directory.

The final workspace test run passed 446 tests across 35 suites, including the new
pointer-selection regression. The automated checks cover code rewrite/truncation, FIFO and
cancellation cooperation, Reduce Motion backlog, Unicode selection, Stop with queued
text, stale events, bounded input and producer cancellation. They complement rather
than replace the native evidence above.
