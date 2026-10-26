use std::path::PathBuf;
use std::process::Command;

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../sideeye-core/tests/fixtures")
        .join(name)
}

#[test]
fn fake_provider_executes_packet_and_emits_provenance() {
    let output = Command::new(env!("CARGO_BIN_EXE_sideeye"))
        .args([
            "review",
            "--provider",
            "fake",
            "--fixture",
            fixture("python-verdict.json").to_str().unwrap(),
            fixture("review-packet.json").to_str().unwrap(),
        ])
        .output()
        .expect("run sideeye");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let verdict: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("verdict is JSON");
    assert_eq!(verdict["packet_id"], "packet-golden-1");
    assert_eq!(verdict["judge_provider"], "fake");
    assert_eq!(verdict["judge_model"], "fixture-judge");
    assert_eq!(verdict["score"], 3);
}

#[test]
fn missing_fixture_fails_before_provider_call() {
    let output = Command::new(env!("CARGO_BIN_EXE_sideeye"))
        .args([
            "review",
            "--provider",
            "fake",
            fixture("review-packet.json").to_str().unwrap(),
        ])
        .output()
        .expect("run sideeye");

    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("--fixture is required"));
}

#[test]
fn unsupported_provider_fails_closed() {
    let output = Command::new(env!("CARGO_BIN_EXE_sideeye"))
        .args([
            "review",
            "--provider",
            "openai",
            fixture("review-packet.json").to_str().unwrap(),
        ])
        .output()
        .expect("run sideeye");

    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("not implemented"));
}
