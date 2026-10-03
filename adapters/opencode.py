"""OpenCode V2 session-export adapter.

OpenCode exposes a projected session export through its local API. This module
normalizes that export into the same SessionTranscript consumed by the Python
judge. It intentionally excludes hidden reasoning, compaction summaries, and
synthetic messages so a future Side-Eye command cannot review its own injected
verdict or leak internal model reasoning as if it were user-visible work.
"""
from __future__ import annotations

from sideeye.judge.transcript import make_transcript


def parse_export(export, *, touched_files=None):
    """Convert an OpenCode V2 export object into a SessionTranscript.

    ``export`` may be the API response (``{"data": {"info", "messages"}}``)
    or the inner data object. The adapter consumes only stable projected fields;
    unknown OpenCode fields are ignored.
    """
    data = export.get("data", export)
    if not isinstance(data, dict):
        raise ValueError("OpenCode export data must be an object")
    info = data.get("info") or {}
    if not isinstance(info, dict):
        raise ValueError("OpenCode export info must be an object")
    messages = data.get("messages")
    if not isinstance(messages, list):
        raise ValueError("OpenCode export messages must be a list")

    turns = []
    for message in messages:
        if not isinstance(message, dict):
            continue
        message_type = message.get("type")
        if message_type == "user":
            _append(turns, "user", message.get("text", ""))
        elif message_type == "assistant":
            for part in message.get("content") or []:
                if not isinstance(part, dict):
                    continue
                # Reasoning is intentionally not part of the review artifact.
                if part.get("type") == "text":
                    _append(turns, "assistant", part.get("text", ""))
                elif part.get("type") == "tool":
                    _append_tool(turns, part)

    model_info = info.get("model") or {}
    if isinstance(model_info, dict):
        provider = model_info.get("providerID")
        model_id = model_info.get("id")
        model = "/".join(str(part) for part in (provider, model_id) if part)
    else:
        model = None

    tokens = info.get("tokens") or {}
    generation_usage = None
    if isinstance(tokens, dict):
        input_tokens = _as_int(tokens.get("input"))
        output_tokens = _as_int(tokens.get("output"))
        reasoning_tokens = _as_int(tokens.get("reasoning"))
        if any(value is not None for value in (input_tokens, output_tokens, reasoning_tokens)):
            generation_usage = {
                "input_tokens": input_tokens or 0,
                "output_tokens": (output_tokens or 0) + (reasoning_tokens or 0),
                "total_tokens": sum(value or 0 for value in (
                    input_tokens, output_tokens, reasoning_tokens
                )),
            }

    created = (info.get("time") or {}).get("created") if isinstance(info.get("time"), dict) else None
    created_at = _as_int(created)
    if created_at is not None and created_at > 10_000_000_000:
        created_at //= 1000

    return make_transcript(
        session_id=info.get("id"),
        source="opencode",
        turns=turns,
        model=model,
        created_at=created_at,
        generation_usage=generation_usage,
        touched_files=touched_files,
    )


def _append(turns, role, text):
    if isinstance(text, str):
        turns.append({"role": role, "text": text})


def _append_tool(turns, part):
    state = part.get("state") or {}
    content = state.get("content") if isinstance(state, dict) else None
    texts = []
    for item in content or []:
        if isinstance(item, dict) and isinstance(item.get("text"), str):
            texts.append(item["text"])
    if texts:
        name = part.get("name") or "tool"
        _append(turns, "tool", f"{name} result:\n{''.join(texts)}")


def _as_int(value):
    return value if isinstance(value, int) and not isinstance(value, bool) else None
