# How Agents Get the Framework's Full Usage: Research

2026-11. Compares Expo's Exact (source and docs read), Rune, GPUI (source read), and the approaches of Expo, Next.js, Svelte, Angular, Storybook and others
(second-hand summary from web search, original pages not read one by one; parts from memory were marked in the original report).

## Three references

| | Exact | Rune | GPUI |
| --- | --- | --- | --- |
| What to read first | `AGENTS.md` points to the single must-read page `docs/start-here.md` (337 lines); every other doc opens with "for lookup, do not read through" (`contract-for-agents.md:3-5`) | A 64-line root README ("Use with your agent") → `examples/hello` and the 2915-line crate README | GPUI notes in the root `AGENTS.md` (190 lines), then read rustdoc |
| Lookup | `contract vocab <name>`: any tag, property, function, concept, with the list coming from the compiler's own tables so it cannot drift; about 60 recipes, one problem each | None (planned: `llms.txt` and a component catalog generated from definitions) | rustdoc guide modules (`_ownership_and_data_flow.rs` etc.), about 45 examples, component previews |
| Runtime | `agent.mjs tree / tap id / type / clock / state / logs / screenshot`; `verify` runs build, tests, light and dark themes, two widths and largest font size in one go: "a clean verify means done" | Any app with `--tree --snap --script --trace`; motion gate | No inspection interface for agents |
| Diagnostics | Points out the cause, observable symptom, fix and doc anchor | Planned: "errors with fixes (Text in a dynamic list has no key: add `.key(id)`)" | Panics explain what happened but give no fix |
| Staying correct | Recipes have a byte cap (default 3072; when CI errors it tells you how to change it); `--help` and argument parsing are aligned by tests; recipes are embedded in the CLI via `include_str!`; the pitfall list requires reproducibility and a date, and entries are deleted once superseded by a diagnostic | Site code snippets are cut from real source at build time and fail if missing; examples come with tests | doctests, compilable examples, rule hygiene (rules are "pitfalls to avoid", not an architecture map) |
| Feedback | `exact feedback note`, a journal per app | One changelog entry per change | — |

## Ecosystem

- **Expo**: `llms.txt` (about 94 KB index), `llms-full.txt` (about 1.9 MB) and per-topic files; the new-project template writes `AGENTS.md`, pointing questions to the relevant files;
  MCP servers (docs search, simulator automation, builds, observability); Claude Code skill plugins (`expo-router`, `expo-ui` ...).
- **Next.js**: ships version-matched docs with the package, a codemod tool generates `AGENTS.md`; the dev server exposes MCP (`/_next/mcp`: errors, routes).
- **Svelte**: MCP provides sectioned doc reads and `svelte-autofixer` (returns issues and suggestions, suited to an "edit until clean" loop).
- **Angular / Storybook**: best practices, example search; Storybook uses the component manifest as the single source.
- **Vercel's evals** ([original](https://vercel.com/blog/agents-md-outperforms-skills-in-our-agent-evals), 2026-01-27; Next.js 16,
  APIs outside the training data): no docs 53%; skill installed only 53% (the skill was not invoked in 56% of cases); explicitly told to use the skill 79%; an
  approximately 8 KB compressed index in `AGENTS.md` 100%. The original gives no model, framework or number of questions. Conclusion: skills suit vertical workflows; broad framework knowledge goes in an always-present `AGENTS.md`.
- **llms.txt**: crawlers mostly do not read it; it helps coding agents only if `AGENTS.md` or MCP explicitly points to it.
- **Index services such as Context7**: coverage and version matching are unreliable, so use only as a fallback.
- **Rust**: rustdoc JSON (nightly, format changes often, needs a wrapper in your own stable format); doctests keep examples compilable; `#[diagnostic::on_unimplemented]`,
  `#[must_use]`, `#[deprecated(note)]` make compile errors teach; `compile_fail` doctests pin the error output for common mistakes.

## Where skills are installed (2026-11, each vendor's official docs)

- Format: [Agent Skills](https://agentskills.io/specification). `name` (≤ 64, lowercase and hyphens, equal to the directory name) and `description` (≤ 1024) are required;
  optional `license`, `compatibility`, `metadata`; the directory is `name/SKILL.md` plus `references/` etc.; the spec does not define discovery locations.
- Project-level `.agents/skills/`: read by Codex, Cursor, Copilot, Gemini CLI, OpenCode, Amp, Windsurf, Zed, Junie, Goose.
  User-level `~/.agents/skills/`: Codex, Cursor, Copilot, Gemini CLI, OpenCode, Windsurf, Zed, Goose (Amp uses `~/.config/agents/skills`).
- Own directory only: Claude Code (`.claude/skills`, `~/.claude/skills`), Kiro (`.kiro/skills`). Cline / Roo not verified.
- Vercel's `npx skills add`: detects installed agents, puts the canonical copy in `.agents/skills/<name>`, symlinks it into each vendor's directory (copying on failure), and uses content hashes to decide updates.
- Next.js: ships version-matched docs with the package; `create-next-app` and `next dev` write a marked block of guidance into the project `AGENTS.md`; skills only handle workflows.
- `AGENTS.md`: maintained by the Agentic AI Foundation under the Linux Foundation, the one nearest the edited file takes effect; Claude Code reads it only when there is no `CLAUDE.md`,
  otherwise `CLAUDE.md` needs to contain `@AGENTS.md`.

## Conclusions

1. **Code is the single source**: the API inventory (methods, signatures, docs, examples) is generated from code, not hand-written; only concepts and "when to use which" are hand-written.
2. **One must-read page, the rest by lookup** (Exact): a small always-present entry point (Vercel: an always-present index beats on-demand skills), everything else looked up by name. Framework knowledge ships with the version as skills
   in `.agents/skills/` (linked for Claude Code and Kiro), plus an always-present block of guidance in the project `AGENTS.md` to cover the case where skills are not invoked.
3. **The running framework is the index**: `vocab <name>`-style queries are answered by the binary itself, with content from a generated registry so it cannot drift (Exact).
4. **A verification loop matters more than docs**: the headless app's semantic tree, acting by test id, clock, pending work, layout issues, screenshots, plus a "clean means done" `check` (Exact `verify`, Svelte autofixer).
5. **Diagnostics teach the fix**: stable code, cause, symptom, fix, doc anchor; the same text serves compile time, runtime, the CLI and MCP.
6. **Examples are tests**: every example and every recipe is compiled and run; common mistakes are pinned with `compile_fail` on the error text.
7. **CLI first, MCP is another exit for the same commands**: agents always have a shell; MCP is only a wrapper.
8. **Version consistency**: ships with the crate, carries a version number; detects a mismatch between the project's version and the docs version.
9. **Feedback and evals**: `feedback note`; evaluate in CI with a fixed task set (with and without each kind of material), and count failures by diagnostic code.
