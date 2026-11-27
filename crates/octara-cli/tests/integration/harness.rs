//! The generated driver builds and acts on real authored application entries.

use octara_inspect::{AgentAction, AgentCommand, AgentReceipt, AgentRequest, protocol::Delivery};
use octara_model::semantics::A11yAction;
use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};

fn cli(manifest: &str, args: &[&str]) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_octara"));
    command.args(args).arg("--manifest-path").arg(octara_cli::api::workspace_root().join(manifest));
    command
}

fn counter(args: &[&str]) -> Command {
    cli("examples/Cargo.toml", args)
}

fn success(output: std::process::Output) -> String {
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    String::from_utf8(output.stdout).unwrap()
}

fn send(input: &mut impl Write, output: &mut impl BufRead, command: AgentCommand) -> AgentReceipt {
    let request = AgentRequest::new(None, command);
    writeln!(input, "{}", serde_json::to_string(&request).unwrap()).unwrap();
    input.flush().unwrap();
    octara_cli::live::read_reply(output).unwrap()
}

#[test]
fn check_inspect_and_run_the_declared_counter() {
    let report: serde_json::Value = serde_json::from_str(&success(counter(&["check", "--json"]).output().unwrap())).unwrap();
    assert_eq!(report["schema"], "octara.check/2");
    assert_eq!(report["clean"], true);
    assert_eq!(report["cells"], serde_json::json!(["light/phone", "light/desktop", "dark/phone", "dark/desktop"]));

    let json = success(counter(&["inspect", "--json", "--size", "390x844", "--theme", "dark"]).output().unwrap());
    let inspected: AgentReceipt = serde_json::from_str(&json).unwrap();
    assert_eq!(inspected.protocol, "octara.inspect/2");
    assert!(inspected.progress.settled);
    assert_eq!(inspected.progress.presented_frame, None);
    let snapshot = inspected.snapshot.unwrap();
    assert_eq!((snapshot.viewport.size.width, snapshot.viewport.size.height), (390.0, 844.0));
    assert!(snapshot.nodes.iter().any(|node| node.test_id.as_deref() == Some("inc")));

    let mut child = counter(&["run", "--size", "230x220"]).stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap();
    let mut input = child.stdin.take().unwrap();
    let mut output = BufReader::new(child.stdout.take().unwrap());
    let current = send(&mut input, &mut output, AgentCommand::Observe).snapshot.unwrap();
    let target = current.nodes.iter().find(|node| node.test_id.as_deref() == Some("inc")).unwrap().target;
    let stale_target = octara::TargetGuard { tree: target.tree.next_generation().unwrap(), ..target };
    let refused = send(
        &mut input,
        &mut output,
        AgentCommand::Act { target: stale_target, action: AgentAction::Accessibility { action: A11yAction::Press } },
    );
    assert_eq!(refused.code.as_deref(), Some("STALE_TARGET"));
    assert_eq!(refused.delivery, Delivery::Refused);
    let act = send(&mut input, &mut output, AgentCommand::Act { target, action: AgentAction::Accessibility { action: A11yAction::Press } });
    assert!(act.ok, "{act:?}");
    assert_eq!(act.delivery, Delivery::Delivered);
    assert_eq!(act.handled, Some(true));
    let after = send(&mut input, &mut output, AgentCommand::Observe).snapshot.unwrap();
    assert!(after.nodes.iter().any(|node| node.text.as_deref() == Some("Count: 1")), "{after:?}");
    let capture = send(&mut input, &mut output, AgentCommand::Capture { source: Default::default(), settle_ms: 0 });
    let image = capture.capture.unwrap().picture;
    assert_eq!((image.width(), image.height()), (230, 220));
    drop(input);
    assert!(child.wait().unwrap().success());

    let temp = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap();
    let script = temp.path().join("requests.jsonl");
    std::fs::write(
        &script,
        concat!(
            "// observe and stop at the first refused request\n",
            "{\"protocol\":\"octara.inspect/2\",\"op\":\"observe\"}\n",
            "{\"protocol\":\"unsupported\",\"op\":\"observe\"}\n",
            "{\"protocol\":\"octara.inspect/2\",\"op\":\"list_windows\"}\n",
        ),
    )
    .unwrap();
    let output = counter(&["run", "--json", "--script", script.to_str().unwrap()]).output().unwrap();
    assert_eq!(output.status.code(), Some(1));
    let replies: Vec<AgentReceipt> =
        String::from_utf8(output.stdout).unwrap().lines().map(|line| serde_json::from_str(line).unwrap()).collect();
    assert_eq!(replies.len(), 2);
    assert!(replies[0].ok);
    assert_eq!(replies[1].code.as_deref(), Some("INSPECT_PROTOCOL"));
    assert_eq!(replies[1].delivery, Delivery::Refused);
}

#[test]
fn audit_runs_the_declared_counter_headlessly() {
    let output = counter(&["audit", "--json", "--size", "390x844", "--theme", "dark"]).output().unwrap();
    let report: serde_json::Value = serde_json::from_str(&success(output)).unwrap();
    assert_eq!(report["schema"], "octara.audit/1");
    assert_eq!(report["clean"], true, "{report}");
    let windows = report["windows"].as_array().unwrap();
    assert_eq!(windows.len(), 1);
    assert_eq!(windows[0]["title"], "Octara — counter");
    assert_eq!(windows[0]["findings"], serde_json::json!([]));
}

#[cfg(unix)]
#[test]
fn the_declared_entry_compiles_with_the_native_inspection_bridge() {
    let manifest = octara_cli::api::workspace_root().join("examples/Cargo.toml");
    let target = octara_cli::harness::find(Some(&manifest), Some("octara-examples")).unwrap();
    let executable = octara_cli::harness::build_native(&target, false).unwrap();
    assert!(executable.is_file());
}

#[test]
fn the_chat_entry_uses_its_headless_defaults() {
    let temp = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap();
    let mut child = cli("apps/chat/Cargo.toml", &["run"])
        .env("OCTARA_CHAT_DIR", temp.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut input = child.stdin.take().unwrap();
    let mut replies = BufReader::new(child.stdout.take().unwrap());
    let listed = send(&mut input, &mut replies, AgentCommand::ListWindows).windows.unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].title, "Octara Chat");
    assert_eq!((listed[0].viewport.size.width, listed[0].viewport.size.height), (1180.0, 820.0));
    drop(input);
    assert!(child.wait().unwrap().success());
}

#[test]
fn an_app_without_an_entry_is_told_how_to_name_one() {
    let manifest = octara_cli::api::workspace_root().join("crates/octara-kit/Cargo.toml");
    let error = octara_cli::harness::find(Some(&manifest), Some("octara-kit")).unwrap_err();
    assert!(error.contains("[package.metadata.octara]"), "{error}");
}
