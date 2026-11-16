# Conversation settings

The gear button opens a kit Dialog containing a searchable model Combobox and two NumberFields.
All state, lifecycle and input behavior comes from existing framework primitives.

## Commit and request semantics

- Opening creates a fresh draft from the current model and generation settings. Cancel, Escape
  and Close discard it. Nested model search follows the existing modal stack and keyboard rules.
- Save validates the configured model ID and numeric values, atomically replaces `settings.json`,
  then publishes the new Signals. A write failure leaves the previous configuration intact and
  keeps the dialog open with an error. An app without a Store applies settings in memory.
- Restore defaults selects the first configured model and clears overrides. It does not write
  anything until Save. Blank values mean server defaults, not zero. Scripted ignores overrides.
- Temperature accepts 0–2; maximum completion tokens accepts integer values 1–131072. These are
  application input bounds, not a claim about a provider's supported parameters or model limits.
  NumberField normalizes finite numeric drafts on blur/submit according to its range and step;
  unparseable drafts remain visible and disable Save. Temperature's step is 0.1; tokens use 1.
- Send captures model and parameters before starting or queuing a request. Later changes cannot
  retarget active or queued messages. Automatic dequeue and Send now use the queued snapshot.
  Editing a queued message restores its model, parameters and draft for an explicit resend.
- Retry and Regenerate capture the current settings. Title generation uses the same model but
  default parameters; the HTTP adapter derives titles locally without another model request.
- On restart, valid preferences are restored; malformed files fall back to defaults. A saved
  model ID absent from configuration stays unavailable and needs explicit selection.
  Header model changes and editing queued messages change current state; only dialog Save
  persists the preferred startup configuration.

Settings contain only the model ID and generation overrides. Endpoints and keys stay in the
process configuration. Conversation files remain `<thread>.v2.json`; settings are not loaded
as a conversation. A small synchronous settings write occurs only on explicit Save; conversation
saves retain their existing background writer.

## Verification

```sh
cargo test -p octara-chat
cargo build -p octara-chat --example model_selection -p octara-cli --bin octara
python3 apps/chat/live_settings.py
```

Headless tests cover invalid drafts, cancel, save, restore defaults, restart, malformed files,
write failure and temporary-file cleanup. The HTTP test reads actual request JSON for an active
request, a queued request and regeneration, including omitted default keys. The live script
combines the product UI with a loopback HTTP peer, verifies model and parameter snapshots,
resizes the real window, restarts it and captures the GPU-rendered settings UI.

For the real DeepSeek service, use `live_openrouter.py` as documented in [NETWORK.md](NETWORK.md).
Both scripts use Inspector framework input, fresh element targets and isolated app stores;
neither claims native keyboard or Chinese IME injection. Intentional ellipsis in conversation
titles and the compact model picker is accepted; other layout/diagnostic issues fail verification.

## Recorded acceptance — 2026-11-16

- Workspace regression: 863 passed, 3 ignored. After the final local-title refinement, all
  72 Chat tests passed. Workspace Clippy with `--all-targets -- -D warnings` passed.
- `/tmp/octara-settings-verified-20261116/summary.json`: the real-window loopback check passed
  validation, cancel, model/parameter snapshots, regeneration defaults, native resize, restart
  and restore defaults; five GPU captures retained.
- `/tmp/octara-openrouter-verified-20261116/summary.json`: the ordinary application used the
  actual `deepseek/deepseek-v4.1-flash` service. Chinese answer, contextual follow-up,
  regeneration, stopping, persistence and compact resize passed; five GPU captures retained.
- Screenshots in the latter directory: `01-welcome.png`, `02-answer.png`, `03-followup.png`,
  `05-settings.png`, `06-compact.png`. These are rendered application pixels, not mockups.

The API smoke test is opt-in and uses credits; it is not part of `cargo test` or offline CI.
