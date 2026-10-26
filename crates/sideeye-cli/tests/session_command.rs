use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::process::Command;

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures")
        .join(name)
}

fn fake_judge() -> (String, std::thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind fake judge");
    let address = listener.local_addr().expect("fake judge address");
    let handle = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept session review");
        let mut request = [0_u8; 16_384];
        let _ = stream.read(&mut request);
        let body = serde_json::json!({
            "choices": [{
                "finish_reason": "tool_calls",
                "message": {
                    "tool_calls": [{
                        "function": {
                            "name": "record_verdict",
                            "arguments": r#"{"answered_what_was_asked":true,"correctness":"correct","claims_supported":true,"score":4,"issues":[],"overall_severity":"minor","summary":"session fixture"}"#
                        }
                    }]
                }
            }],
            "usage": {"prompt_tokens": 10, "completion_tokens": 5}
        })
        .to_string();
        let response = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\nconnection: close\r\ncontent-length: {}\r\n\r\n{}",
            body.len(),
            body
        );
        stream
            .write_all(response.as_bytes())
            .expect("write response");
    });
    (format!("http://{address}"), handle)
}

fn fake_advisor() -> (String, std::thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind fake advisor");
    let address = listener.local_addr().expect("fake advisor address");
    let handle = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept advice");
        let mut request = [0_u8; 16_384];
        let _ = stream.read(&mut request);
        let body = serde_json::json!({
            "choices": [{
                "finish_reason": "stop",
                "message": {"content": "The recent change is sound; add one regression test."}
            }],
            "usage": {"prompt_tokens": 10, "completion_tokens": 8}
        })
        .to_string();
        let response = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\nconnection: close\r\ncontent-length: {}\r\n\r\n{}",
            body.len(),
            body
        );
        stream
            .write_all(response.as_bytes())
            .expect("write response");
    });
    (format!("http://{address}"), handle)
}

#[test]
fn advise_returns_a_free_form_second_opinion() {
    let (base, server) = fake_advisor();
    let output = Command::new(env!("CARGO_BIN_EXE_sideeye"))
        .env("SIDEEYE_JUDGE_API_KEY", "local")
        .env_remove("SIDEEYE_JUDGE_BASE_URL")
        .env_remove("ANTHROPIC_BASE_URL")
        .args([
            "advise",
            "--client",
            "claude",
            "--rollout",
            fixture("sample_claude_session.jsonl").to_str().unwrap(),
            "--question",
            "is the recent change sound?",
            "--provider",
            "openai-compatible",
            "--base-url",
            &base,
            "--model",
            "local-test",
            "--no-code",
            "--json",
        ])
        .output()
        .expect("run advise");
    server.join().expect("fake advisor thread");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let record: serde_json::Value = serde_json::from_slice(&output.stdout).expect("advice is JSON");
    assert_eq!(record["source"], "advice");
    assert_eq!(record["judge_provider"], "openai-compatible");
    assert!(
        record["advice"]
            .as_str()
            .unwrap()
            .contains("regression test")
    );
}

#[test]
fn claude_session_estimates_without_network_or_code() {
    let output = Command::new(env!("CARGO_BIN_EXE_sideeye"))
        .env("SIDEEYE_JUDGE_API_KEY", "local")
        .env_remove("SIDEEYE_JUDGE_BASE_URL")
        .env_remove("ANTHROPIC_BASE_URL")
        .args([
            "session",
            "--client",
            "claude",
            "--rollout",
            fixture("sample_claude_session.jsonl").to_str().unwrap(),
            "--estimate-only",
            "--provider",
            "openai-compatible",
            "--model",
            "local-test",
            "--no-code",
        ])
        .output()
        .expect("run session");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let estimate: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("estimate is JSON");
    assert_eq!(estimate["turns"], 3);
    assert_eq!(estimate["exact"], false);
    assert_eq!(estimate["cost_known"], false);
}

#[test]
fn codex_session_reviews_through_an_openai_compatible_route() {
    let (base, server) = fake_judge();
    let dir = tempfile::tempdir().expect("tempdir");
    let out = dir.path().join("verdicts.jsonl");
    let output = Command::new(env!("CARGO_BIN_EXE_sideeye"))
        .env("SIDEEYE_JUDGE_API_KEY", "local")
        .env_remove("SIDEEYE_JUDGE_BASE_URL")
        .env_remove("ANTHROPIC_BASE_URL")
        .args([
            "session",
            "--client",
            "codex",
            "--rollout",
            fixture("sample_rollout.jsonl").to_str().unwrap(),
            "--provider",
            "openai-compatible",
            "--base-url",
            &base,
            "--model",
            "local-test",
            "--no-code",
            "--out",
            out.to_str().unwrap(),
        ])
        .output()
        .expect("run session");
    server.join().expect("fake judge thread");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let verdict: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("verdict is JSON");
    assert_eq!(verdict["judge_provider"], "openai-compatible");
    assert_eq!(verdict["score"], 4);
    let saved = std::fs::read_to_string(out).expect("out file");
    assert!(saved.contains("session fixture"));
}
