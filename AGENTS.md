# AGENTS.md

- Write all repository content in English (docs, READMEs, skills, comments, commit messages, diagnostics, and default examples). Non-English text is allowed only in i18n locale tables and CJK/IME/BiDi test fixtures.
- Follow `docs/design/architecture-v2.md` (and `docs/design/v2/*`), enforced by `tools/check_layers.py`.
- Register diagnostics in `octara-model` (`diagnostics.rs`) and keep `cargo clippy --workspace --all-targets -- -D warnings` and tests passing.
- Regenerate `skills/octara/` (`octara-cli skill export skills`) only when finalizing docs/skill changes for release.
