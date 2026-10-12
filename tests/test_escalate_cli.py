"""CLI-level tests for escalate (review mode): the cost ceiling must abort even
under --yes, and a judge failure must exit nonzero. No network — all boundaries
are monkeypatched."""
from __future__ import annotations

import pathlib
import sys

import pytest

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[2]))

import sideeye.escalate as E  # noqa: E402
from sideeye.judge.transcript import make_transcript  # noqa: E402


def _wire(monkeypatch, tmp_path, *, est_cost, judge=None, input_tokens=50_000,
          coverage=None, with_code=False):
    """Stub every external boundary so main() runs offline against a fake session.
    _fit_packet owns rendering + estimation now, so we stub it to controlled values
    (produced, input_tokens, est_cost, exact, reason, coverage). input_tokens
    defaults to a count that FITS the 200k window; pass a larger value to exercise
    main()'s final context-guard assertion; pass coverage to exercise the tiered
    stamping path."""
    monkeypatch.setenv("SIDEEYE_JUDGE_BASE_URL", "https://api.anthropic.com")
    monkeypatch.setenv("SIDEEYE_JUDGE_API_KEY", "real-key")
    fake = tmp_path / "s.jsonl"
    fake.write_text("{}", encoding="utf-8")
    t = make_transcript(session_id="s", source="claude_code",
                        turns=[{"role": "user", "text": "do X"},
                               {"role": "assistant", "text": "did X"}],
                        touched_files=[{"path": "x.py", "count": 1}] if with_code else None)
    monkeypatch.setattr(E, "latest_session", lambda *a, **k: fake)
    # `main()` resolves the default current session through this helper. Mock
    # it explicitly so CI never depends on a developer's real Claude session
    # files or on the runner's HOME contents.
    monkeypatch.setattr(E, "resolve_current_session", lambda *a, **k: (fake, False))
    monkeypatch.setattr(E, "load_transcript", lambda p: t)
    if with_code:
        monkeypatch.setattr(E, "build_diff_entries", lambda *a, **k: [{"path": "x.py"}])
        monkeypatch.setattr(E, "render_diff_artifact", lambda *a, **k: "DIFF")
    monkeypatch.setattr(E, "_fit_packet",
                        lambda *a, **k: ("PRODUCED", input_tokens, est_cost, True, "", coverage))
    if judge is not None:
        monkeypatch.setattr(E, "judge", judge)


def test_max_cost_ceiling_aborts_even_with_yes(monkeypatch, tmp_path):
    # Judge must NEVER be reached when the estimate blows the ceiling.
    def boom(*a, **k):
        raise AssertionError("judge called despite cost ceiling")
    _wire(monkeypatch, tmp_path, est_cost=99.0, judge=boom)
    monkeypatch.setattr(sys, "argv",
                        ["sideeye review", "--yes", "--max-cost", "5",
                         "--out", str(tmp_path / "out.jsonl")])
    with pytest.raises(SystemExit) as e:
        E.main()
    assert e.value.code != 0


def test_under_ceiling_reaches_judge(monkeypatch, tmp_path):
    called = {}
    def fake_judge(*a, **k):
        called["hit"] = True
        return ({"answered_what_was_asked": True, "correctness": "correct",
                 "claims_supported": True, "score": 5, "issues": [],
                 "overall_severity": "none", "summary": "ok"},
                {"judge_model": "claude-fable-5", "input_tokens": 10,
                 "output_tokens": 2, "cost_usd": 0.01, "retries": 0})
    _wire(monkeypatch, tmp_path, est_cost=0.5, judge=fake_judge)
    monkeypatch.setattr(sys, "argv",
                        ["sideeye review", "--yes", "--max-cost", "5",
                         "--out", str(tmp_path / "out.jsonl")])
    E.main()
    assert called.get("hit")


def test_context_guard_final_assertion_aborts_before_judge(monkeypatch, tmp_path):
    # Final assertion: if _fit_packet couldn't get under the window (residual
    # overflow), main() must still refuse before spending. Judge never reached → $0.
    def boom(*a, **k):
        raise AssertionError("judge called despite residual context overflow")
    _wire(monkeypatch, tmp_path, est_cost=4.77, judge=boom, input_tokens=474_074)
    monkeypatch.setattr(sys, "argv",
                        ["sideeye review", "--yes", "--max-cost", "5",
                         "--model", "fable", "--out", str(tmp_path / "out.jsonl")])
    with pytest.raises(SystemExit) as e:
        E.main()
    assert e.value.code != 0
    # No verdict/failure file written — we aborted before spending anything.
    assert not (tmp_path / "out.jsonl").exists()


def test_tiered_coverage_stamps_distinct_adapter_version(monkeypatch, tmp_path):
    # When _fit_packet tiered the packet (coverage set), the recorded verdict must
    # carry v1-sighted-tiered so it can't pool with full-session verdicts.
    cov = {"kept": {"human": 5, "assistant": 3, "tool": 4}, "kept_turns": 12,
           "dropped": {"assistant": 40, "tool": 20, "system": 0},
           "human_capped": 0, "fits": True, "total_turns": 72, "budget_chars": 700_000}
    def ok_judge(*a, **k):
        return ({"answered_what_was_asked": True, "correctness": "correct",
                 "claims_supported": True, "score": 4, "issues": [],
                 "overall_severity": "none", "summary": "ok"},
                {"judge_model": "claude-fable-5", "input_tokens": 190_000,
                 "output_tokens": 200, "cost_usd": 1.9, "retries": 0})
    _wire(monkeypatch, tmp_path, est_cost=1.9, judge=ok_judge, input_tokens=190_000,
          coverage=cov, with_code=True)
    out = tmp_path / "out.jsonl"
    monkeypatch.setattr(sys, "argv",
                        ["sideeye review", "--yes", "--max-cost", "5",
                         "--model", "fable", "--out", str(out)])
    E.main()
    import json
    rec = json.loads(out.read_text().strip().splitlines()[-1])
    assert rec["adapter_version"] == "v1-sighted-tiered"


def test_judge_failure_exits_nonzero(monkeypatch, tmp_path):
    def failing_judge(*a, **k):
        raise RuntimeError("judge HTTP 500")
    _wire(monkeypatch, tmp_path, est_cost=0.5, judge=failing_judge)
    monkeypatch.setattr(sys, "argv",
                        ["sideeye review", "--yes", "--out", str(tmp_path / "out.jsonl")])
    with pytest.raises(SystemExit) as e:
        E.main()
    assert e.value.code != 0
    # The spend/failure is still recorded (documented), even though we exit nonzero.
    assert (tmp_path / "out.jsonl").exists()
