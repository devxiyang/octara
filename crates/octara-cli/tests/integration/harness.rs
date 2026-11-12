//! `octara check` / `inspect` / `run` against the examples crate, whose Cargo.toml names its view
//! (`[package.metadata.octara] view = "octara_examples::apps::counter"`).

use std::process::Command;

fn octara(args: &[&str], stdin: Option<&str>) -> (i32, String, String) {
    use std::io::Write;
    let manifest = octara_cli::api::workspace_root().join("examples/Cargo.toml");
    let mut child = Command::new(env!("CARGO_BIN_EXE_octara"))
        .args(args)
        .arg("--manifest-path")
        .arg(manifest)
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
fn check_inspect_and_run_drive_the_app() {
    // check: the counter is clean at every theme and size (the kit's buttons take their touch
    // size on the phone's platform).
    let (code, json, err) = octara(&["check", "--json"], None);
    assert_eq!(code, 0, "{err}{json}");
    let report: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(report["schema"], "octara.check/1");
    assert_eq!(report["clean"], true);
    assert_eq!(report["cells"], serde_json::json!(["light/phone", "light/desktop", "dark/phone", "dark/desktop"]));
    let (code, text, _) = octara(&["check"], None);
    assert_eq!((code, text.lines().last()), (0, Some("check: clean (light/phone, light/desktop, dark/phone, dark/desktop)")));

    // inspect: the semantic tree at a size.
    let (code, json, err) = octara(&["inspect", "--json", "--size", "390x844", "--theme", "dark"], None);
    assert_eq!(code, 0, "{err}");
    let snap: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!((snap["schema"].as_str(), snap["theme"].as_str()), (Some("octara.inspect/1"), Some("dark")));
    assert!(json.contains("\"id\": \"inc\""));
    assert!(json.contains("\"source\": \"examples/src/apps.rs:"), "sources are relative to the workspace");

    // run: a script stops at the first failure.
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR"));
    let script = dir.join("counter.jsonl");
    std::fs::write(
        &script,
        "{\"op\":\"tap\",\"id\":\"inc\"}\n{\"op\":\"expect\",\"id\":\"count\",\"text\":\"Count: 1\"}\n{\"op\":\"expect\",\"id\":\"count\",\"text\":\"Count: 9\"}\n{\"op\":\"tap\",\"id\":\"inc\"}\n",
    )
    .unwrap();
    let (code, text, err) = octara(&["run", "--script", script.to_str().unwrap()], None);
    std::fs::remove_file(&script).unwrap();
    assert_eq!(code, 1, "{err}");
    assert_eq!(
        text.lines().collect::<Vec<_>>(),
        ["line 1: ok", "line 2: ok", "line 3: EXPECTATION_FAILED — expected `count` text \"Count: 9\", found \"Count: 1\"", "run: failed"]
    );

    // run without a script: requests from stdin, a reply line each.
    let (code, out, err) = octara(&["run"], Some("{\"op\":\"tap\",\"id\":\"inc\"}\n{\"op\":\"tap\",\"id\":\"nope\"}\n"));
    assert_eq!(code, 0, "{err}");
    let replies: Vec<serde_json::Value> = out.lines().map(|l| serde_json::from_str(l).unwrap()).collect();
    assert_eq!(replies[0]["ok"], true);
    assert_eq!(replies[1]["code"], "NO_SUCH_ELEMENT");

    // capture: the harness paints with the reference renderer.
    let shot = dir.join("counter.png");
    let request = serde_json::json!({ "op": "capture", "path": shot, "scale": 1.0 }).to_string() + "\n";
    let (code, out, err) = octara(&["run"], Some(&request));
    assert_eq!(code, 0, "{err}");
    let reply: serde_json::Value = serde_json::from_str(out.lines().next().unwrap()).unwrap();
    assert_eq!(
        (reply["ok"].as_bool(), reply["capture"]["renderer"].as_str(), reply["capture"]["settled"].as_bool()),
        (Some(true), Some("reference"), Some(true)),
        "{reply}"
    );
    let png = std::fs::read(&shot).unwrap();
    std::fs::remove_file(&shot).unwrap();
    assert_eq!(&png[1..4], b"PNG");
}

#[test]
fn an_app_without_a_view_is_told_how_to_name_one() {
    let manifest = octara_cli::api::workspace_root().join("crates/octara-kit/Cargo.toml");
    let err = octara_cli::harness::find(Some(&manifest), Some("octara-kit")).unwrap_err();
    assert!(err.contains("[package.metadata.octara] view"), "{err}");
}
