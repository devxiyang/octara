//! `octara check` / `inspect` / `run` against apps with `[package.metadata.octara] entry`.

use std::process::Command;

fn cli(manifest: &str, args: &[&str]) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_octara"));
    command.args(args).arg("--manifest-path").arg(octara_cli::api::workspace_root().join(manifest));
    command
}

fn output(command: &mut Command, stdin: Option<&str>) -> (i32, String, String) {
    use std::io::Write;
    let mut child = command
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(stdin.unwrap_or("").as_bytes()).unwrap();
    let out = child.wait_with_output().unwrap();
    (out.status.code().unwrap_or(-1), String::from_utf8(out.stdout).unwrap(), String::from_utf8(out.stderr).unwrap())
}

#[test]
fn check_inspect_and_run_drive_the_apps() {
    let counter = |args: &[&str], stdin| output(&mut cli("examples/Cargo.toml", args), stdin);

    // check: the counter is clean at every theme and size (the kit's buttons take their touch
    // size on the phone's platform).
    let (code, json, err) = counter(&["check", "--json"], None);
    assert_eq!(code, 0, "{err}{json}");
    let report: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(report["schema"], "octara.check/1");
    assert_eq!(report["clean"], true);
    assert_eq!(report["cells"], serde_json::json!(["light/phone", "light/desktop", "dark/phone", "dark/desktop"]));
    let (code, text, _) = counter(&["check"], None);
    assert_eq!((code, text.lines().last()), (0, Some("check: clean (light/phone, light/desktop, dark/phone, dark/desktop)")));

    // inspect: the semantic tree at a size.
    let (code, json, err) = counter(&["inspect", "--json", "--size", "390x844", "--theme", "dark"], None);
    assert_eq!(code, 0, "{err}");
    let snap: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!((snap["schema"].as_str(), snap["theme"].as_str()), (Some("octara.inspect/2"), Some("dark")));
    assert_eq!(snap["viewport"], serde_json::json!([390.0, 844.0]));
    assert_eq!(snap["completion"], serde_json::json!({"window": snap["window"], "layout": "settled", "surface": {"state": "unavailable"}}));
    assert!(json.contains("\"id\": \"inc\""));
    let source = json.lines().find(|l| l.contains("\"source\""));
    assert!(json.contains("\"source\": \"examples/src/apps.rs:"), "sources are relative to the workspace: {source:?}");

    // run: a script stops at the first failure.
    let temp = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap();
    let dir = temp.path();
    let script = dir.join("counter.jsonl");
    std::fs::write(
        &script,
        "{\"op\":\"tap\",\"id\":\"inc\"}\n{\"op\":\"expect\",\"id\":\"count\",\"text\":\"Count: 1\"}\n{\"op\":\"expect\",\"id\":\"count\",\"text\":\"Count: 9\"}\n{\"op\":\"tap\",\"id\":\"inc\"}\n",
    )
    .unwrap();
    let (code, text, err) = counter(&["run", "--script", script.to_str().unwrap()], None);
    assert_eq!(code, 1, "{err}");
    assert_eq!(
        text.lines().collect::<Vec<_>>(),
        ["line 1: ok", "line 2: ok", "line 3: EXPECTATION_FAILED — expected `count` text \"Count: 9\", found \"Count: 1\"", "run: failed"]
    );

    // run without a script: requests from stdin, a reply line each.
    let (code, out, err) =
        counter(&["run"], Some("{\"op\":\"tap\",\"id\":\"inc\"}\n{\"op\":\"tap\",\"id\":\"nope\"}\n{\"op\":\"inspect\"}\n"));
    assert_eq!(code, 0, "{err}");
    let replies: Vec<serde_json::Value> = out.lines().map(|l| serde_json::from_str(l).unwrap()).collect();
    assert_eq!(replies[0]["ok"], true);
    assert_eq!(replies[0]["completion"]["surface"]["state"], "unavailable");
    assert_eq!(replies[1]["code"], "NO_SUCH_ELEMENT");
    assert_eq!(replies[2]["snapshot"]["viewport"], serde_json::json!([800.0, 600.0]));
    assert_eq!(replies[2]["snapshot"]["windows"][0]["title"], "Octara — counter");
    let (code, _, err) = counter(&["run", "--theme", "invalid"], None);
    assert_eq!(code, 2, "{err}");
    assert!(err.contains("unknown theme `invalid`"), "{err}");

    // capture: the harness paints with the reference renderer.
    let shot = dir.join("counter.png");
    let request = serde_json::json!({ "op": "capture", "path": shot, "scale": 1.0 }).to_string() + "\n";
    let (code, out, err) = counter(&["run"], Some(&request));
    assert_eq!(code, 0, "{err}");
    let reply: serde_json::Value = serde_json::from_str(out.lines().next().unwrap()).unwrap();
    assert_eq!(
        (reply["ok"].as_bool(), reply["capture"]["renderer"].as_str(), reply["capture"]["settled"].as_bool()),
        (Some(true), Some("reference"), Some(true)),
        "{reply}"
    );
    let png = std::fs::read(&shot).unwrap();
    assert_eq!(&png[1..4], b"PNG");

    // Both packages build into the same harness path, so finish the counter before driving Chat.
    let chat = |args: &[&str], stdin| {
        let mut command = cli("apps/chat/Cargo.toml", args);
        command
            .env_remove("OCTARA_CHAT_ENDPOINT")
            .env_remove("OCTARA_CHAT_MODEL")
            .env_remove("OCTARA_CHAT_API_KEY")
            .env_remove("OPENROUTER_API_KEY")
            .env("OCTARA_CHAT_TIMEOUT_SECS", "0")
            .env("OCTARA_CHAT_DIR", dir.join("chat"));
        output(&mut command, stdin)
    };
    let (code, json, err) = chat(&["inspect", "--json"], None);
    assert_eq!(code, 0, "{err}");
    let snap: octara::inspect::Snapshot = serde_json::from_str(&json).unwrap();
    assert_eq!(snap.windows[0].title, "Octara Chat");
    assert_eq!(snap.viewport, [1180.0, 820.0]);
    assert_eq!(snap.theme, "light");
    assert_eq!(snap.find("model").unwrap().value.as_deref(), Some("Scripted"));

    let (code, json, err) = chat(&["run", "--size", "390x844", "--theme", "dark"], Some("{\"op\":\"inspect\"}\n"));
    assert_eq!(code, 0, "{err}");
    let reply: octara::inspect::Reply = serde_json::from_str(&json).unwrap();
    let snap = reply.snapshot.unwrap();
    assert_eq!(snap.viewport, [390.0, 844.0]);
    assert_eq!(snap.theme, "dark");
    assert_eq!(snap.find("model").unwrap().value.as_deref(), Some("Scripted"));

    let (code, json, err) = chat(&["check", "--json"], None);
    assert_eq!(code, 0, "{err}{json}");
    let report: octara::inspect::CheckReport = serde_json::from_str(&json).unwrap();
    assert!(report.clean, "{json}");
}

#[test]
fn an_app_without_an_entry_is_told_how_to_name_one() {
    let manifest = octara_cli::api::workspace_root().join("crates/octara-kit/Cargo.toml");
    let err = octara_cli::harness::find(Some(&manifest), Some("octara-kit")).unwrap_err();
    assert!(err.contains("[package.metadata.octara] entry"), "{err}");
}
