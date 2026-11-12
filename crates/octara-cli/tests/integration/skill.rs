//! The `octara` skill: generated for this version, installed where agents look, with a pointer
//! in the project's AGENTS.md; the repository's copy (for `npx skills add`) is current.

use octara_cli::api::{self, Api};
use octara_cli::skill::{self, Install};
use std::path::{Path, PathBuf};

fn api() -> &'static Api {
    api::load()
}

/// A fresh scratch directory under cargo's per-target temp dir, removed by the caller.
fn scratch(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("skill-{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("project")).unwrap();
    std::fs::create_dir_all(dir.join("home")).unwrap();
    dir
}

fn opts(dir: &Path) -> Install {
    Install { project: dir.join("project"), home: dir.join("home"), global: false, agents: None, pointer: true }
}

#[test]
fn the_skill_follows_the_agent_skills_format() {
    let files = skill::files(Some(api()));
    let (_, skill_md) = files.iter().find(|(p, _)| p == "SKILL.md").unwrap();
    let front = skill_md.strip_prefix("---\n").and_then(|s| s.split_once("\n---\n")).map(|(f, _)| f).unwrap();
    let name = front.lines().find_map(|l| l.strip_prefix("name: ")).unwrap();
    assert_eq!(name, skill::NAME, "the name is the directory's");
    let description = front.lines().find_map(|l| l.strip_prefix("description: ")).unwrap();
    // Quoted: the text contains `: `, which a plain YAML scalar may not.
    let description = description.strip_prefix('"').and_then(|d| d.strip_suffix('"')).expect("a double-quoted YAML string");
    assert!(!description.is_empty() && description.len() <= 1024 && !description.contains('"'));
    assert!(skill_md.lines().count() < 500, "SKILL.md stays under the spec's 500 lines");
    let (_, start) = files.iter().find(|(p, _)| p == "references/start-here.md").unwrap();
    assert!(!start.contains("\n# use octara"), "doctest-hidden lines are dropped for readers");
    assert!(!start.contains("rust,no_run"));
    // The API reference: an index of modules, a file per module, a large type on its own.
    let (_, index) = files.iter().find(|(p, _)| p == "references/api/index.md").unwrap();
    assert!(index.contains("[`octara::view`](octara.view.md)"), "{index}");
    let (_, view) = files.iter().find(|(p, _)| p == "references/api/octara.view.md").unwrap();
    assert!(view.contains("- `pub fn text(") && view.contains("members: [octara.view.Styled.md]"), "{view}");
    let (_, styled) = files.iter().find(|(p, _)| p == "references/api/octara.view.Styled.md").unwrap();
    assert!(
        styled
            .lines()
            .any(|l| l.starts_with("- `fn p_4(self) -> Self`") && l.ends_with(".rs:418`") || l.starts_with("- `fn p_4(self) -> Self`")),
        "{styled}"
    );
    assert!(skill::files(None).iter().all(|(p, _)| !p.starts_with("references/api/")), "the exported copy has no reference");
}

#[test]
fn install_writes_the_canonical_copy_links_and_the_pointer() {
    let dir = scratch("install");
    std::fs::create_dir_all(dir.join("home/.claude")).unwrap(); // Claude Code is installed
    std::fs::write(dir.join("project/AGENTS.md"), "# My app\n\nRun `cargo test`.\n").unwrap();
    std::fs::write(dir.join("project/CLAUDE.md"), "Be brief.\n").unwrap();
    let cat = api();

    let report = skill::install(cat, &opts(&dir)).unwrap();
    let canonical = dir.join("project/.agents/skills/octara");
    assert!(skill::stale(Some(cat), &canonical).is_empty(), "{report:?}");
    let claude = dir.join("project/.claude/skills/octara");
    assert_eq!(std::fs::read_link(&claude).unwrap(), Path::new("../../.agents/skills/octara"));
    assert!(skill::stale(Some(cat), &claude).is_empty(), "Claude Code sees the same files");
    assert!(!dir.join("project/.kiro").exists(), "agents that are not installed get nothing");

    let agents_md = std::fs::read_to_string(dir.join("project/AGENTS.md")).unwrap();
    assert!(agents_md.starts_with("# My app\n\nRun `cargo test`.\n\n<!-- octara:start -->"), "{agents_md}");
    assert!(agents_md.contains(".agents/skills/octara/SKILL.md"));
    assert_eq!(std::fs::read_to_string(dir.join("project/CLAUDE.md")).unwrap(), "Be brief.\n\n@AGENTS.md\n");

    // Again: nothing to do.
    assert_eq!(skill::install(cat, &opts(&dir)).unwrap(), Vec::<String>::new());
    // An edited copy is refreshed; the pointer block is replaced, not repeated.
    std::fs::write(canonical.join("SKILL.md"), "---\nname: octara\ndescription: old\n---\n").unwrap();
    std::fs::write(dir.join("project/AGENTS.md"), agents_md.replace("Before writing", "OLD. Before writing") + "\nMore notes.\n").unwrap();
    skill::install(cat, &opts(&dir)).unwrap();
    assert!(skill::stale(Some(cat), &canonical).is_empty());
    let again = std::fs::read_to_string(dir.join("project/AGENTS.md")).unwrap();
    assert_eq!(again.matches("<!-- octara:start -->").count(), 1);
    assert!(!again.contains("OLD.") && again.ends_with("More notes.\n"), "{again}");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn global_install_and_explicit_agents() {
    let dir = scratch("global");
    let cat = api();
    let o = Install { global: true, agents: Some(vec!["claude".into(), "amp".into()]), ..opts(&dir) };
    skill::install(cat, &o).unwrap();
    assert!(skill::stale(Some(cat), &dir.join("home/.agents/skills/octara")).is_empty());
    assert!(skill::stale(Some(cat), &dir.join("home/.claude/skills/octara")).is_empty());
    assert!(skill::stale(Some(cat), &dir.join("home/.config/agents/skills/octara")).is_empty(), "Amp's user directory");
    assert!(!dir.join("project/AGENTS.md").exists(), "a global install leaves projects alone");
    let bad = Install { agents: Some(vec!["nope".into()]), ..opts(&dir) };
    assert!(skill::install(cat, &bad).is_err());
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn install_does_not_overwrite_another_skill() {
    let dir = scratch("guard");
    let theirs = dir.join("project/.agents/skills/octara");
    std::fs::create_dir_all(&theirs).unwrap();
    std::fs::write(theirs.join("SKILL.md"), "---\nname: something-else\n---\n").unwrap();
    assert!(skill::install(api(), &opts(&dir)).is_err());
    assert_eq!(std::fs::read_to_string(theirs.join("SKILL.md")).unwrap(), "---\nname: something-else\n---\n");
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn the_repository_copy_is_current() {
    let root = api::workspace_root().join("skills/octara");
    let stale = skill::stale(None, &root);
    assert!(stale.is_empty(), "skills/octara is out of date ({stale:?}) — run `cargo run -p octara-cli -- skill export skills`");
}
