import json
import os
import stat

from sideeye import configure as setup


def test_setup_writes_judge_config_securely_and_preserves_settings(monkeypatch, tmp_path):
    config = tmp_path / "sideeye.json"
    config.write_text(json.dumps({"existing": {"keep": True}}))
    monkeypatch.setenv("SIDEEYE_CONFIG", str(config))
    monkeypatch.setenv("SIDEEYE_JUDGE_API_KEY", "test-key")
    monkeypatch.setattr(setup, "detect_harnesses", lambda: {"Claude Code": False, "Codex": False, "OpenCode": False})

    assert setup.main(["--no-opencode"]) == 0

    saved = json.loads(config.read_text())
    assert saved["existing"] == {"keep": True}
    assert saved["judge"]["api_key"] == "test-key"
    assert stat.S_IMODE(config.stat().st_mode) == 0o600


def test_setup_requires_a_credential_in_noninteractive_mode(monkeypatch, tmp_path):
    monkeypatch.setenv("SIDEEYE_CONFIG", str(tmp_path / "sideeye.json"))
    monkeypatch.delenv("SIDEEYE_JUDGE_API_KEY", raising=False)
    monkeypatch.setattr(setup.sys.stdin, "isatty", lambda: False)

    assert setup.main(["--no-opencode"]) == 1
    assert not (tmp_path / "sideeye.json").exists()


def test_setup_updates_existing_jsonc_without_dropping_comments(monkeypatch, tmp_path):
    config = tmp_path / "opencode.jsonc"
    config.write_text('{\n  // keep this setting\n  "model": "provider/model"\n}\n')
    monkeypatch.setattr(setup, "_opencode_config_file", lambda: config)
    monkeypatch.setenv("SIDEEYE_CONFIG", str(tmp_path / "sideeye.json"))
    monkeypatch.setenv("SIDEEYE_JUDGE_API_KEY", "test-key")
    source = tmp_path / "plugin"
    source.mkdir()
    (source / "package.json").write_text("{}")
    monkeypatch.setattr(setup, "_plugin_source", lambda: source)
    monkeypatch.setattr(setup, "_which", lambda name: "/usr/bin/npm" if name == "npm" else "/usr/local/bin/sideeye")
    monkeypatch.setattr(setup.subprocess, "run", lambda *args, **kwargs: type("Result", (), {"returncode": 0, "stdout": "", "stderr": ""})())
    monkeypatch.setattr(setup, "detect_harnesses", lambda: {"Claude Code": False, "Codex": False, "OpenCode": True})

    assert setup.main([]) == 0
    saved = config.read_text()
    assert "keep this setting" in saved
    assert '"plugins"' in saved
    assert "opencode-plugin" in saved
