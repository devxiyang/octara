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

## Evidence so far

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
  injection. Visible presentation and OS-close acceptance remain pending desktop unlock.
