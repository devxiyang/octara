# Retained targets in a real window

`targets` is a visible fixture for Inspector requests that carry a target observed earlier.
The controls use ordinary Octara components, named state, `show_else`, and window APIs.
`live_targets.py` connects to an already running fixture through the built CLI; it does not
launch or close the application, inject OS input, or retry failed requests.

Build the binaries, then launch `targets` with a fresh private Inspector socket using the
normal desktop/computer-use workflow:

```sh
cargo build -p octara-examples --bin targets -p octara-cli
OCTARA_INSPECT=/tmp/octara-targets.sock target/debug/targets
```

The main title is **Octara Target Validation**; the child always uses **Octara Target Child**.
Start with zero hits, zero generation/value updates, and no child. Connect separately:

```sh
python3 examples/live_targets.py \
  --socket /tmp/octara-targets.sock \
  --octara target/debug/octara \
  --output /tmp/octara-targets-evidence-new
```

The output directory must be new. Each CLI invocation receives exactly one JSON request through
`run --connect`; its request, raw stdout/stderr, exit status, and parsed reply remain in a
numbered JSON file and `transcript.jsonl`. `summary.json` records success or the first failure.
GPU PNGs and their capture reports remain alongside the transcript. A failed/lost reply stops
the sequence without replaying input; inspect the application before deciding how to recover.
The fixture stays open for independent computer-use inspection.

| Control or label | Meaning |
| --- | --- |
| `target-action` | Adds one shared hit; the same test id exists separately in each window. |
| `targets-hits`, `targets-generation`, `targets-updates` | Readable shared counters in both windows. |
| `targets-replace` | Increments generation and switches `show_else` branches, destroying the old node. |
| `targets-update` | Changes the existing target's label and left margin without replacing it. |
| `targets-open`, `targets-close` | Opens/closes the child while keeping the main window alive. |
| `targets-child-close` | Closes the child from inside it. |
| `targets-child`, `targets-child-openings`, `targets-error` | Main-window child status, successful opens, and an open failure. |
| Inspector `reload` | Calls the real `App::hot_reload`; named counters survive rebuilding all windows. |

The script checks the following sequence:

1. A freshly observed node target works. Updating text/geometry keeps that token usable.
2. Same-id replacement changes the token. The old token is refused for `tap`, `double_tap`,
   `long_press`, `focus`, `hover`, and `type`, with no extra hits; a fresh token works.
3. A window-target-guarded `reload` preserves the counters, retires the earlier node and window
   tokens, and allows a newly observed target to work.
4. A child is addressed by its own window number and target. Closing/reopening its same title
   retires both its window and node tokens. Old-number/old-token and new-number/old-token
   requests are refused without affecting the shared counter; the new child target works.
5. An id-only request still works for compatibility. The expected final state is **7 hits,
   generation 1, value updates 1, child openings 2**, with the reopened child still present.

Refusals must be `STALE_TARGET`, with both `Reply.delivery` and `live.delivery` equal to
`refused`. Successful input must report `delivered`; routes distinguish framework events from
semantic operations. Key checkpoints require settled GPU captures and no layout issues or
diagnostics. These are framework inputs delivered to real native windows, not proof of OS
keyboard/mouse injection; computer-use screenshots and actions provide that separate evidence.

`Snapshot.target`, `WindowInfo.target`, and `Node.target` are opaque identity guards. Copy them
unchanged into a request's optional `target`; do not parse or manufacture them. They detect
replacement, reload, and window retirement, not changes in a node's value or geometry. They are
not permissions, authentication, state versions, or a promise that every later event will finish.
Legacy id-only requests intentionally resolve the current element and have no stale-target guard.

A refusal means no input was delivered for that request. A failure after focus, pointer movement,
or another event was already delivered must report `delivered`; it cannot be treated as safe to
retry. This script covers refusal before delivery and successful fresh input. Mid-gesture
retirement and other failures after partial delivery require their own evidence, and transport
loss still leaves the outcome uncertain.

## Recorded run, 2026-11-14

The fixture was launched through computer use as a native macOS app, with Inspector PID
`48189`. The script completed all **73 requests**, saved six settled GPU captures, and found
no layout issues or diagnostics at those checkpoints. Its final state was 7 hits, generation 1,
one value update and two child openings. Evidence is retained locally at
`/private/tmp/octara-targets-20261114-4vp8z4cj/evidence-01`.

The same-ID rejection steps `017`–`028` returned `STALE_TARGET` and `delivery: "refused"`
without increasing the counter. Reload step `031` used the actual application hot-reload
entry; steps `033`–`036` rejected its prior node and window targets. The first child's window
number `4294967298` was replaced by `12884901890`; steps `057`–`064` rejected both the closed
number and an old token attached to the newly opened number. Fresh references worked after
each rejection. An id-only request at `069` remained compatible.

Computer use then independently observed both windows, closed the child through its visible
control, and reopened the same title through the main window's visible control. That third
child had window number `21474836482`. The old target was again refused without changing
the 7-hit counter; a freshly observed target succeeded and the visible counter became 8.
The corresponding records are `cua-before-child-close.json`,
`cua-closed-target-refusal.json` and `cua-reopened-loop.json` in the parent directory. The
saved `cua-reopened-display.png` and report contain actual WindowServer pixels
(`renderer: "display"`, `pixels: "display"`, 1280×720 at 2×), with no painted fallback.

The first computer-use launch reported a ScreenCaptureKit stream error even though it had
started the app. Inspector confirmed an active, visible, unoccluded real window; rebinding
computer use succeeded and supplied the independent screenshots and control actions above.
The tool error is not counted as a passing screenshot.

Core regressions separately cover Focus callbacks and Key-to-Text fallback replacing a node
before text, and guarded gesture tests cover replacement after partial input. Those checks
are not claimed as additional OS-input scenarios in this run. The validation app was quit
through computer use, its process exit was checked, and only its stopped temporary bundle and
socket were removed; the JSON and PNG evidence was retained.

## Source comparison

Exact's native input keeps an entry, nonce and handle, then rechecks them and focus after
obtaining focus before sending text. Its held release validates the original owner rather
than moving focus back. This supports the same local continuation check used here.
[Exact native input, fixed revision](https://github.com/expo/exact/blob/08c889a9de21a30b7c3b308422fc588cf1a480d3/host/apple/Sources/ExactKit/NativeModule.swift#L1031).

Exact's arena generations and kernel incarnation provide useful allocation boundaries, but
its agent tags do not by themselves constitute an expected-target check across host
replacement. Octara's opaque reference also includes the app instance namespace.
[Exact arena](https://github.com/expo/exact/blob/08c889a9de21a30b7c3b308422fc588cf1a480d3/kernel/src/arena.rs#L262),
[Exact agent boundary](https://github.com/expo/exact/blob/08c889a9de21a30b7c3b308422fc588cf1a480d3/runner/src/agent.rs#L86).
