import json
from pathlib import Path

from sideeye.adapters.opencode import parse_export


FIXTURE = Path(__file__).parent / "fixtures" / "sample_opencode_export.json"


def test_opencode_export_maps_visible_turns_and_tool_results():
    transcript = parse_export(json.loads(FIXTURE.read_text()), touched_files=[{"path": "parser.rs"}])

    assert transcript["session_id"] == "ses_fixture_opencode"
    assert transcript["source"] == "opencode"
    assert transcript["model"] == "openai/gpt-test"
    assert transcript["created_at"] == 1735689600
    assert transcript["generation_usage"] == {
        "input_tokens": 100,
        "output_tokens": 30,
        "total_tokens": 130,
        "cache_read_tokens": 900,
        "cache_write_tokens": 10,
    }
    assert transcript["generator_models"] == ["openai/gpt-test"]
    assert [turn["role"] for turn in transcript["turns"]] == ["user", "assistant", "tool", "tool"]
    assert "hidden reasoning" not in str(transcript)
    assert "Side-Eye verdict" not in str(transcript)
    assert "parser.rs contents" in transcript["turns"][2]["text"]
    assert '"path": "parser.rs"' in transcript["turns"][2]["text"]
    assert "test command failed" in transcript["turns"][3]["text"]
    assert transcript["touched_files"] == [{"path": "parser.rs"}]


def test_opencode_export_accepts_inner_data_shape():
    export = json.loads(FIXTURE.read_text())["data"]
    transcript = parse_export(export)
    assert transcript["session_id"] == "ses_fixture_opencode"
