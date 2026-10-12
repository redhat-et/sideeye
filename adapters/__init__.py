"""Capture adapters — each turns some capture source into a SessionTranscript.

Adapters (this POC, both client-side scaffolds):
  - codex_rollout  : the Codex CLI's session rollout logs
  - claude_code    : the Claude Code session transcripts
Production adapter #2 would be a praxis gateway sampling-tap emitting events.
All produce the same SessionTranscript, so everything downstream is
capture-agnostic.
"""
from __future__ import annotations

import json
import os
import pathlib

from sideeye.adapters import claude_code, codex_rollout, opencode

CLAUDE_DIR = pathlib.Path.home() / ".claude" / "projects"
CODEX_DIR = pathlib.Path.home() / ".codex" / "sessions"


def claude_project_dir(cwd=None):
    """The ~/.claude/projects/<mangled-cwd> dir for a given cwd (default: os.getcwd).

    Claude Code mangles the cwd into a dirname by replacing '/' with '-'
    (e.g. /Users/x/proj -> -Users-x-proj). One project dir per cwd.
    """
    cwd = cwd or os.getcwd()
    return CLAUDE_DIR / cwd.replace("/", "-")


def load_transcript(path):
    """Auto-detect the capture source from the file and parse it."""
    path = pathlib.Path(path)
    with open(path, encoding="utf-8") as f:
        for _ in range(10):
            line = f.readline()
            if not line:
                break
            line = line.strip()
            if not line:
                continue
            try:
                obj = json.loads(line)
            except json.JSONDecodeError:
                continue
            t = obj.get("type")
            if t in ("session_meta", "turn_context", "response_item"):
                return codex_rollout.parse_rollout(path)
            if t in ("user", "assistant") and "message" in obj:
                return claude_code.parse_session(path)
    # Fallback: try both.
    return codex_rollout.parse_rollout(path) or claude_code.parse_session(path)


def session_files(client, scope="all"):
    """List candidate session files for a client, oldest -> newest by mtime.

    scope:
      "all"     — every project (claude) / every session (codex). Default, so the
                  sampler still scans across projects unchanged.
      "cwd"     — claude only: only the current working directory's project dir.
                  Use this for "the session I'm in", not the global latest (which
                  is a coin flip across all live projects).
      <string>  — a specific project dirname under ~/.claude/projects/, or a path.
    """
    if client == "codex":
        files = CODEX_DIR.rglob("rollout-*.jsonl")
        return sorted(files, key=lambda p: p.stat().st_mtime)
    if client != "claude":
        raise ValueError(f"unknown client: {client!r} (use 'claude' or 'codex')")

    if scope == "all":
        files = CLAUDE_DIR.glob("*/*.jsonl")
    elif scope == "cwd":
        pdir = claude_project_dir()
        files = pdir.glob("*.jsonl") if pdir.exists() else []
    else:
        pdir = pathlib.Path(scope)
        if not pdir.is_absolute():
            pdir = CLAUDE_DIR / scope
        files = pdir.glob("*.jsonl") if pdir.exists() else []
    # Top-level session files only — skip nested subagents/ transcripts, which
    # live at <project>/<session-uuid>/subagents/*.jsonl.
    return sorted(
        (p for p in files if "subagents" not in p.parts),
        key=lambda p: p.stat().st_mtime,
    )


def latest_session(client, scope="all"):
    files = session_files(client, scope)
    return files[-1] if files else None


def resolve_current_session(client):
    """Best-effort "the session I'm in", robust to a drifted shell cwd.

    When Claude Code's own session ID is exported (CLAUDE_CODE_SESSION_ID, set
    for skill/tool shell calls), use it directly — session ids are UUIDs, so a
    global lookup is unambiguous, deterministic, and immune to a sibling
    session in the same project dir having written more recently (the silent
    wrong-session race). Otherwise: try the current project (cwd) first —
    precise when the shell is actually in the session's dir. Then walk up
    parent directories (like git finding .git), so running from
    sideeye/calibration still finds the praxis-ai project session. Only falls
    back to "globally most recent" if no ancestor matches.

    Returns (path_or_None, fell_back_bool) so the caller can note the fallback.
    Note: if the env id is set but its transcript is not on disk, resolution
    silently degrades to the mtime heuristic — the CALLER should surface that
    (it knows the env id and the resolved path)."""
    if client == "claude":
        env_id = os.environ.get("CLAUDE_CODE_SESSION_ID")
        if env_id:
            hits = sorted(CLAUDE_DIR.glob(f"*/{env_id}.jsonl")) if CLAUDE_DIR.exists() else []
            if hits:
                return hits[0], False
    s = latest_session(client, scope="cwd")
    if s:
        return s, False
    # Walk up: sideeye/calibration -> sideeye -> praxis-ai -> ...
    cwd = pathlib.Path.cwd()
    for parent in cwd.parents:
        pdir = claude_project_dir(str(parent))
        if pdir.exists():
            files = session_files(client, scope=str(pdir))
            if files:
                return files[-1], False
    return latest_session(client, scope="all"), True
