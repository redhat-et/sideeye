"""One-command Side-Eye setup.

Setup is deliberately conservative: it writes only Side-Eye's judge config and
the OpenCode plugin entry, preserves unrelated JSON settings, and never prints
credentials. Harnesses that do not yet have a native adapter are reported as
pending instead of being presented as ready.
"""
from __future__ import annotations

import argparse
import getpass
import json
import os
import pathlib
import shutil
import stat
import subprocess
import sys
import tempfile

from sideeye.config import AMBIENT_BASE_DEFAULT, config_path, load_config


class SetupError(RuntimeError):
    pass


def _which(name: str) -> str | None:
    return shutil.which(name)


def _opencode_config_file() -> pathlib.Path:
    explicit = os.environ.get("OPENCODE_CONFIG")
    if explicit:
        return pathlib.Path(explicit).expanduser()
    command = _which("opencode")
    if command:
        try:
            result = subprocess.run(
                [command, "debug", "paths", "config"],
                capture_output=True, text=True, timeout=20,
            )
            if result.returncode == 0 and result.stdout.strip():
                return pathlib.Path(result.stdout.strip()).expanduser() / "opencode.json"
        except (OSError, subprocess.SubprocessError):
            pass
    config_dir = os.environ.get("OPENCODE_CONFIG_DIR")
    if config_dir:
        return pathlib.Path(config_dir).expanduser() / "opencode.json"
    return pathlib.Path.home() / ".config" / "opencode" / "opencode.json"


def _write_json_secure(path: pathlib.Path, data: dict) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    old_mode = stat.S_IMODE(path.stat().st_mode) if path.exists() else 0o600
    fd, temporary = tempfile.mkstemp(prefix=f".{path.name}.", dir=path.parent, text=True)
    try:
        os.fchmod(fd, old_mode & 0o600 or 0o600)
        with os.fdopen(fd, "w", encoding="utf-8") as handle:
            json.dump(data, handle, indent=2)
            handle.write("\n")
        os.replace(temporary, path)
        os.chmod(path, old_mode & 0o600 or 0o600)
    except Exception:
        try:
            os.unlink(temporary)
        except FileNotFoundError:
            pass
        raise


def _configure_judge(*, base_url: str | None, api_key: str | None) -> pathlib.Path:
    path = config_path()
    config = load_config(path)
    judge = dict(config.get("judge") or {})
    existing_key = judge.get("api_key")
    key = api_key or os.environ.get("SIDEEYE_JUDGE_API_KEY") or existing_key
    if not key:
        if not sys.stdin.isatty():
            raise SetupError(
                "no judge credential configured; set SIDEEYE_JUDGE_API_KEY or run "
                "sideeye setup in an interactive terminal"
            )
        key = getpass.getpass("Side-Eye judge API key (hidden): ").strip()
    if not key:
        raise SetupError("judge API key cannot be empty")
    judge["api_key"] = key
    selected_base = base_url or os.environ.get("SIDEEYE_JUDGE_BASE_URL") or judge.get("base_url")
    if selected_base:
        judge["base_url"] = selected_base
    elif "base_url" in judge:
        judge.pop("base_url")
    config["judge"] = judge
    _write_json_secure(path, config)
    return path


def _plugin_source() -> pathlib.Path | None:
    source = pathlib.Path(__file__).resolve().parent / "integrations" / "opencode"
    return source if source.is_dir() else None


def _install_opencode_plugin() -> tuple[str, pathlib.Path | None]:
    source = _plugin_source()
    if source is None:
        return "pending (plugin source is not included in this installation)", None
    npm = _which("npm")
    if npm is None:
        return "pending (npm is required to build the OpenCode plugin)", None
    data_root = pathlib.Path(
        os.environ.get("SIDEEYE_DATA_DIR")
        or pathlib.Path.home() / ".local" / "share" / "sideeye"
    ).expanduser()
    destination = data_root / "opencode-plugin"
    shutil.copytree(
        source,
        destination,
        dirs_exist_ok=True,
        ignore=shutil.ignore_patterns("node_modules", ".git", "tests", "*.map"),
    )
    for command in ([npm, "ci", "--ignore-scripts"], [npm, "run", "build"]):
        result = subprocess.run(command, cwd=destination, capture_output=True, text=True, timeout=180)
        if result.returncode != 0:
            raise SetupError(
                f"OpenCode plugin build failed: {' '.join(command)}\n"
                f"{(result.stderr or result.stdout).strip()}"
            )

    config_path_value = _opencode_config_file()
    if config_path_value.exists():
        try:
            opencode_config = json.loads(config_path_value.read_text(encoding="utf-8"))
        except json.JSONDecodeError as exc:
            raise SetupError(
                f"OpenCode config is not plain JSON ({config_path_value}); "
                "setup will not rewrite JSONC automatically"
            ) from exc
    else:
        opencode_config = {"$schema": "https://opencode.ai/config.json"}
    plugins = list(opencode_config.get("plugins") or [])
    entry = {"package": str(destination), "options": {"command": [_which("sideeye") or "sideeye"]}}
    if not any(
        (item == str(destination))
        or (isinstance(item, dict) and item.get("package") == str(destination))
        for item in plugins
    ):
        plugins.append(entry)
    opencode_config["plugins"] = plugins
    _write_json_secure(config_path_value, opencode_config)
    return "configured", config_path_value


def detect_harnesses() -> dict[str, bool]:
    return {
        "Claude Code": bool(_which("claude") or (pathlib.Path.home() / ".claude").exists()),
        "Codex": bool(_which("codex") or (pathlib.Path.home() / ".codex").exists()),
        "OpenCode": bool(_which("opencode")),
    }


def main(argv=None) -> int:
    parser = argparse.ArgumentParser(description="Configure Side-Eye and detected harness integrations")
    parser.add_argument("--judge-base-url", default=None)
    parser.add_argument("--judge-api-key", default=None, help=argparse.SUPPRESS)
    parser.add_argument("--no-opencode", action="store_true", help="do not configure the OpenCode plugin")
    args = parser.parse_args(argv)
    try:
        judge_path = _configure_judge(base_url=args.judge_base_url, api_key=args.judge_api_key)
        print(f"judge route: configured ({judge_path})")
        for name, detected in detect_harnesses().items():
            print(f"{name}: {'detected' if detected else 'not detected'}")
        if args.no_opencode or not detect_harnesses()["OpenCode"]:
            print("OpenCode plugin: skipped")
        else:
            state, path = _install_opencode_plugin()
            print(f"OpenCode plugin: {state}" + (f" ({path})" if path else ""))
        print("Run `sideeye route` to verify the judge guard without spending.")
        return 0
    except SetupError as exc:
        print(f"sideeye setup: {exc}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
