# Native failure boundary validation

2026-11-16.

The implementation follows the narrow host-boundary policy in
[panic boundaries](panic-boundaries.md), rather than introducing local widget rollback.

## Reproduction

Build the CLI and fixture with the existing target cache:

```sh
cargo build -p octara-cli -p octara-examples --bin octara --bin window
swiftc examples/live_window.swift -o /tmp/octara-live-window
python3 -B examples/live_failure.py --output /tmp/octara-native-failure \
  --native-helper /tmp/octara-live-window
```

The helper acts only on the supplied test PID. The desktop must be unlocked and the helper
must have accessibility access. The Python harness uses framework input through the live CLI,
not native keyboard/pointer injection; only activation and closing use the OS adapter.

## Earlier locked-desktop evidence

- Core Inspector regression covers guarded and unguarded callback panic, unknown delivery,
  diagnostic reads and refusal of later input/reload/settle/capture.
- Host response regression covers exactly-once completion and preserving refused, delivered
  and unknown outcomes.
- Three native processes (handler, task and binding panic) remained inspectable, produced GPU
  painted captures before failure, and refused later work. Logs are in
  `/tmp/octara-live-failure-complete-20261116`.
- Those processes reported occluded windows and zero presented frames. A read-only session
  check confirmed the desktop was locked. This is evidence for the live protocol and offscreen
  GPU path only, **not** a pass for visible presentation or native close.
- The harness now requires nonzero presented frames and a non-occluded window before fault
  injection. At that point visible presentation and OS-close acceptance were pending desktop unlock.

## Deterministic continuation without an interactive desktop

The pending-gesture tests use the real gesture job and scheduler with a headless platform:

- A timer-triggered task panic interrupts an already-held long press. Its reply remains
  delivered, no recognition/release callback runs, no additional frame is presented, and
  explicit shutdown removes the held interaction and remaining work.
- The same failure after transport disconnect dispatches no cancellation into the failed App
  and emits no reply to the disconnected client.
- A double tap waiting for the prior click interval has delivered no new input. A task panic
  during that wait preserves refused delivery.
- Ready work failing during request preparation returns refused before input admission.
- Diagnostic snapshots advertise only inspect/issues after failure, and sending a normal
  diagnostic response does not generate a second response when its responder is dropped.

Pending gesture construction now records delivery from the job in one place, shared by initial
admission and subsequent polls. These tests establish runtime and protocol behavior; they do
not replace the deferred visible-presentation/native-close checks.

## Unlocked desktop acceptance, 2026-11-16

The user unlocked the desktop and the harness was rerun, first before teardown changes and
then against binaries rebuilt with resumable teardown. The final evidence is in
`/tmp/octara-native-failure-resumable-20261116/results.json`, with per-case CLI transcripts,
window logs and `*-before.png` GPU captures in the same directory.

| Injected failure | Presented frames before/after failure | Occluded | Native close |
| --- | --- | --- | --- |
| Handler | 2 / 2 | false | passed |
| Task | 2 / 2 | false | passed |
| Binding | 2 / 2 | false | passed |

All three passed: an ordinary missing-element refusal did not stop the App; the normal action
updated the UI; panic returned APP_FAILED with unknown delivery; only inspect/issues remained
advertised; later input, reload, settle, resize and capture were refused. Presented frame counts
stopped and presentation demand cleared. The PID-scoped accessibility helper pressed each native
close button, each failed process exited with status 1, and its Inspector socket was removed.
The handler's before-failure capture was also visually inspected.

These are actual visible-window presentation and OS-close checks, not only offscreen rendering.
Fault injection still uses framework input over the Inspector protocol, not native keyboard or
pointer injection. They cover handler/task/binding failure; resumable cleanup/context/future
Drop panic behavior is covered separately by deterministic core tests, not claimed as an
additional native UI scenario. The earlier locked-desktop limitation is now closed for these
three cases.
