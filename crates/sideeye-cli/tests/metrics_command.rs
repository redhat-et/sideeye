use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_sideeye"))
}

#[test]
fn labels_join_the_ledger_and_produce_metrics() {
    let dir = tempfile::tempdir().expect("tempdir");
    let ledger = dir.path().join("verdicts.jsonl");
    let labels = dir.path().join("labels.jsonl");
    fs::write(
        &ledger,
        concat!(
            r#"{"record_id":"r1","session_id":"s1","source":"opencode","verdict":{"cost_usd":0.1}}"#,
            "\n",
            r#"{"record_id":"r2","session_id":"s2","source":"opencode","verdict":{"cost_usd":0.2}}"#,
            "\n"
        ),
    )
    .expect("ledger");

    for (label, id) in [("confirmed", "r1"), ("false-positive", "r2")] {
        let output = Command::new(bin())
            .args([
                "label",
                label,
                "--id",
                id,
                "--ledger",
                ledger.to_str().unwrap(),
                "--labels",
                labels.to_str().unwrap(),
            ])
            .output()
            .expect("run label");
        assert!(
            output.status.success(),
            "stderr: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let output = Command::new(bin())
        .args([
            "stats",
            "--ledger",
            ledger.to_str().unwrap(),
            "--labels",
            labels.to_str().unwrap(),
            "--json",
        ])
        .output()
        .expect("run stats");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).expect("stats json");
    assert_eq!(report["total_verdicts"], 2);
    assert_eq!(report["labeled"], 2);
    assert_eq!(report["confirmed"], 1);
    assert_eq!(report["false_positive"], 1);
    assert_eq!(report["precision"], 0.5);
    assert_eq!(report["coverage"], 1.0);
    assert_eq!(report["total_cost_usd"], 0.3);
    assert_eq!(report["cost_per_confirmed_usd"], 0.3);
}

#[test]
fn unknown_labels_fail_closed() {
    let dir = tempfile::tempdir().expect("tempdir");
    let output = Command::new(bin())
        .args([
            "label",
            "maybe",
            "--ledger",
            dir.path().join("v.jsonl").to_str().unwrap(),
            "--labels",
            dir.path().join("l.jsonl").to_str().unwrap(),
        ])
        .output()
        .expect("run label");
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("unknown label"));
}
