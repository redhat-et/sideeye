import io
import json
import pathlib
import sys

import sideeye.escalate as E


FIXTURE = pathlib.Path(__file__).parent / "fixtures" / "sample_opencode_export.json"


def test_opencode_stdin_bridge_is_offline_testable_and_redacts(monkeypatch, tmp_path):
    export = json.loads(FIXTURE.read_text())
    export["data"]["messages"][1]["text"] = "Fix parser with API_KEY=secret-value"
    repo = tmp_path / "repo"
    repo.mkdir()
    (repo / ".git").mkdir()
    wrapper = {
        "repo": str(repo),
        "touched_files": [{"path": "parser.rs", "count": 1}],
        "data": export["data"],
    }
    seen = {}

    monkeypatch.setattr(E, "require_judge_route", lambda base=None: ("https://judge.example", "key", "config"))
    monkeypatch.setattr(E, "build_diff_entries", lambda *args, **kwargs: [])
    def fake_fit(transcript, *args, **kwargs):
        seen["transcript"] = transcript
        return ("PRODUCED", 50, 0.01, True, "", None)

    monkeypatch.setattr(E, "_fit_packet", fake_fit)

    def fake_judge(*args, **kwargs):
        seen["judge_args"] = args
        return (
            {
                "answered_what_was_asked": True,
                "correctness": "correct",
                "claims_supported": True,
                "score": 5,
                "issues": [],
                "overall_severity": "none",
                "summary": "fixture verdict",
            },
            {"judge_model": "claude-opus-4-8", "input_tokens": 50, "output_tokens": 5,
             "cost_usd": 0.01, "retries": 0},
        )

    monkeypatch.setattr(E, "judge", fake_judge)
    monkeypatch.setattr(sys, "stdin", io.StringIO(json.dumps(wrapper)))
    monkeypatch.setattr(sys, "argv", [
        "sideeye review", "--opencode-export", "-", "--yes",
        "--out", str(tmp_path / "verdicts.jsonl"),
    ])

    E.main()

    transcript = seen["transcript"]
    rendered = json.dumps(transcript)
    assert "secret-value" not in rendered
    assert "hidden reasoning" not in rendered
    assert "/sideeye review" not in rendered
    assert "test command failed" in rendered
    assert "pytest -q" in rendered
    record = json.loads((tmp_path / "verdicts.jsonl").read_text().strip())
    assert record["session_id"] == "ses_fixture_opencode"
    assert record["generation_cache_read_tokens"] == 900


def test_estimate_only_is_structured_and_never_calls_judge(monkeypatch, tmp_path, capsys):
    export = json.loads(FIXTURE.read_text())
    monkeypatch.setattr(E, "require_judge_route", lambda base=None: ("https://judge.example", "key", "config"))
    monkeypatch.setattr(E, "_fit_packet", lambda *args, **kwargs: ("PRODUCED", 50, 0.25, True, "", None))
    monkeypatch.setattr(E, "judge", lambda *args, **kwargs: (_ for _ in ()).throw(AssertionError("judge called")))
    monkeypatch.setattr(sys, "stdin", io.StringIO(json.dumps(export)))
    monkeypatch.setattr(sys, "argv", [
        "sideeye review", "--opencode-export", "-", "--estimate-only", "--json",
    ])

    E.main()

    estimate = json.loads(capsys.readouterr().out.strip().splitlines()[-1])
    assert estimate["type"] == "estimate"
    assert estimate["estimated_cost_usd"] == 0.25


def test_judge_model_seen_in_generator_turns_is_a_conflict():
    transcript = {"generator_models": ["anthropic/claude-fable-5-1"], "turns": []}
    assert E._generator_model_conflict(transcript, "claude-fable-5") == [
        "anthropic/claude-fable-5-1"
    ]
