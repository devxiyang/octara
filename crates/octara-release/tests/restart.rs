#![cfg(feature = "signing")]

use octara_release::{Policy, ReleaseTarget, Staged, Store, public_key, signing};
use std::io;
use std::path::Path;
use std::process::{Command, Stdio};

const CHILD_STORE: &str = "OCTARA_TEST_RELEASE_RESTART_STORE";
const CHILD_KEY: &str = "OCTARA_TEST_RELEASE_RESTART_PUBLIC_KEY";
const INTERRUPTED: i32 = 23;

fn target() -> ReleaseTarget {
    ReleaseTarget {
        app: "dev.octara.restart-test".into(),
        channel: "stable".into(),
        platform: "macos".into(),
        runtime: "restart/1".into(),
        resource_schema: "restart.resource/1".into(),
        state_schema: "restart.state/1".into(),
    }
}

fn store(path: &Path, key: &str) -> Store {
    Store::new(path, Policy { target: target(), public_key: public_key(key).unwrap(), embedded_sequence: 1 })
}

fn signed(key: &[u8], sequence: u64, title: &str) -> Vec<u8> {
    let document = serde_json::json!({
        "format": "octara.resource.v1",
        "target": target(),
        "sequence": sequence,
        "payload": title,
    });
    signing::sign(key, serde_json::to_vec(&document).unwrap().as_slice()).unwrap()
}

fn prepare(payload: &str) -> io::Result<String> {
    Ok(serde_json::from_str(payload)?)
}

#[test]
fn signed_candidate_rolls_back_after_process_exit_before_first_frame() {
    if let Some(path) = std::env::var_os(CHILD_STORE) {
        let key = std::env::var(CHILD_KEY).unwrap();
        let startup = store(Path::new(&path), &key).start(prepare).unwrap();
        let (version, title) = startup.selected.as_ref().unwrap();
        assert_eq!((version.sequence, title.as_str()), (3, "Trial"));
        assert!(startup.recovery.is_none());
        std::process::exit(INTERRUPTED);
    }

    let directory = tempfile::tempdir().unwrap();
    let site = directory.path().join("site");
    let state = directory.path().join("state");
    let (private, public) = signing::generate_key().unwrap();
    let store = store(&state, &public);
    let stage = |sequence, title| {
        let packet = signed(&private, sequence, title);
        let published = signing::publish(&site, public_key(&public).unwrap(), packet.as_slice()).unwrap();
        let bytes = std::fs::read(site.join(published.package)).unwrap();
        assert_eq!(store.stage(bytes.as_slice(), prepare).unwrap(), Staged::New(published.version));
        (published.version, bytes)
    };

    let (confirmed, _) = stage(2, "Confirmed");
    let first = store.start(prepare).unwrap();
    assert_eq!(first.selected, Some((confirmed, "Confirmed".into())));
    assert_eq!(first.session.confirm().unwrap(), Some(confirmed));
    drop(first);
    let (trial, trial_packet) = stage(3, "Trial");

    let output = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "signed_candidate_rolls_back_after_process_exit_before_first_frame"])
        .env(CHILD_STORE, state)
        .env(CHILD_KEY, public)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(INTERRUPTED), "{output:?}");

    let rolled_back = store.start(prepare).unwrap();
    assert_eq!(rolled_back.selected, Some((confirmed, "Confirmed".into())));
    assert!(rolled_back.recovery.as_deref().is_some_and(|reason| reason.contains("did not confirm its first frame")));
    assert_eq!(rolled_back.session.confirm().unwrap(), None);
    assert_eq!(store.stage(trial_packet.as_slice(), prepare).unwrap(), Staged::AlreadyAdmitted(trial));
    drop(rolled_back);
    let steady = store.start(prepare).unwrap();
    assert_eq!(steady.selected, Some((confirmed, "Confirmed".into())));
    assert!(steady.recovery.is_none());
}
