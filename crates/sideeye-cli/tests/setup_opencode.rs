use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

fn bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_sideeye"))
}

#[cfg(unix)]
fn fake_bin(directory: &Path, name: &str, code: i32) {
    let path = directory.join(name);
    fs::write(&path, format!("#!/bin/sh\nexit {code}\n")).expect("write fake binary");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).expect("chmod fake binary");
}

#[test]
#[cfg(unix)]
fn setup_installs_the_opencode_plugin_and_preserves_jsonc() {
    let dir = tempfile::tempdir().expect("tempdir");
    let config = dir.path().join("sideeye.json");
    let opencode = dir.path().join("opencode.jsonc");
    fs::write(
        &opencode,
        "{\n  // keep this\n  \"model\": \"provider/model\"\n}\n",
    )
    .expect("seed opencode");
    let fakebin = dir.path().join("bin");
    fs::create_dir_all(&fakebin).expect("fakebin");
    fake_bin(&fakebin, "npm", 0);
    fake_bin(&fakebin, "opencode", 0);
    fake_bin(&fakebin, "git", 0);

    let binary_dir = bin().parent().unwrap().to_path_buf();
    let path = std::env::join_paths([binary_dir, fakebin.clone()])
        .expect("join paths")
        .to_string_lossy()
        .into_owned();
    let data = dir.path().join("data");

    let output = Command::new(bin())
        .env("SIDEEYE_CONFIG", &config)
        .env("HOME", dir.path().join("home"))
        .env("SIDEEYE_JUDGE_API_KEY", "test-key")
        .env("SIDEEYE_DATA_DIR", &data)
        .env("OPENCODE_CONFIG", &opencode)
        .env("PATH", &path)
        .env_remove("SIDEEYE_JUDGE_BASE_URL")
        .args([
            "setup",
            "--judge-base-url",
            "https://judge.example/v1",
            "--repo",
            env!("CARGO_MANIFEST_DIR"),
            "--model",
            "claude-opus-5-5",
            "--max-tokens",
            "4096",
        ])
        .output()
        .expect("run setup");

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let saved = fs::read_to_string(&opencode).expect("read opencode");
    assert!(saved.contains("// keep this"));
    let parsed: serde_json::Value = serde_json::from_str(
        &saved
            .lines()
            .map(|line| line.split("//").next().unwrap_or(""))
            .collect::<Vec<_>>()
            .join("\n"),
    )
    .expect("jsonc");
    assert_eq!(parsed["model"], "provider/model");
    let plugin = &parsed["plugins"][0];
    assert_eq!(
        plugin["package"],
        data.join("opencode-plugin").to_string_lossy().as_ref()
    );
    assert_eq!(plugin["options"]["engine"], "rust");
    assert_eq!(plugin["options"]["repo"], env!("CARGO_MANIFEST_DIR"));
    assert!(plugin["options"].get("command").is_none());
    assert_eq!(plugin["options"]["model"], "claude-opus-5-5");
    assert_eq!(plugin["options"]["maxTokens"], 4096);
    assert_eq!(plugin["options"]["provider"], "anthropic");
    assert!(data.join("opencode-plugin/src/index.ts").is_file());
    assert!(data.join("opencode-plugin/package.json").is_file());

    let cli: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(dir.path().join("cli.json")).expect("cli"))
            .expect("cli json");
    assert_eq!(cli["plugins"][0]["options"]["engine"], "rust");
}

#[test]
fn version_is_available() {
    let output = Command::new(bin())
        .arg("--version")
        .output()
        .expect("run version");
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).starts_with("sideeye "));
}

#[test]
#[cfg(unix)]
fn setup_leaves_config_untouched_when_the_plugin_build_fails() {
    let dir = tempfile::tempdir().expect("tempdir");
    let config = dir.path().join("sideeye.json");
    let opencode = dir.path().join("opencode.jsonc");
    let original = "{\n  // keep this\n  \"model\": \"provider/model\"\n}\n";
    fs::write(&opencode, original).expect("seed opencode");
    let fakebin = dir.path().join("bin");
    fs::create_dir_all(&fakebin).expect("fakebin");
    fake_bin(&fakebin, "npm", 1);
    fake_bin(&fakebin, "opencode", 0);
    fake_bin(&fakebin, "git", 0);

    let binary_dir = bin().parent().unwrap().to_path_buf();
    let path = std::env::join_paths([binary_dir, fakebin])
        .expect("join paths")
        .to_string_lossy()
        .into_owned();

    let output = Command::new(bin())
        .env("SIDEEYE_CONFIG", &config)
        .env("HOME", dir.path().join("home"))
        .env("SIDEEYE_JUDGE_API_KEY", "test-key")
        .env("SIDEEYE_DATA_DIR", dir.path().join("data"))
        .env("OPENCODE_CONFIG", &opencode)
        .env("PATH", &path)
        .env_remove("SIDEEYE_JUDGE_BASE_URL")
        .args(["setup", "--repo", env!("CARGO_MANIFEST_DIR")])
        .output()
        .expect("run setup");

    assert!(!output.status.success());
    assert_eq!(fs::read_to_string(&opencode).expect("opencode"), original);
}

#[test]
#[cfg(unix)]
fn setup_installs_claude_and_codex_skills_in_the_user_home() {
    let dir = tempfile::tempdir().expect("tempdir");
    let config = dir.path().join("sideeye.json");
    let home = dir.path().join("home");
    let fakebin = dir.path().join("bin");
    fs::create_dir_all(&fakebin).expect("fakebin");
    fake_bin(&fakebin, "claude", 0);
    fake_bin(&fakebin, "codex", 0);

    let binary_dir = bin().parent().unwrap().to_path_buf();
    let path = std::env::join_paths([binary_dir, fakebin])
        .expect("join paths")
        .to_string_lossy()
        .into_owned();
    let output = Command::new(bin())
        .env("SIDEEYE_CONFIG", &config)
        .env("HOME", &home)
        .env("SIDEEYE_JUDGE_API_KEY", "test-key")
        .env("PATH", &path)
        .env_remove("SIDEEYE_JUDGE_BASE_URL")
        .env_remove("ANTHROPIC_BASE_URL")
        .args([
            "setup",
            "--judge-base-url",
            "https://judge.example",
            "--no-opencode",
            "--yes",
        ])
        .output()
        .expect("run setup");
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let claude =
        fs::read_to_string(home.join(".claude/skills/sideeye/SKILL.md")).expect("claude skill");
    assert!(claude.contains("sideeye session --client claude"));
    let codex =
        fs::read_to_string(home.join(".agents/skills/sideeye/SKILL.md")).expect("codex skill");
    assert!(codex.contains("sideeye session --client codex"));
    assert!(
        home.join(".agents/skills/sideeye/agents/openai.yaml")
            .is_file()
    );
}
