# Run Chat

Run `cargo run -p octara-chat-desktop` from the repository root. Chat uses Octara v2’s native windows, editor, and components. Create and manage conversations, read replies as they stream, stop or regenerate a reply, and adjust the model and generation settings.

## Choose a backend

| Environment | App behavior |
| --- | --- |
| No model service configured | Uses the offline Scripted backend to exercise the UI, queue, and cancellation |
| `OPENROUTER_API_KEY` | Uses the built-in OpenRouter preset `deepseek/deepseek-v4.1-flash` |
| `OCTARA_CHAT_ENDPOINT` and `OCTARA_CHAT_MODEL` | Uses the full Chat Completions URL and model you specify. `OCTARA_CHAT_API_KEY` is optional. This configuration takes precedence over the OpenRouter preset |
| `OCTARA_CHAT_DIR` | Stores conversations and settings in the specified directory for isolated tests |
| `OCTARA_CHAT_TIMEOUT_SECS` | Sets the connection and idle read timeouts to a value from 1 to 600 seconds. The default is 60 seconds |

Chat saves conversations as `<thread>.v2.json`. Use the composer’s attachment picker or drag files into the composer. Chat stores attachments as local file references and sends the conversation text to the model.

Model selections use stable IDs. Chat records the model and generation parameters when you send or queue a request. Switching models during a stream applies to future requests. Stopping or regenerating cancels the previous request, and Chat ignores late deltas and completion events from it. Canceling the settings dialog discards the draft. Saving applies the new settings.

With the Scripted model selected, type **/transit** in the composer, select the command, and send the prefilled question. The reply includes expandable analysis notes, citations, images, equations, a light curve, and Rust code. Click an image to enlarge it. Type **@** to insert a sample contact you can delete as a unit, or use **/attach** to open the file picker.

## Deterministic checks

The CLI runs the same UI with the offline model, a fixed clock, and in-memory conversations. Use `--size` and `--theme` to override the window size and theme for inspection.

```sh
cargo run -p octara-cli -- check --package octara-chat
cargo run -p octara-cli -- inspect --package octara-chat --size 390x844
cargo test -p octara-chat
```

The HTTP tests use a loopback server to cover SSE, cancellation, and timeouts. The UI tests exercise the editor and dialogs through Inspect v2. The long-session tests cover virtualized lists and scroll position during streaming. [HTTP tests](tests/network.rs) · [UI tests](tests/chat.rs) · [Long-session tests](tests/long_session.rs)

## Live window and local HTTP

Use the CLI to launch offline Chat with Inspect. These commands run on Unix.

```sh
chat_tmp="$(mktemp -d /tmp/octara-chat.XXXXXX)"
cargo run -p octara-cli -- launch --package octara-chat-desktop --socket "$chat_tmp/inspect.sock" --receipt "$chat_tmp/build.json"
```

In another terminal, replace `/tmp/octara-chat.XXXXXX` with the first terminal’s temporary directory. Run the live check script or inspect the semantic tree. Remove the temporary directory after closing the window.

```sh
python3 -B apps/chat/live_chat.py --connect /tmp/octara-chat.XXXXXX/inspect.sock
cargo run -p octara-cli -- inspect --connect /tmp/octara-chat.XXXXXX/inspect.sock
```

To test local HTTP, start the isolated SSE test server, then launch Chat with the full URL it writes and a separate conversation directory. The test server requires no credentials. [SSE test server](serve_network_fixture.py)

```sh
python3 -B apps/chat/serve_network_fixture.py --output /tmp/octara-chat-peer
```

```sh
chat_tmp="$(mktemp -d /tmp/octara-chat-http.XXXXXX)"
OCTARA_CHAT_ENDPOINT="$(cat /tmp/octara-chat-peer/endpoint.txt)" \
OCTARA_CHAT_MODEL=local-fixture OCTARA_CHAT_DIR="$chat_tmp/session" cargo run -p octara-chat-desktop
```

Send a message and confirm that a reply titled “Local HTTP stream” appears. Close the window, then stop the test server.

## Inspect long sessions

Run `cargo run -p octara-chat-desktop --example long_session` with `OCTARA_CHAT_DIR` pointing to an empty directory to generate a history of 1,000 messages. Set `OCTARA_CHAT_MESSAGES=10000` to inspect a larger history. Sending a message in this window starts a long offline reply.

```sh
cargo run --release -p octara-chat --example long_bench -- --samples 40
cargo run --release -p octara-chat --example stream_bench -- --samples 20
```

The benchmarks report in-process stream intake and commit timings, transaction counts, frame numbers, and the number of mounted messages.
