# Chat model selection verification

Chat uses the kit Combobox with application-owned `ModelEntry.id` values. Display
names, list positions and HTTP model names do not identify a selection. The
[README](README.md#model-selection) defines send, regenerate, queue and Stop
semantics. This is an application-level configuration change; the framework's
signals, input routes and `Model` transport boundary remain the same.

The normal executable registers `network` when configured and always includes
`scripted`. The verification example registers two real `NetworkModel` adapters
against one local HTTP peer:

| Application ID | Display name | HTTP `model` |
| --- | --- | --- |
| `local-a` | 本地模型 A | `wire-a` |
| `local-b` | 本地模型 B | `wire-b` |

The example uses the ordinary Chat root, with no fixture-only UI commands. The
peer supplies deterministic responses, not hosted model inference. It accepts
only its three fixture model names and records model names, request termination,
history roles, byte counts and hashes; it does not log request text or credentials.

## Reproduce

Run the focused headless and real-HTTP tests:

```sh
CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0 \
  cargo test -p octara-chat --test model_identity --test model_picker --test network
```

Build the window example and CLI, then run against a new evidence directory:

```sh
CARGO_INCREMENTAL=0 CARGO_PROFILE_RELEASE_DEBUG=0 \
  cargo build --release -p octara-chat --example model_selection -p octara-cli --bin octara
python3 apps/chat/live_model_selection.py \
  --window target/release/examples/model_selection --cli target/release/octara \
  --output /tmp/octara-model-selection-new --timeout 20
```

The runner starts its own loopback HTTP server and native window, uses an isolated
conversation store and fresh Inspector targets, and closes its child processes.
It removes only its own socket and empty socket directory. All evidence stays in
the output directory. It needs a graphical desktop and permission for local
sockets; no model account, API key or user conversation directory is needed.

## Recorded real-window run, 2026-11-15

The release example passed on macOS with a 1000×720 window, then a native resize
to 390×640. `summary.json` records 27 delivered framework input operations and a
matched, synchronized native resize. The sequence was:

1. Send with A and receive a partial answer. Open the picker, search with Chinese
   text and select B using Down/Enter. A remains the only active HTTP request.
2. Resize the window, reopen the picker, and dismiss it with Escape. The popup
   stays inside the viewport, the header does not overlap, selection stays B and
   focus returns to the trigger.
3. Stop A. Its partial answer stays visible and the peer records `client_closed`.
   Regenerate sends the original question to B, which completes normally.
4. Send a follow-up with B, then select A and start another held response.
5. Select B and queue a question. Select A again: the queue row still shows B.
   Completing A releases the queued request to B, without changing the picker.

The five HTTP requests used **A, B, B, A, B**. Every request's history was compared
with the isolated store using ordered role/length/hash records. There were no
extra title requests: `NetworkModel` derives its titles locally. Both checked
window states reported no layout issues or framework diagnostics.

Evidence from this run remains at
`/private/tmp/octara-chat-models-20261115/live-01/`:

| Artifact | Evidence |
| --- | --- |
| `summary.json`, `routes.json` | Outcomes, fresh targets, actual input/capture/resize routes |
| `03-http-a-still-active.json`, `08-http-final.json` | Model ownership, socket cancellation and ordered history |
| `02-desktop-picker.png`, `04-narrow-picker.png` | GPU-rendered picker states at both sizes |
| `07-queued-b-selected-a.png` | Queue bound to B while A is selected |
| `08-final.png` | Final window GPU rendering at 390×640 |
| `08-display.png` | Actual WindowServer display capture at 780×1280, scale 2 |
| `model-selection-cli.jsonl`, state JSON files, `store/` | Complete operation replies and saved conversation |

The desktop computer-control tool was unavailable in this session. Text, keys
and taps used the Inspector **framework** input route in the real window;
Chinese text here was committed text, not native IME composition. This run
verifies native resizing and display capture, but does not requalify OS input,
candidate windows or IME. Earlier native-input evidence is separate from this
run. The runner reports the actual capture source and never treats a GPU fallback
as a display capture.

## Regression boundaries

- Identity tests cover duplicate labels, reordered configuration, removed IDs,
  empty configuration, preservation of drafts/files, and Stop with an unavailable
  selection. Reply and title capture the same model even when `respond` changes
  the selected signal reentrantly. A missing queued model cannot cancel the
  current request or silently retarget the queue; editing restores its original
  ID until the user explicitly chooses another model.
- UI tests cover Chinese search, keyboard highlight versus selection, composer
  draft/selection/node preservation, independently usable Stop, and queued Send
  now retaining its original model. Long names and titles are exercised at 320,
  390 and 1000 pixels in both appearances.
- Ten HTTP tests exercise real TCP/SSE, including continued data on A after
  selecting B, socket cancellation, regeneration with B, and multiple queued
  requests carrying their original models and complete history.
- Deliberate text truncation remains visible as `TEXT_TRUNCATED`. Tests permit it
  only on the explicitly constrained picker/title nodes; full model names remain
  available through semantic values and the tooltip. Other diagnostics still fail.

The reload identity test also exposed a message-ID counter resetting while named
conversation state survived. The counter now uses named state too. Configuration
reload tests stop their requests before reloading; they do not promise that an
active HTTP request survives component disposal or hot reload.

Validation logs are in `/private/tmp/octara-chat-models-20261115/`. The full
workspace run passed 509 tests; the subsequently added missing-queue regression
also passed in the final five-test identity suite. Workspace Clippy with warnings
denied, formatting and diff checks passed. `octara check` reported no findings in
all four light/dark phone/desktop cells. The Chat and kit all-targets suite and
the release verification build also passed. No hosted endpoint was contacted.
