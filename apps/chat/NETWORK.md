# HTTP streaming in Chat

The normal Chat executable can use a text-only Chat Completions endpoint. `Model`
and the existing `Sender<Event>` remain its UI boundary: a reusable reqwest client
and Tokio runtime read HTTP outside the UI thread. The core framework needs no
HTTP runtime or provider-specific model abstraction.

## Configuration

Set these variables before starting `octara-chat`:

| Variable | Meaning |
| --- | --- |
| `OCTARA_CHAT_ENDPOINT` | Complete URL, including `/v1/chat/completions` or the provider's equivalent path |
| `OCTARA_CHAT_MODEL` | Model identifier accepted by that server |
| `OCTARA_CHAT_API_KEY` | Optional bearer token; kept outside conversation storage and diagnostics |
| `OCTARA_CHAT_TIMEOUT_SECS` | Connect and read-idle timeout, default 30, from 1 to 600 seconds |
| `OCTARA_CHAT_DIR` | Conversation directory; use an isolated directory for verification |

With no network variables the app opens with Scripted. Configuring any network
variable requires both endpoint and model; invalid configuration exits with a
readable error. A configured network model is selected first, with Scripted still
available in the picker. The CLI/headless `octara_chat::app` stays deterministic
and offline; attach to the normal window to inspect real network behavior.

The adapter sends text history as user/assistant messages with `stream: true` and
`n: 1`. It derives short conversation titles locally. File contents, images, tools,
provider-specific reasoning and usage accounting are outside this text adapter.
Attachment chips do not imply file upload. A tool-call response fails explicitly.

## Stream and lifetime behavior

The parser follows [SSE line framing](https://html.spec.whatwg.org/multipage/server-sent-events.html#event-stream-interpretation),
including UTF-8 split across HTTP chunks, CRLF/CR/LF, comments and multiline `data`.
An event has a 1MiB buffer limit. The
[Chat Completions protocol](https://developers.openai.com/api/reference/resources/chat)
provides text/refusal deltas, finish reasons and an optional usage-only chunk.

- Text/refusal deltas append in order. A `stop` finish reason or `[DONE]` completes
  a nonempty response. A usage-only chunk does not complete it.
- EOF without completion, malformed events, an empty response, output truncation,
  content filtering and unsupported tool calls fail while preserving received text.
- HTTP errors, connection failure and idle timeout appear in the normal failure
  callout with Retry. Raw server bodies, URLs and credentials are not copied into
  error messages. Redirects and automatic HTTP retries are disabled.
- Stop retires the Chat request before aborting its network future. Abort releases
  pending HTTP reads; scope cleanup does the same. Already queued events remain
  subject to the existing request-owner checks. Runtime shutdown does not join
  network workers on the UI thread.

Waiting for response has the `waiting-<message>` ID. Once text arrives, the visible
reply and Stop control indicate ongoing reception. Failures expose `failed-<message>` and `retry-<message>`;
Stop and Regenerate use the existing controls. These live values, pending work,
source locations and screenshots supply the Agent's feedback without a second
request-state registry.

## Local HTTP verification

The Python peer serves actual loopback TCP/HTTP. It supplies deterministic model
responses, not real model inference, and requires no account or key.

```sh
python3 apps/chat/serve_network_fixture.py --output /tmp/octara-http-peer-new
```

It writes `endpoint.txt` and `events.jsonl`. In another terminal, using an empty
conversation directory and unused Inspector path:

```sh
OCTARA_CHAT_ENDPOINT="$(cat /tmp/octara-http-peer-new/endpoint.txt)" \
OCTARA_CHAT_MODEL=local-fixture \
OCTARA_CHAT_DIR=/tmp/octara-http-chat-new \
OCTARA_INSPECT=/tmp/octara-http-chat-new.sock \
cargo run -p octara-chat
```

The first word of the question selects the scenario:

| Question starts with | Response |
| --- | --- |
| `normal` | Small paced Markdown deltas with Chinese and emoji, then completion |
| `hold` | Headers followed by silence; verify waiting, Stop and socket cancellation |
| `idle` | A text prefix followed by silence; verify partial text, Stop or idle timeout |
| `drop` | First attempt drops after partial text; retrying the same question succeeds |
| `partial` | Ends inside an SSE event; the answer must fail |
| `http401` / `http429` | HTTP authentication or quota failure with a visible Retry action |

The fixture has request/output/time bounds and content-free request/termination
events. Its `/status` and `/control` endpoints are test-peer controls; they never
operate the app or synthesize OS input. See `--help` for the control interface.

Use `octara inspect --connect PATH --json` to observe the normal window, and
`octara run --connect PATH --json` for guarded interaction, capture and diagnostics.
Do not use headless `advance` as network time. A bounded live `settle` can time out
while the server intentionally holds a stream; inspect the waiting state or Stop.
An explicit user Retry creates a new request; a transport error is never silently
replayed by this adapter.

Automated checks use a local TCP server, actual HTTP response framing and the real
Chat request lifecycle:

```sh
CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0 \
  cargo test -p octara-chat --all-targets
```

The network checks need permission to bind loopback sockets. No test contacts a
hosted model service. They cover fragmented Unicode, history, HTTP failures,
interruption, cancellation, regeneration, scope disposal and idle timeout.

## Recorded native run, 2026-11-15

The normal release Chat binary connected to the loopback peer with an isolated
store. CUA supplied native input; the live Inspector read the actual window and
captured WindowServer pixels. No fixture-only commands were added to Chat.

Five HTTP requests exercised these outcomes:

1. `hold`: Waiting for response was visible; native Command+Period produced
   Stopped, and the server observed `client_closed` before any text.
2. `drop`: an interrupted response retained its text and exposed the failure and
   Retry action.
3. Clicking that Retry created exactly one new request with identical question
   history, and completed normally. The stored answer was Done.
4. `http429`: the callout described the rate/quota failure and offered Retry;
   private server-body details did not appear in the conversation.
5. `idle`: the first text, including Chinese and emoji, remained visible after
   native Stop. The server observed `client_closed` and the stored answer was Stopped.

Seven captures reported `renderer: display` and `pixels: display`. The request log,
snapshots, captures, stored statuses and cross-check report remain under
`/private/tmp/octara-network-20261115/`. This verifies real HTTP and native-window
interaction with a deterministic peer; it does not claim hosted-model inference,
TLS endpoint interoperability or physical input-to-display latency.

The final workspace run passed 460 tests across 36 suites, including 48 Chat tests.
Clippy with warnings denied, formatting, and `octara check` passed; the latter
reported no findings in light/dark phone/desktop layouts. Five invalid startup
configurations failed without echoing supplied sensitive values.

A final restart confirmed the corrected generic welcome text through native AX.
Restart attempts also encountered the previous run's refused Inspector socket;
only that identified temporary socket was removed after confirming it had no
listener. A blank CUA restart image was excluded from the seven display captures
above. Use a new Inspector path for a new run rather than treating a failed launch
or stale endpoint as a successful connection. Test windows and the HTTP peer were
closed, and temporary launch products were removed after verification.
