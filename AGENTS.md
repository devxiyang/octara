# AGENTS.md

## Safety

- Do not publicly share the user's private Gmail address without explicit confirmation.
- Reuse this workspace's Cargo build cache; after builds/tests, stop spawned processes and remove temporary files, sockets, and captures. Leave global Cargo registries and toolchains alone.
- Treat Git branches matching `archive/*` as permanent; deleting, rewriting, or repointing one requires explicit confirmation naming the exact branch (local or remote).

## Working on this repository

- Architecture and crate boundaries follow `docs/design/architecture-v2.md` (and `docs/design/v2/*`), enforced by `tools/check_layers.py`.
- Register diagnostics in `octara-model` (`diagnostics.rs`) and keep unit/integration tests cleanly organized and passing with `cargo clippy --workspace --all-targets`.
- Regenerate `skills/octara/` (`octara-cli skill export skills`) only when finalizing docs/skill changes (Phase 5).
