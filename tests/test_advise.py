"""Unit tests for advise mode: last-exchange packet, free-form judge call,
judge-route env isolation. No network."""
from __future__ import annotations

import pathlib
import sys

import pytest

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[2]))

from sideeye.judge import judge as J  # noqa: E402
from sideeye.judge.transcript import last_exchange, make_transcript, recent_exchanges  # noqa: E402


def _t(turns):
    return make_transcript(session_id="s", source="claude_code", turns=turns)


# --- recent_exchanges: skip Side-Eye's own machinery (the self-capture bug) ---

_SKILL_BODY = ("Base directory for this skill: /Users/x/.claude/skills/escalate-last\n"
               "# /escalate-last — quick second opinion")


def _with_escalation(real_turns):
    # real exchange(s), then the /escalate-last machinery that trails them.
    return _t(real_turns + [
        {"role": "user", "text": _SKILL_BODY},
        {"role": "assistant", "text": '[ran: sideeye advise --current --yes --question ""]'},
        {"role": "tool", "text": "[tool_result: sideeye · advice mode\n  packet: last exchange"},
        {"role": "assistant", "text": "``` sideeye · advice mode\nSECOND OPINION\n..."},
    ])


def test_recent_exchanges_skips_its_own_invocation():
    t = _with_escalation([{"role": "user", "text": "SQLite or JSONL?"},
                          {"role": "assistant", "text": "I'd use SQLite."}])
    out = recent_exchanges(t, 1)
    assert "SQLite or JSONL?" in out and "I'd use SQLite." in out
    # None of the escalation machinery leaks into the packet.
    assert "Base directory for this skill" not in out
    assert "[ran: sideeye" not in out
    assert "sideeye · advice mode" not in out


def test_recent_exchanges_n_returns_multiple_real_exchanges():
    t = _with_escalation([
        {"role": "user", "text": "first ask"}, {"role": "assistant", "text": "first answer"},
        {"role": "user", "text": "second ask"}, {"role": "assistant", "text": "second answer"},
    ])
    out2 = recent_exchanges(t, 2)
    assert "first ask" in out2 and "second ask" in out2      # both real exchanges
    out1 = recent_exchanges(t, 1)
    assert "second ask" in out1 and "first ask" not in out1  # just the last


def test_recent_exchanges_includes_tool_results_when_sighted():
    t = _t([
        {"role": "user", "text": "fix the bug"},
        {"role": "assistant", "text": "on it [Edit: /repo/x.py]"},
        {"role": "tool", "text": "[tool_result: applied edit, 3 lines changed]"},
        {"role": "assistant", "text": "done"},
    ])
    light = recent_exchanges(t, 1)                       # default: no tool dumps
    sighted = recent_exchanges(t, 1, include_tools=True)  # evidence included
    assert "applied edit" not in light
    assert "applied edit" in sighted and "fix the bug" in sighted


def test_edit_ref_regex_scopes_the_diff_to_recent_files():
    # advise scopes the diff to files edited in the shown exchanges via the
    # adapter's [Edit: <path>] refs — verify the extraction that drives it.
    from sideeye.advise import _EDIT_REF
    text = ("ASSISTANT:\non it [Edit: /repo/a.py] and [Write: /repo/b.py]\n"
            "also [MultiEdit: /repo/c.py] but not [ran: pytest] or [tool_use: Read]")
    assert set(_EDIT_REF.findall(text)) == {"/repo/a.py", "/repo/b.py", "/repo/c.py"}


def test_human_question_quoting_escalate_output_is_not_skipped():
    # A human pasting advice output into their own short question must stay
    # reviewable — role-aware detection means only the injected skill body is machinery.
    t = _t([{"role": "user",
             "text": "is this expected?\n``` sideeye · advice mode\nSECOND OPINION\n..."},
            {"role": "assistant", "text": "Yes, that's the known bug."}])
    out = recent_exchanges(t, 1)
    assert "is this expected?" in out and "known bug" in out


# --- last_exchange: the light packet -------------------------------------

def test_last_exchange_picks_last_user_and_assistant():
    t = _t([
        {"role": "user", "text": "first ask"},
        {"role": "assistant", "text": "first answer"},
        {"role": "user", "text": "SQLite or JSONL?"},
        {"role": "assistant", "text": "I'd use SQLite."},
    ])
    out = last_exchange(t)
    # Only the LAST exchange, not the whole transcript.
    assert "SQLite or JSONL?" in out and "I'd use SQLite." in out
    assert "first ask" not in out and "first answer" not in out


def test_last_exchange_user_only():
    t = _t([{"role": "user", "text": "just a question"}])
    out = last_exchange(t)
    assert "just a question" in out and "ASSISTANT" not in out


def test_last_exchange_falls_back_when_no_clean_turns():
    # tool-only tail still yields something (render fallback), never empty.
    t = _t([{"role": "user", "text": "q"}, {"role": "tool", "text": "toolout"}])
    assert last_exchange(t).strip()


# --- build_advice_body: free-form, no forced tool ------------------------

def test_build_advice_body_has_no_tools_and_carries_packet():
    body = J.build_advice_body("ADVICE-RUBRIC", "which one?", "USER:\nx\n\nASSISTANT:\ny")
    assert body["system"] == "ADVICE-RUBRIC"
    assert "tools" not in body and "tool_choice" not in body   # free-form, not a verdict
    user = body["messages"][0]["content"]
    assert "which one?" in user and "USER:" in user


def test_build_advice_body_omits_question_when_empty():
    body = J.build_advice_body("r", "", "the exchange")
    assert "the exchange" in body["messages"][0]["content"]
    assert "question I want your opinion" not in body["messages"][0]["content"]


def test_build_advice_body_budget_covers_thinking():
    # 2026-09-03 incident: at the 1024 global default, fable-5 spent the whole
    # budget on a thinking block and emitted zero text (stop_reason=max_tokens,
    # thinking-only content) — advice mode died on every call.
    body = J.build_advice_body("r", "q", "packet")
    assert body["max_tokens"] == J.ADVICE_MAX_TOKENS >= 4096


def test_estimate_cost_follows_body_budget(monkeypatch):
    # The estimate must price the budget the body actually carries, not the
    # function default — otherwise advice estimates understate ~8x.
    monkeypatch.setattr(J, "count_tokens", lambda *a, **k: (1000, True, ""))
    body = {"model": "claude-fable-5", "max_tokens": J.ADVICE_MAX_TOKENS}
    _, est, exact, _ = J.estimate_cost("http://x", "k", body, "claude-fable-5")
    assert exact
    # 1000 in @ $10/M + (8192//2) out @ $50/M
    assert est == pytest.approx(round(1000 * 10.0 / 1e6 + 4096 * 50.0 / 1e6, 6))


# --- advise(): text + cost meta ------------------------------------------

def _text_response(text, usage=None):
    return {"content": [{"type": "thinking", "thinking": "hmm"},
                        {"type": "text", "text": text}],
            "usage": usage or {"input_tokens": 1200, "output_tokens": 90}}


def test_advise_returns_text_and_costed_meta(monkeypatch):
    monkeypatch.setattr(J, "call_judge", lambda *a, **k: _text_response("Recommendation: JSONL."))
    text, meta = J.advise("q", "packet", "rubric", base_url="http://x", api_key="k",
                          model="claude-fable-5")
    assert text == "Recommendation: JSONL."          # thinking block stripped
    assert meta["retries"] == 0
    # Fable = $10/M in, $50/M out: 1200*10/M + 90*50/M
    assert meta["cost_usd"] == pytest.approx(round(1200 * 10.0 / 1e6 + 90 * 50.0 / 1e6, 6))


def test_advise_raises_on_empty_text(monkeypatch):
    monkeypatch.setattr(J, "call_judge", lambda *a, **k: {"content": [], "usage": {}})
    with pytest.raises(ValueError):
        J.advise("q", "packet", "rubric", base_url="http://x", api_key="k")


# Empty-text failures must name themselves: a refusal, a truncation, and a
# thinking-only completion are three different bugs with three different
# fixes — one opaque message made them a mystery (live dogfood 2026-09-03:
# 'judge returned no text' on a 106k-token packet, cause undiagnosable).

def test_advise_error_names_refusal(monkeypatch):
    monkeypatch.setattr(J, "call_judge", lambda *a, **k: {
        "content": [], "stop_reason": "refusal",
        "usage": {"input_tokens": 106000, "output_tokens": 0}})
    with pytest.raises(ValueError, match=r"stop_reason=refusal.*content policy"):
        J.advise("q", "packet", "rubric", base_url="http://x", api_key="k")


def test_advise_error_names_truncation(monkeypatch):
    monkeypatch.setattr(J, "call_judge", lambda *a, **k: {
        "content": [{"type": "thinking", "thinking": "..."}],
        "stop_reason": "max_tokens", "usage": {"output_tokens": 4096}})
    with pytest.raises(ValueError, match=r"stop_reason=max_tokens.*raise max_tokens"):
        J.advise("q", "packet", "rubric", base_url="http://x", api_key="k")


def test_advise_error_names_thinking_only(monkeypatch):
    monkeypatch.setattr(J, "call_judge", lambda *a, **k: {
        "content": [{"type": "thinking", "thinking": "lots of it"}],
        "stop_reason": "end_turn", "usage": {"output_tokens": 900}})
    with pytest.raises(ValueError, match=r"thinking-only"):
        J.advise("q", "packet", "rubric", base_url="http://x", api_key="k")


def test_extract_text_skips_thinking_blocks():
    resp = {"content": [{"type": "thinking", "thinking": "secret"},
                        {"type": "text", "text": "visible"}]}
    assert J._extract_text(resp) == "visible"


# --- judge-route env isolation (the critical trap) -----------------------

def test_resolve_judge_route_prefers_sideeye_over_anthropic(monkeypatch):
    """Run from a Qwen session: ANTHROPIC_BASE_URL points at the cheap route,
    but SIDEEYE_JUDGE_BASE_URL must win so the review hits the real Claude route."""
    monkeypatch.setenv("ANTHROPIC_BASE_URL", "https://ai-gateway-qwen-x.example")
    monkeypatch.setenv("ANTHROPIC_API_KEY", "qwen-key")
    monkeypatch.setenv("SIDEEYE_JUDGE_BASE_URL", "https://api.anthropic.com")
    monkeypatch.setenv("SIDEEYE_JUDGE_API_KEY", "real-key")
    base, key = J.resolve_judge_route()
    assert base == "https://api.anthropic.com" and key == "real-key"


def test_resolve_model_aliases():
    assert J.resolve_model("fable") == "claude-fable-5"
    assert J.resolve_model("OPUS") == "claude-opus-4-8"      # case-insensitive
    assert J.resolve_model("sonnet") == "claude-sonnet-5"
    assert J.resolve_model("claude-fable-5") == "claude-fable-5"   # full id passthrough
    assert J.resolve_model("some-other-model") == "some-other-model"


def test_resolve_judge_route_falls_back_to_anthropic(monkeypatch, tmp_path):
    monkeypatch.setenv("SIDEEYE_CONFIG", str(tmp_path / "no-config.json"))
    monkeypatch.delenv("SIDEEYE_JUDGE_BASE_URL", raising=False)
    monkeypatch.delenv("SIDEEYE_JUDGE_API_KEY", raising=False)
    monkeypatch.setenv("ANTHROPIC_BASE_URL", "https://api.anthropic.com")
    monkeypatch.setenv("ANTHROPIC_API_KEY", "k")
    base, key = J.resolve_judge_route()
    assert base == "https://api.anthropic.com" and key == "k"
