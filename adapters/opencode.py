"""OpenCode V2 session-export adapter.

OpenCode exposes a projected session export through its local API. This module
normalizes that export into the same SessionTranscript consumed by the Python
judge. It intentionally excludes hidden reasoning, compaction summaries, and
synthetic messages so a future Side-Eye command cannot review its own injected
verdict or leak internal model reasoning as if it were user-visible work.
"""
from __future__ import annotations

from sideeye.judge.transcript import make_transcript


_SELF_REVIEW_MARKERS = (
    "/sideeye",
    "sideeye ·",
    "sideeye-rs",
    "session : opencode export",
    "escalating to ",
    "side-eye verdict",
)


def parse_export(export, *, touched_files=None):
    """Convert an OpenCode V2 export object into a SessionTranscript.

    ``export`` may be the API response (``{"data": {"info", "messages"}}``)
    or the inner data object. The adapter consumes only stable projected fields;
    unknown OpenCode fields are ignored.
    """
    if not isinstance(export, dict):
        raise ValueError("OpenCode export must be an object")
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
    generator_models = set()
    for message in messages:
        if not isinstance(message, dict):
            continue
        message_type = message.get("type")
        if message_type == "user":
            _append(turns, "user", message.get("text", ""))
        elif message_type == "assistant":
            model = _model_name(message.get("model"))
            if model:
                generator_models.add(model)
            if isinstance(message.get("error"), dict):
                _append(turns, "assistant", "model error:\n" + _json_text(message["error"]), model=model)
            for part in message.get("content") or []:
                if not isinstance(part, dict):
                    continue
                # Reasoning is intentionally not part of the review artifact.
                if part.get("type") == "text":
                    _append(turns, "assistant", part.get("text", ""), model=model)
                elif part.get("type") == "tool":
                    _append_tool(turns, part, model=model)

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
        cache = tokens.get("cache") if isinstance(tokens.get("cache"), dict) else {}
        cache_read = _as_int(cache.get("read"))
        cache_write = _as_int(cache.get("write"))
        if any(value is not None for value in (
            input_tokens, output_tokens, reasoning_tokens, cache_read, cache_write
        )):
            generation_usage = {
                "input_tokens": input_tokens or 0,
                "output_tokens": (output_tokens or 0) + (reasoning_tokens or 0),
                "total_tokens": sum(value or 0 for value in (
                    input_tokens, output_tokens, reasoning_tokens
                )),
                "cache_read_tokens": cache_read or 0,
                "cache_write_tokens": cache_write or 0,
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
        generator_models=sorted(generator_models),
    )


def _append(turns, role, text, *, model=None):
    if isinstance(text, str):
        if any(marker in text.lstrip().lower()[:120] for marker in _SELF_REVIEW_MARKERS):
            return
        turn = {"role": role, "text": text}
        if model:
            turn["model"] = model
        turns.append(turn)


def _append_tool(turns, part, *, model=None):
    state = part.get("state") or {}
    content = state.get("content") if isinstance(state, dict) else None
    texts = []
    for item in content or []:
        if isinstance(item, dict) and isinstance(item.get("text"), str):
            texts.append(item["text"])
    name = part.get("name") or "tool"
    status = state.get("status") if isinstance(state, dict) else None
    input_text = _json_text(state.get("input")) if isinstance(state, dict) and state.get("input") is not None else "(none)"
    output_text = "".join(texts) if texts else "(no output)"
    error = state.get("error") if isinstance(state, dict) else None
    error_text = f"\nerror: {_json_text(error)}" if error is not None else ""
    _append(
        turns,
        "tool",
        f"{name} input:\n{input_text}\n{name} output (status={status or 'unknown'}):\n{output_text}{error_text}",
        model=model,
    )


def _model_name(model):
    if not isinstance(model, dict):
        return None
    provider = model.get("providerID")
    model_id = model.get("id")
    return "/".join(str(part) for part in (provider, model_id) if part) or None


def _json_text(value):
    if isinstance(value, str):
        return value
    try:
        import json
        return json.dumps(value, ensure_ascii=False, sort_keys=True)
    except (TypeError, ValueError):
        return repr(value)


def _as_int(value):
    return value if isinstance(value, int) and not isinstance(value, bool) else None
