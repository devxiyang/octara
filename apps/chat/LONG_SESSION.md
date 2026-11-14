# Long Chat sessions

This qualification uses the product Chat root, Store loading, Model event channel,
paced Markdown, virtual transcript and composer. It does not substitute a cheaper
list of labels. Saved history mixes user questions, Chinese, emoji, Markdown,
code blocks, collapsed reasoning and tool calls with expandable input/output.

## Behavior and fix

Scrolling up releases tail following. Streaming elsewhere must leave the reading
position alone. Width changes reflow measured rows while preserving the first
previously laid-out visible message's position. A growing composer reduces the
transcript viewport. “Jump to latest” measures the current tail and resumes following
later growth.

The previous virtual-list anchor selection treated an unchanged height as evidence
that a row had already been measured. It skipped a visible row precisely when that
row grew, anchored a later short row and moved the text being read. Core regressions
reproduced 60 logical pixels of drift while streaming and 180 after reflow. The fix
records which **mounted** rows have actually completed layout and anchors the first
such visible row even when its height changes. Unmounting removes that record;
there is no new history-wide measurement invalidation or scroll state machine.

Crossing into the compact layout previously rebuilt the entire main pane and
resumed tail following. Chat now retains one main pane across breakpoints; only
desktop navigation is conditional. Compact conversations use the existing modal
mechanism over the still-laid-out main pane. Reading state, tool disclosures and
the composer keep their node identity. Desktop divider sizing and keyboard steps
remain the same; Escape closes the compact sidebar and Mod+L closes it before
focusing the composer.

Opening tool details also exposed `STRICT_READ_UNTRACKED`: construction read the
part signal to obtain its stable input TextBuffer. That handle is now obtained with
`untrack`; its text, tool state and output retain their existing reactive bindings.

## Reproduce

```sh
CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0 cargo test -p octara --test integration scroll::
CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0 cargo test -p octara-chat --test long_session
CARGO_INCREMENTAL=0 CARGO_PROFILE_RELEASE_DEBUG=0 cargo build --release \
  -p octara-chat --example long_session --example long_bench -p octara-cli --bin octara
```

Open the live example with an explicit **empty isolated store** and a new socket:

```sh
OCTARA_CHAT_DIR=/tmp/chat-long-store OCTARA_CHAT_MESSAGES=1000 \
  OCTARA_INSPECT=/tmp/chat-long.sock target/release/examples/long_session
```

Use `10000` for the larger history. Existing/nonempty stores and other counts are
rejected. This example never defaults to the user's saved Chat directory.

`long.start` sends a normal Chat draft; `long.stream.1`, `.10`, `.100` and
`long.chunk` deliver actual Model text events. `long.run` sends up to 1,500 chunks
at 20ms intervals; `long.pause` stops that producer. `long.finish` ends the answer,
`long.stop` uses normal Chat cancellation. These are fixture commands, not product
APIs or synthetic OS input. Each new reply resets chunk and byte counts.

The recorder observes an **already running** window. It never launches or stops
an application and never synthesizes mouse or keyboard OS events:

```sh
python3 apps/chat/observe_long_session.py --socket /tmp/chat-long.sock \
  --output /tmp/chat-long-evidence --name 01-tail \
  --invoke long.start --invoke long.stream.10 \
  --expect-tail yes --tail-id msg-1003 --capture display --settle-ms 2000
```

Scroll up using the real window, then record a baseline. Continue streaming,
resize the native window, grow the composer with a multiline UTF-8 file, and click
the product's “Jump to latest” button. Examples of subsequent observations:

```sh
python3 apps/chat/observe_long_session.py --socket /tmp/chat-long.sock \
  --output /tmp/chat-long-evidence --name 02-reading --expect-tail no --capture display
python3 apps/chat/observe_long_session.py --socket /tmp/chat-long.sock \
  --output /tmp/chat-long-evidence --name 03-offscreen --invoke long.stream.100 \
  --anchor-from 02-reading --expect-tail no --capture painted --settle-ms 2000
python3 apps/chat/observe_long_session.py --socket /tmp/chat-long.sock \
  --output /tmp/chat-long-evidence --name 04-narrow --size 820x640 \
  --anchor-from 02-reading --expect-tail no --capture display --settle-ms 2000
```

`--composer-file FILE` focuses the composer, selects its contents and replaces
them through the framework input route. Commands are semantic input; size requests
use the native-window route. The recorder obtains a fresh opaque target before
each input and requires confirmed delivery. Native scrolling/clicks are separately
performed by the operator or computer-use tool.

Each unique observation name saves snapshot JSON, all request/reply records,
capture reports and optional PNG. Failures preserve the evidence and return nonzero;
inputs are not retried. Anchor checks compare message identity and position relative
to the transcript (one logical pixel tolerance); `--anchor-mode visible` only checks
continued visibility and records the displacement. Tail checks require Jump to be
absent and the latest row at the viewport bottom. `roundtrip_ms` includes CLI startup
and socket IPC: it is **not frame time**. A zero capture wait budget permits pending
streams; the capture report states whether it settled and whether display capture
fell back to GPU painting.

## CPU and work scaling

Run the benchmark separately from builds and interactive validation:

```sh
target/release/examples/long_bench --samples 40 --output /tmp/chat-long-bench.json
```

The output path must be new. Five fresh App instances measure initial Store/JSON,
reactive data/index construction, mounting and display-list preparation. Seed
generation is excluded and the OS file cache is warm; this is not a cold-disk test.
Initial load and retained conversation data may grow with history.

Seven cases measure idle display-list preparation, visible and offscreen streamed
chunks, visible streaming into an already long answer, revealing a streamed tail,
wheel scrolling, and a 1000→820→1000 width roundtrip. The long-answer case primes
1,500 chunks through the same Model before timing; the report records its starting
and ending chunks/bytes. Each steady case has four warmups and 40 samples. Streaming
lands paced text explicitly. Reports contain raw sample bytes, p50/p95 CPU durations, closures,
node/structure/layout work, display work, visible/mounted rows and live data counts.

The gates check viewport-relative mounted rows, zero UI node churn for offscreen
chunks and bounded structural work when history grows tenfold. They do not impose
a machine-dependent millisecond threshold or infer complexity from mounted counts
alone. The CPU host uses bundled fonts; real windows use system fonts and CJK/emoji
fallback. CPU measurements exclude socket transport, native event delivery, GPU
raster/submission and physical presentation; they do not establish a display FPS.
Message virtualization bounds mounted messages, not the Markdown blocks inside a
single enormous answer. That separate growth cost is measured by the long-answer case.

## Observed results — 2026-11-14

Final-source windows ran on macOS 27.2 arm64 with system fonts, Chinese and emoji
fallback. Each used a new isolated store. CUA scrolled the actual transcript,
expanded historical tool details and activated Jump; the recorder sent Model
commands, native size requests and ordinary composer input. Both windows crossed
1000×720 → 480×720 → 1200×800, including the compact breakpoint. The 10k window also
opened its compact conversations modal and returned with Escape.

| History | Final streamed chunks / UTF-8 bytes | Reading-anchor drift | Latest bottom error |
| --- | ---: | ---: | ---: |
| 1,000 messages | 1,405 / 137,064 | 0 px | 0 px |
| 10,000 messages | 1,520 / 148,587 | 0 px | 0 px |

Transcript and composer opaque targets stayed identical across the observations;
expanded tool content stayed mounted. Five draft lines increased the composer from
31 to 102 logical pixels without moving the reading anchor. Continuous-stream
captures explicitly reported pending work. WindowServer display captures and live
GPU captures are distinguished in every report, including one covered-window
fallback in the 1k compact observation. The exploratory run and a refused recorder
key request are retained separately from these final seven passing observations
per history size. All test windows exited; their temporary application bundles were
removed, retaining the isolated stores and evidence.

The final Release benchmark ran after the windows exited and all builds finished.
All **16 cases / 39 structural checks** passed. CPU operation + display-list
preparation durations below are p50 / p95 in milliseconds, with 40 samples per
steady case:

| Case | 1k history | 10k history |
| --- | ---: | ---: |
| Visible short-answer chunk | 0.081 / 0.104 | 0.079 / 0.125 |
| Visible ~150KB-answer chunk | 0.917 / 1.153 | 0.906 / 1.064 |
| Offscreen chunk | 0.0045 / 0.0049 | 0.0053 / 0.0078 |
| Reveal streamed tail | 0.695 / 0.807 | 0.694 / 0.839 |
| Wheel scroll | 0.043 / 0.065 | 0.039 / 0.062 |
| Width roundtrip | 0.194 / 0.202 | 0.201 / 0.214 |

Initial-load p50 was 5.95 → 15.63ms with a warm OS file cache. Mounted messages
peaked at 13 for both history sizes. Offscreen chunks created no UI nodes, performed
no structural operations and ran no layout. The long-answer case mounted at most
six messages but retained 2,344 Kernel nodes, demonstrating its separate Markdown
growth cost. The same workload's cost stayed comparable as history grew tenfold;
retained data and initial-load cost still grow with history.

Local raw evidence is under `/private/tmp/octara-chat-long-20261114/`:

- `window-report.json` checks final snapshots, retained targets, tool content and drafts.
- `final-1000/evidence/` and `final-10000/evidence/` contain the final seven observations,
  screenshots and complete request/reply records, with `window.log` beside each folder.
- `long-bench-verified.json` and `bench-verified.log` are the final CPU run. Report SHA-256:
  `43a45027c643670780d1351a9a0e5684ae70be2cc1be402c34180b828a8f220f`.

`cargo test --workspace --all-targets` passed 434 tests; workspace Clippy with
`-D warnings`, formatting and whitespace checks passed. This evidence establishes
these scenarios and CPU/work boundaries, not end-to-end display FPS or GPU latency.

## Source lessons

- Exact's [scroll-away regression](https://github.com/expo/exact/blob/4c16fa59f74b1920af7dbc861a15a8105ed0ad13/issues/closed/20261022-web-list-at-its-end-reports-a-scroll-away-from-it.md)
  records an estimated-to-measured layout correction being mistaken for scrolling
  away. Measurement correction must preserve the reader's intent.
- Exact's [measurement tests](https://github.com/expo/exact/blob/4c16fa59f74b1920af7dbc861a15a8105ed0ad13/runner/src/instance/collection/tests.rs#L374)
  distinguish estimates from current-width measurements and reject stale feedback.
  Our single-process fix needs a mounted-row measurement distinction, not Exact's
  cross-host feedback protocol.
- Rune's [virtual-list anchoring](https://github.com/anomalyco/rune/blob/f1f54ce8bf485ee3a95804deace315f53a8970f6/crates/rune/src/el/virtual_list.rs#L244)
  retains row identity across changes. Its [virtual rows](https://github.com/anomalyco/rune/blob/f1f54ce8bf485ee3a95804deace315f53a8970f6/crates/rune/src/el/virtual_rows.rs#L42)
  only build mounted rows but can still scan keys; this is why time and operation
  counts accompany mounted-row counts here.

These sources informed the scenarios and small fix. Their own tests and reported
performance are not Octara validation results.
