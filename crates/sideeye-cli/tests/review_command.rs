use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::process::Command;

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../sideeye-core/tests/fixtures")
        .join(name)
}

#[test]
fn fake_provider_executes_packet_and_emits_provenance() {
    let dir = tempfile::tempdir().expect("tempdir");
    let out = dir.path().join("verdicts.jsonl");
    let output = Command::new(env!("CARGO_BIN_EXE_sideeye"))
        .args([
            "review",
            "--provider",
            "fake",
            "--fixture",
            fixture("python-verdict.json").to_str().unwrap(),
            "--out",
            out.to_str().unwrap(),
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
    let saved = std::fs::read_to_string(out).expect("out file");
    let record: serde_json::Value = serde_json::from_str(saved.trim()).expect("record");
    assert_eq!(record["session_id"], "session-golden-1");
    assert_eq!(record["verdict"]["score"], 3);
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
fn openai_compatible_provider_parses_a_tool_call() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind fake judge");
    let address = listener.local_addr().expect("fake judge address");
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept review");
        let mut request = [0_u8; 16_384];
        let _ = stream.read(&mut request);
        let body = serde_json::json!({
            "choices": [{
                "finish_reason": "tool_calls",
                "message": {
                    "tool_calls": [{
                        "function": {
                            "name": "record_verdict",
                            "arguments": "{\"answered_what_was_asked\":true,\"correctness\":\"correct\",\"claims_supported\":true,\"score\":4,\"issues\":[],\"overall_severity\":\"minor\",\"summary\":\"openai fixture\"}"
                        }
                    }]
                }
            }],
            "usage": {"prompt_tokens": 10, "completion_tokens": 5}
        })
        .to_string();
        let response = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{}",
            body.len(),
            body
        );
        stream
            .write_all(response.as_bytes())
            .expect("write response");
    });

    let base = format!("http://{address}");
    let output = Command::new(env!("CARGO_BIN_EXE_sideeye"))
        .env("SIDEEYE_JUDGE_API_KEY", "test-key")
        .env_remove("SIDEEYE_JUDGE_BASE_URL")
        .env_remove("ANTHROPIC_BASE_URL")
        .args([
            "review",
            "--provider",
            "openai-compatible",
            "--base-url",
            &base,
            "--model",
            "local-test",
            fixture("review-packet.json").to_str().unwrap(),
        ])
        .output()
        .expect("run sideeye");
    server.join().expect("fake judge thread");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let verdict: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("verdict is JSON");
    assert_eq!(verdict["judge_provider"], "openai-compatible");
    assert_eq!(verdict["judge_model"], "local-test");
    assert_eq!(verdict["score"], 4);
}

#[test]
fn unsupported_provider_fails_closed() {
    let output = Command::new(env!("CARGO_BIN_EXE_sideeye"))
        .args([
            "review",
            "--provider",
            "cohere",
            fixture("review-packet.json").to_str().unwrap(),
        ])
        .output()
        .expect("run sideeye");

    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("not implemented"));
}
