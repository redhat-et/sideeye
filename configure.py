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
import re
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


def _normalize_judge_base_url(value: str) -> str:
    value = value.rstrip("/")
    return value[:-3] if value.endswith("/v1") else value


def _git_root(path: pathlib.Path | None = None) -> pathlib.Path | None:
    root = path or pathlib.Path.cwd()
    try:
        result = subprocess.run(
            ["git", "-C", str(root), "rev-parse", "--show-toplevel"],
            capture_output=True, text=True, timeout=20,
        )
    except (OSError, subprocess.SubprocessError):
        return None
    if result.returncode != 0 or not result.stdout.strip():
        return None
    return pathlib.Path(result.stdout.strip()).resolve()


def _opencode_config_file() -> pathlib.Path:
    explicit = os.environ.get("OPENCODE_CONFIG")
    if explicit:
        return pathlib.Path(explicit).expanduser()
    command = _which("opencode")
    if command:
        try:
            command_path = pathlib.Path(command)
            debug_command = [command, "debug", "paths", "config"]
            debug_env = None
            # Some local launchers prepend --standalone and select a private
            # XDG profile. Ask the real binary directly so setup writes to the
            # profile the launcher actually uses.
            if command_path.is_file() and command_path.read_text(errors="ignore").find("ENMAAS_OPENCODE_CONFIG_HOME") >= 0:
                wrapper = command_path.read_text(errors="ignore")
                match = re.search(r"XDG_CONFIG_HOME=\"\$\{ENMAAS_OPENCODE_CONFIG_HOME:-\$HOME/([^}\"]+)\}\"", wrapper)
                default_home = pathlib.Path.home() / (match.group(1) if match else ".config/opencode")
                config_home = os.environ.get("ENMAAS_OPENCODE_CONFIG_HOME") or str(default_home)
                real_binary = os.environ.get("OPENCODE_REAL_BIN") or "/opt/homebrew/bin/opencode"
                debug_command = [real_binary, "debug", "paths", "config"]
                debug_env = {**os.environ, "XDG_CONFIG_HOME": config_home}
            result = subprocess.run(
                debug_command,
                capture_output=True, text=True, timeout=20, env=debug_env,
            )
            if result.returncode == 0 and result.stdout.strip():
                directory = pathlib.Path(result.stdout.strip()).expanduser()
                for name in ("opencode.jsonc", "opencode.json"):
                    candidate = directory / name
                    if candidate.exists():
                        return candidate
                return directory / "opencode.json"
        except (OSError, subprocess.SubprocessError):
            pass
    config_dir = os.environ.get("OPENCODE_CONFIG_DIR")
    if config_dir:
        directory = pathlib.Path(config_dir).expanduser()
    else:
        directory = pathlib.Path.home() / ".config" / "opencode"
    for name in ("opencode.jsonc", "opencode.json"):
        candidate = directory / name
        if candidate.exists():
            return candidate
    return directory / "opencode.json"


def _opencode_cli_config_file() -> pathlib.Path:
    return _opencode_config_file().parent / "cli.json"


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


def _write_text_secure(path: pathlib.Path, text: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    old_mode = stat.S_IMODE(path.stat().st_mode) if path.exists() else 0o600
    fd, temporary = tempfile.mkstemp(prefix=f".{path.name}.", dir=path.parent, text=True)
    try:
        os.fchmod(fd, old_mode & 0o600 or 0o600)
        with os.fdopen(fd, "w", encoding="utf-8") as handle:
            handle.write(text)
        os.replace(temporary, path)
        os.chmod(path, old_mode & 0o600 or 0o600)
    except Exception:
        try:
            os.unlink(temporary)
        except FileNotFoundError:
            pass
        raise


def _jsonc_insert_plugins(text: str, entry: dict) -> str:
    """Add one plugin entry to a JSONC object without rewriting comments."""
    if "\"plugins\"" in text and str(entry["package"]) in text:
        return text
    encoded = json.dumps(entry, indent=2)
    if "\"plugins\"" not in text:
        close = _last_object_close(text)
        if close is None:
            raise SetupError("OpenCode JSONC config has no root object")
        before = text[:close].rstrip()
        separator = "," if before and not before.endswith(",") else ""
        addition = f'{separator}\n  "plugins": [\n{_indent(encoded, 4)}\n  ]\n'
        return text[:len(before)] + addition + text[close:]

    start = text.find("\"plugins\"")
    opening = text.find("[", start)
    closing = _matching_close(text, opening, "[", "]") if opening >= 0 else None
    if opening < 0 or closing is None:
        raise SetupError("OpenCode JSONC plugins entry is not an array")
    before = text[opening + 1:closing].rstrip()
    separator = "," if before and not before.endswith(",") else ""
    addition = f'{separator}\n{_indent(encoded, 2)}\n'
    return text[:opening + 1] + before + addition + text[closing:]


def _indent(text: str, spaces: int) -> str:
    prefix = " " * spaces
    return "\n".join(prefix + line for line in text.splitlines())


def _matching_close(text: str, opening: int, left: str, right: str) -> int | None:
    depth = 0
    in_string = False
    escaped = False
    for index in range(opening, len(text)):
        char = text[index]
        if in_string:
            if escaped:
                escaped = False
            elif char == "\\":
                escaped = True
            elif char == '"':
                in_string = False
            continue
        if char == '"':
            in_string = True
        elif char == left:
            depth += 1
        elif char == right:
            depth -= 1
            if depth == 0:
                return index
    return None


def _last_object_close(text: str) -> int | None:
    depth = 0
    in_string = False
    escaped = False
    last = None
    for index, char in enumerate(text):
        if in_string:
            if escaped:
                escaped = False
            elif char == "\\":
                escaped = True
            elif char == '"':
                in_string = False
            continue
        if char == '"':
            in_string = True
        elif char == "{":
            depth += 1
        elif char == "}":
            depth -= 1
            if depth == 0:
                last = index
    return last


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
        judge["base_url"] = _normalize_judge_base_url(selected_base)
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
    command_options = {"command": [_which("sideeye") or "sideeye"]}
    rust = _which("sideeye-rs")
    if rust:
        # Prefer the native engine when it is installed. Python remains the
        # fallback for users who only have the Python package.
        command_options["engine"] = "rust"
        command_options["rust"] = [rust]
    repository = _git_root()
    if repository:
        command_options["repo"] = str(repository)
    entry = {"package": str(destination), "options": command_options}
    if config_path_value.exists():
        raw_config = config_path_value.read_text(encoding="utf-8")
        try:
            opencode_config = json.loads(raw_config)
        except json.JSONDecodeError as exc:
            _write_text_secure(config_path_value, _jsonc_insert_plugins(raw_config, entry))
            return "configured", config_path_value
    else:
        opencode_config = {"$schema": "https://opencode.ai/config.json"}
    plugins = list(opencode_config.get("plugins") or [])
    found = False
    for index, item in enumerate(plugins):
        if isinstance(item, dict) and item.get("package") == str(destination):
            plugins[index] = {**item, "options": {**(item.get("options") or {}), **command_options}}
            found = True
        elif item == str(destination):
            plugins[index] = entry
            found = True
    if not found:
        plugins.append(entry)
    opencode_config["plugins"] = plugins
    _write_json_secure(config_path_value, opencode_config)
    # OpenCode's TUI plugins are configured in cli.json. Keep the server and
    # TUI entries aligned so the slash alias is available after a restart.
    cli_path = _opencode_cli_config_file()
    cli_entry = entry
    if cli_path.exists():
        cli_raw = cli_path.read_text(encoding="utf-8")
        try:
            cli_config = json.loads(cli_raw)
        except json.JSONDecodeError:
            _write_text_secure(cli_path, _jsonc_insert_plugins(cli_raw, cli_entry))
        else:
            cli_plugins = list(cli_config.get("plugins") or [])
            found = False
            for index, item in enumerate(cli_plugins):
                if isinstance(item, dict) and item.get("package") == str(destination):
                    cli_plugins[index] = {**item, "options": {**(item.get("options") or {}), **command_options}}
                    found = True
                elif item == str(destination):
                    cli_plugins[index] = cli_entry
                    found = True
            if not found:
                cli_plugins.append(cli_entry)
            cli_config["plugins"] = cli_plugins
            _write_json_secure(cli_path, cli_config)
    else:
        _write_json_secure(cli_path, {"$schema": "https://opencode.ai/v2/cli.json", "plugins": [cli_entry]})
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
