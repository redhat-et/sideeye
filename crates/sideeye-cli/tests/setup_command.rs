use std::fs;
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
