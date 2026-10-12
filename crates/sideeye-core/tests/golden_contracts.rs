use sideeye_core::{VERDICT_SCHEMA, Verdict};

#[test]
fn python_judge_fixture_round_trips_through_rust_contract() {
    let source = include_str!("fixtures/python-verdict.json");
    let verdict: Verdict = serde_json::from_str(source).expect("Python verdict fixture parses");
    verdict
        .validate()
        .expect("Python verdict fixture validates");
    assert_eq!(verdict.schema_version, VERDICT_SCHEMA);
    assert_eq!(verdict.issues.len(), 1);
    assert!(verdict.issues[0].evidence.is_empty());

    let encoded = serde_json::to_string(&verdict).expect("verdict serializes");
    let decoded: Verdict = serde_json::from_str(&encoded).expect("encoded verdict parses");
    assert_eq!(decoded, verdict);
}
