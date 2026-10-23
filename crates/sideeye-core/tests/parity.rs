use sideeye_core::{Correctness, Severity, Verdict};

fn fixture(name: &str) -> Verdict {
    let path = format!(
        "{}/tests/fixtures/parity/{name}",
        env!("CARGO_MANIFEST_DIR")
    );
    serde_json::from_str(&std::fs::read_to_string(path).expect("read fixture"))
        .expect("parse verdict fixture")
}

#[test]
fn python_and_rust_agree_on_semantic_fields_for_live_defect_case() {
    let python = fixture("python-defect-verdict.json");
    let rust = fixture("rust-defect-verdict.json");

    assert_eq!(python.answered_what_was_asked, rust.answered_what_was_asked);
    assert_eq!(python.correctness, rust.correctness);
    assert_eq!(python.claims_supported, rust.claims_supported);
    assert_eq!(python.score, rust.score);
    assert_eq!(python.overall_severity, rust.overall_severity);

    assert_eq!(rust.correctness, Correctness::Incorrect);
    assert_eq!(rust.overall_severity, Severity::Critical);
    assert!(!rust.claims_supported);
}
