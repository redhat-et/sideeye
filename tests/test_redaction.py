from sideeye.judge.transcript import redact_text, redact_transcript


def test_python_redactor_matches_bridge_credential_shapes():
    value = (
        "API_KEY=sk-ant-secret Authorization: Bearer bearer-secret "
        "x-api-key: header-secret https://example.test/?token=url-secret prose"
    )
    redacted = redact_text(value)
    assert "secret" not in redacted
    assert "bearer-secret" not in redacted
    assert "header-secret" not in redacted
    assert "url-secret" not in redacted
    assert "prose" in redacted


def test_redact_transcript_does_not_mutate_source():
    original = {"turns": [{"role": "user", "text": "API_KEY=secret"}]}
    redacted = redact_transcript(original)
    assert original["turns"][0]["text"] == "API_KEY=secret"
    assert redacted["turns"][0]["text"] == "API_KEY=[REDACTED]"
