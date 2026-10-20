"""Unit tests for the Side-Eye judge module: rubric loading, verdict schema
validation, and structured-output (tool_use) parsing. No network."""
from __future__ import annotations

import pathlib
import sys

import pytest

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[2]))

from sideeye.judge import judge as J  # noqa: E402
from sideeye.judge.schema import (  # noqa: E402
    VerdictError,
    is_flagged,
    validate_verdict,
)


def _good_verdict(**overrides):
    v = {
        "answered_what_was_asked": True,
        "correctness": "correct",
        "claims_supported": True,
        "score": 5,
        "issues": [],
        "overall_severity": "none",
        "summary": "Clean and correct.",
    }
    v.update(overrides)
    return v


# --- rubric loading -------------------------------------------------------

def test_load_rubric_and_version(tmp_path):
    p = tmp_path / "rubric_v1.md"
    p.write_text("grade it", encoding="utf-8")
    assert J.load_rubric(p) == "grade it"
    assert J.rubric_version(p) == "rubric_v1"


# --- verdict schema validation --------------------------------------------

def test_valid_verdict_passes():
    assert validate_verdict(_good_verdict()) is not None


def test_valid_verdict_with_issues():
    v = _good_verdict(
        correctness="incorrect", score=2, overall_severity="major",
        issues=[{"description": "wrong API", "severity": "major"}],
    )
    assert validate_verdict(v)


@pytest.mark.parametrize("bad", [
    _good_verdict(score=0),
    _good_verdict(score=6),
    _good_verdict(score=True),          # bool is not a valid int score
    _good_verdict(score="5"),
])
def test_bad_score_rejected(bad):
    with pytest.raises(VerdictError):
        validate_verdict(bad)


def test_missing_field_rejected():
    v = _good_verdict()
    del v["summary"]
    with pytest.raises(VerdictError):
        validate_verdict(v)


def test_bad_correctness_rejected():
    with pytest.raises(VerdictError):
        validate_verdict(_good_verdict(correctness="mostly"))


def test_bad_severity_rejected():
    with pytest.raises(VerdictError):
        validate_verdict(_good_verdict(overall_severity="catastrophic"))


def test_non_bool_claims_rejected():
    with pytest.raises(VerdictError):
        validate_verdict(_good_verdict(claims_supported="yes"))


def test_malformed_issue_rejected():
    with pytest.raises(VerdictError):
        validate_verdict(_good_verdict(issues=[{"description": "x"}]))  # no severity
    with pytest.raises(VerdictError):
        validate_verdict(_good_verdict(issues=[{"description": "", "severity": "minor"}]))


def test_empty_summary_rejected():
    with pytest.raises(VerdictError):
        validate_verdict(_good_verdict(summary="   "))


# --- structured-output parsing --------------------------------------------

def _response_with_tool(input_dict):
    return {
        "content": [
            {"type": "text", "text": "thinking..."},
            {"type": "tool_use", "name": "record_verdict", "input": input_dict},
        ],
        "usage": {"input_tokens": 100, "output_tokens": 20},
    }


def test_parse_verdict_extracts_and_validates():
    resp = _response_with_tool(_good_verdict())
    v = J.parse_verdict(resp)
    assert v["score"] == 5


def test_parse_verdict_no_tool_use_raises():
    resp = {"content": [{"type": "text", "text": "no tool call here"}]}
    with pytest.raises(ValueError):
        J.parse_verdict(resp)


def test_parse_verdict_invalid_input_raises():
    resp = _response_with_tool(_good_verdict(score=99))
    with pytest.raises(VerdictError):
        J.parse_verdict(resp)


# --- judge retry on malformed verdict -------------------------------------

def _bad_then_good_responses():
    """First response: verdict missing `summary` (the fable-5 failure mode).
    Second response: a complete, valid verdict."""
    bad = _good_verdict()
    del bad["summary"]
    return [
        _response_with_tool(bad),
        _response_with_tool(_good_verdict(summary="Fixed on retry.")),
    ]


def test_judge_retries_once_on_malformed_verdict(monkeypatch):
    """A malformed first verdict triggers exactly one corrective retry; the
    returned verdict is the good one from the second call."""
    calls = _bad_then_good_responses()
    sent = []
    def fake_call(base_url, api_key, body, timeout=60, **kwargs):
        sent.append(body)
        return calls.pop(0)
    monkeypatch.setattr(J, "call_judge", fake_call)
    v, meta = J.judge("q", "a", "rubric", base_url="http://x", api_key="k")
    assert v["summary"] == "Fixed on retry."
    assert meta["retries"] == 1
    # The retry must carry the prior assistant turn + a corrective user turn.
    retry_msgs = sent[1]["messages"]
    assert retry_msgs[-2]["role"] == "assistant"
    assert "summary" in sent[1]["messages"][-1]["content"]   # the nudge names the gap


def test_judge_accumulates_cost_across_retry(monkeypatch):
    """The reported cost/tokens must reflect BOTH calls, not just the last —
    the retry is a real billed call on the same 132k-token input."""
    calls = _bad_then_good_responses()
    # First call: 100 in / 20 out; second: 100 in / 15 out (the retry re-reads
    # the full input). Fable = $10/M in, $50/M out.
    calls[0]["usage"] = {"input_tokens": 100, "output_tokens": 20}
    calls[1]["usage"] = {"input_tokens": 100, "output_tokens": 15}
    monkeypatch.setattr(J, "call_judge", lambda *a, **k: calls.pop(0))
    v, meta = J.judge("q", "a", "rubric", base_url="http://x", api_key="k",
                      model="claude-fable-5")
    assert meta["input_tokens"] == 200   # both calls
    assert meta["output_tokens"] == 35
    # 200 * 10/M + 35 * 50/M
    assert meta["cost_usd"] == pytest.approx(round(200 * 10.0 / 1e6 + 35 * 50.0 / 1e6, 6))


def test_judge_no_retry_when_first_verdict_valid(monkeypatch):
    """A valid first verdict means zero retries and a single call's cost."""
    monkeypatch.setattr(J, "call_judge",
                        lambda *a, **k: _response_with_tool(_good_verdict()))
    v, meta = J.judge("q", "a", "rubric", base_url="http://x", api_key="k")
    assert meta["retries"] == 0
    assert meta["input_tokens"] == 100 and meta["output_tokens"] == 20


def test_judge_propagates_when_retry_also_malformed(monkeypatch):
    """If the retry also produces an invalid verdict, the VerdictError propagates
    so the caller can record the failure honestly (escalate saves an error record)."""
    bad = _good_verdict()
    del bad["summary"]
    monkeypatch.setattr(J, "call_judge",
                        lambda *a, **k: _response_with_tool(bad))
    with pytest.raises(VerdictError):
        J.judge("q", "a", "rubric", base_url="http://x", api_key="k")


# --- request building -----------------------------------------------------

def test_build_request_body_forces_tool_and_carries_pair():
    body = J.build_request_body("RUBRIC-TEXT", "the question", "the answer")
    assert body["system"] == "RUBRIC-TEXT"
    assert body["tool_choice"] == {"type": "tool", "name": "record_verdict"}
    assert body["tools"][0]["name"] == "record_verdict"
    user = body["messages"][0]["content"]
    assert "the question" in user and "the answer" in user


# --- cost + flagging ------------------------------------------------------

def test_cost_uses_model_pricing():
    # Sonnet 5 = $2/M in, $10/M out.
    c = J.cost_usd("claude-sonnet-5", {"input_tokens": 1_000_000, "output_tokens": 1_000_000})
    assert c == pytest.approx(12.0)


# --- code artifact wired into the judge payload ---------------------------

def test_judge_session_appends_code_artifact(monkeypatch):
    """When a code_artifact is given, it's appended to the produced text the
    judge sees — so the judge reviews the code, not just the narrative."""
    seen_body = {}
    def fake_call(base_url, api_key, body, timeout=60, **kwargs):
        seen_body["content"] = body["messages"][0]["content"]
        return _response_with_tool(_good_verdict())
    monkeypatch.setattr(J, "call_judge", fake_call)
    from sideeye.judge.transcript import make_transcript
    t = make_transcript(session_id="s", source="claude_code",
                        turns=[{"role": "user", "text": "do X"},
                               {"role": "assistant", "text": "did X"}])
    J.judge_session(t, "rubric", base_url="http://x", api_key="k",
                    code_artifact="## Code changes\n+++ new.py\nactual code here")
    assert "actual code here" in seen_body["content"]
    assert "## Code changes" in seen_body["content"]


def test_judge_session_without_code_artifact_is_narrative_only(monkeypatch):
    """No code_artifact = narrative only (the blind fallback). The judge still
    works; the absence is recorded via adapter_version downstream."""
    seen_body = {}
    def fake_call(base_url, api_key, body, timeout=60, **kwargs):
        seen_body["content"] = body["messages"][0]["content"]
        return _response_with_tool(_good_verdict())
    monkeypatch.setattr(J, "call_judge", fake_call)
    from sideeye.judge.transcript import make_transcript
    t = make_transcript(session_id="s", source="claude_code",
                        turns=[{"role": "user", "text": "do X"},
                               {"role": "assistant", "text": "did X"}])
    J.judge_session(t, "rubric", base_url="http://x", api_key="k")
    assert "## Code changes" not in seen_body["content"]


# --- pre-judge cost estimate ----------------------------------------------

def test_estimate_cost_fallback_when_count_tokens_unreachable(monkeypatch):
    """When count_tokens can't reach the endpoint, fall back to a chars/4
    estimate (including tool schemas) and report exact=False with a reason."""
    body = J.build_request_body("rubric " * 50, "the question", "the answer " * 200,
                                model="claude-fable-5")
    # Force count_tokens to fail by pointing at an unroutable host.
    inp, cost, exact, reason = J.estimate_cost("http://127.0.0.1:1", "k", body, "claude-fable-5")
    assert exact is False
    assert inp > 0
    assert "network error" in reason
    # Fable = $10/M in, $50/M out; output est = max_tokens//2 = 512.
    expected = round(inp * 10.0 / 1e6 + 512 * 50.0 / 1e6, 6)
    assert cost == pytest.approx(expected)


def test_estimate_cost_fallback_counts_tool_schema():
    """The chars/4 fallback must count the tool schema — otherwise it
    systematically undercounts billed input (record_verdict is ~500 tokens)."""
    body = J.build_request_body("r", "q", "a", model="claude-sonnet-5")
    # Text-only count (system + message): "r" + the wrapper + "q" + "a".
    text_only = (len(body["system"]) + len(body["messages"][0]["content"])) // 4
    inp, _, exact, _ = J.estimate_cost("", "k", body, "claude-sonnet-5")
    assert exact is False
    # The fallback must be materially larger than text-only (tool schema added).
    assert inp > text_only + 50


def test_estimate_cost_exact_when_count_tokens_succeeds(monkeypatch):
    """When count_tokens returns a count, use it exactly and report exact=True."""
    def fake_count(base_url, api_key, body, timeout=30, **kwargs):
        return 42_000, True, ""
    monkeypatch.setattr(J, "count_tokens", fake_count)
    body = J.build_request_body("r", "q", "a", model="claude-opus-4-8")
    inp, cost, exact, reason = J.estimate_cost("http://x", "k", body, "claude-opus-4-8")
    assert exact is True
    assert reason == ""
    assert inp == 42_000
    # Opus = $5/M in, $25/M out; output est = 512.
    expected = round(42_000 * 5.0 / 1e6 + 512 * 25.0 / 1e6, 6)
    assert cost == pytest.approx(expected)


def test_count_tokens_reason_on_empty_base_url():
    """An empty base URL (the env var wasn't set) must fail fast with a clear
    reason, not a cryptic MissingSchema traceback."""
    body = J.build_request_body("r", "q", "a", model="claude-sonnet-5")
    n, ok, reason = J.count_tokens("", "k", body)
    assert ok is False and n == 0
    assert "base URL" in reason


def test_count_tokens_strips_max_tokens(monkeypatch):
    """count_tokens must not forward max_tokens (a sampling param the endpoint
    rejects); tools/tool_choice are kept because tool schemas are billed input."""
    sent = {}
    class FakeResp:
        status_code = 200
        def json(self):
            return {"input_tokens": 7}
    def fake_post(url, headers=None, json=None, timeout=None):
        sent["url"] = url
        sent["body"] = json
        return FakeResp()
    monkeypatch.setattr(J.requests, "post", fake_post)
    body = J.build_request_body("r", "q", "a", model="claude-sonnet-5")
    n, ok, reason = J.count_tokens("http://x", "k", body)
    assert ok is True and n == 7 and reason == ""
    assert sent["url"] == "http://x/v1/messages/count_tokens"
    assert "max_tokens" not in sent["body"]
    assert "tools" in sent["body"] and "tool_choice" in sent["body"]


def test_anthropic_base_url_with_v1_suffix_is_normalized(monkeypatch):
    sent = {}

    class FakeResp:
        status_code = 200
        text = ""

        def json(self):
            return {"input_tokens": 1}

    def fake_post(url, **kwargs):
        sent["url"] = url
        return FakeResp()

    monkeypatch.setattr(J.requests, "post", fake_post)
    body = J.build_request_body("r", "q", "a", model="claude-sonnet-5")
    J.count_tokens("https://judge.example/v1", "k", body)
    assert sent["url"] == "https://judge.example/v1/messages/count_tokens"


def test_is_flagged_thresholds():
    assert not is_flagged(_good_verdict())
    assert is_flagged(_good_verdict(score=2))
    assert is_flagged(_good_verdict(overall_severity="major"))
    assert is_flagged(_good_verdict(claims_supported=False))
    assert not is_flagged(_good_verdict(overall_severity="minor",
                                        issues=[{"description": "nit", "severity": "minor"}]))


# --- judge route guard (never grade via the cheap model) ------------------

def test_route_guard_blocks_qwen_route():
    """The dogfood Qwen route is a known cheap-model endpoint. A Qwen judge
    reviewing a Qwen session is a self-graded scoreboard — hard block."""
    url = "https://ai-gateway-qwen.example"
    err = J.judge_route_guard(url)
    assert err is not None and "Qwen" in err


def test_route_guard_blocks_local_glm_gateway():
    for url in ("http://127.0.0.1:8181", "http://localhost:8180/v1"):
        assert J.judge_route_guard(url) is not None


def test_route_guard_allows_anthropic_route():
    """The real Claude dogfood route (and a plain Anthropic URL) must pass."""
    assert J.judge_route_guard(
        "https://ai-gateway-anthropic.example") is None
    assert J.judge_route_guard("https://api.anthropic.com") is None
    assert J.judge_route_guard(None) is None  # absence is the caller's problem, not a route error


def test_call_judge_refuses_forbidden_route_before_network(monkeypatch):
    """call_judge is the last line of defense: it must raise before any HTTP,
    even if a CLI guard was bypassed. No network call may be attempted."""
    def no_network(*a, **k):
        raise AssertionError("network must not be reached for a forbidden route")
    monkeypatch.setattr(J.requests, "post", no_network)
    with pytest.raises(ValueError):
        J.call_judge("https://ai-gateway-qwen-x.example", "k", {"messages": []})
