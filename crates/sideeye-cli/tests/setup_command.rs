use std::fs;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::process::{Command, Stdio};

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

fn bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_sideeye"))
}

#[test]
fn setup_writes_secure_config_and_normalizes_base() {
    let dir = tempfile::tempdir().expect("tempdir");
    let config = dir.path().join("sideeye.json");
    let output = Command::new(bin())
        .env("SIDEEYE_CONFIG", &config)
        .env("SIDEEYE_JUDGE_API_KEY", "test-key")
        .env_remove("SIDEEYE_JUDGE_BASE_URL")
        .args([
            "setup",
            "--judge-base-url",
            "https://judge.example/v1",
            "--no-opencode",
        ])
        .output()
        .expect("run setup");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let saved: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&config).expect("read config")).expect("json");
    assert_eq!(saved["judge"]["api_key"], "test-key");
    assert_eq!(saved["judge"]["base_url"], "https://judge.example");
    #[cfg(unix)]
    assert_eq!(
        fs::metadata(&config).unwrap().permissions().mode() & 0o777,
        0o600
    );
}

#[test]
fn setup_preserves_existing_keys() {
    let dir = tempfile::tempdir().expect("tempdir");
    let config = dir.path().join("sideeye.json");
    fs::write(&config, r#"{"keep":true,"judge":{"model":"x"}}"#).expect("seed");
    let output = Command::new(bin())
        .env("SIDEEYE_CONFIG", &config)
        .env("SIDEEYE_JUDGE_API_KEY", "new-key")
        .env_remove("SIDEEYE_JUDGE_BASE_URL")
        .args(["setup", "--no-opencode"])
        .output()
        .expect("run setup");
    assert!(output.status.success());
    let saved: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&config).expect("read config")).expect("json");
    assert_eq!(saved["keep"], true);
    assert_eq!(saved["judge"]["model"], "x");
    assert_eq!(saved["judge"]["api_key"], "new-key");
}

#[test]
fn setup_verify_uses_the_free_token_count_endpoint() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind fake judge");
    let address = listener.local_addr().expect("fake judge address");
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept setup probe");
        let mut request = [0_u8; 8192];
        let _ = stream.read(&mut request);
        let body = r#"{"input_tokens":7}"#;
        let response = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{}",
            body.len(),
            body
        );
        stream
            .write_all(response.as_bytes())
            .expect("write probe response");
    });

    let dir = tempfile::tempdir().expect("tempdir");
    let config = dir.path().join("sideeye.json");
    let base = format!("http://{address}");
    let output = Command::new(bin())
        .env("SIDEEYE_CONFIG", &config)
        .env("SIDEEYE_JUDGE_API_KEY", "test-key")
        .env_remove("SIDEEYE_JUDGE_BASE_URL")
        .env_remove("ANTHROPIC_BASE_URL")
        .args([
            "setup",
            "--judge-base-url",
            &base,
            "--no-opencode",
            "--yes",
            "--verify",
        ])
        .output()
        .expect("run setup");
    server.join().expect("fake judge thread");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Free judge check: OK (7 input tokens"));
    let saved: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&config).expect("read config")).expect("json");
    assert_eq!(saved["judge"]["base_url"], format!("http://{address}"));
}

#[test]
fn setup_without_credentials_fails_closed_noninteractively() {
    let dir = tempfile::tempdir().expect("tempdir");
    let config = dir.path().join("sideeye.json");
    let output = Command::new(bin())
        .env("SIDEEYE_CONFIG", &config)
        .env_remove("SIDEEYE_JUDGE_API_KEY")
        .env_remove("SIDEEYE_JUDGE_BASE_URL")
        .stdin(Stdio::null())
        .args(["setup", "--no-opencode"])
        .output()
        .expect("run setup");
    assert!(!output.status.success());
    assert!(!config.exists());
}
