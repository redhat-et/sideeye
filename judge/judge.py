"""Side-Eye judge: grade a (request, response) with a versioned rubric via
Claude, using a forced tool call for structured output.

Reused unchanged for pair grading (Phase B) and session grading (Phase C):
`asked` is the user request, `produced` is either a single answer or a whole
session transcript. The judge routes through the dogfood Anthropic route so the
judge's own spend is metered on the dashboard.
"""
from __future__ import annotations

import json
import pathlib

import requests

from .schema import (
    CORRECTNESS_VALUES,
    JUDGE_FIELDS,
    OVERALL_SEVERITY_VALUES,
    SEVERITY_VALUES,
    VerdictError,
    validate_verdict,
)
from .transcript import first_user_ask, render

ANTHROPIC_VERSION = "2023-06-01"
DEFAULT_MODEL = "claude-sonnet-5"
DEFAULT_MAX_TOKENS = 1024

# Advice gets its own output budget: thinking-capable judges burn tokens on
# thinking blocks BEFORE emitting a word of text. Live incident 2026-09-03:
# advice calls died as "judge returned no text" at the 1024 default —
# stop_reason=max_tokens with a thinking-only content array (the budget was
# all thinking). Free-form advice runs longer than a record_verdict tool
# call anyway; 8k bounds a fable-rate worst case at ~$0.40.
ADVICE_MAX_TOKENS = 8192

# Judge-model pricing, USD per token (input, output), from the dogfood
# model_pricing table (verified 2026-08-21). Used only to report the judge
# call's own cost in each verdict; the authoritative bill is the metered spend.
PRICING = {
    "claude-sonnet-5": (2.0 / 1e6, 10.0 / 1e6),
    "claude-opus-4-8": (5.0 / 1e6, 25.0 / 1e6),
    "claude-fable-5": (10.0 / 1e6, 50.0 / 1e6),
}

# Judge context windows (input-token limit), per model. The gateway sends the
# standard model ids — not the [1m] long-context variants — so these are the
# 200k standard windows. A packet larger than this is rejected by the provider
# with a 400 ("prompt is too long") BEFORE any useful work, and on the metered
# route it STILL shows up as spend, so Side-Eye must refuse to send it. Override
# a specific model here if a longer-context variant is ever routed.
CONTEXT_WINDOWS = {
    "claude-sonnet-5": 200_000,
    "claude-opus-4-8": 200_000,
    "claude-fable-5": 200_000,
    "claude-haiku-4-5-20251001": 200_000,
}
DEFAULT_CONTEXT_WINDOW = 200_000


def _anthropic_base_url(base_url):
    """Normalize a provider base URL before appending Anthropic API paths."""
    base = (base_url or "").rstrip("/")
    return base[:-3] if base.endswith("/v1") else base


def context_window(model) -> int:
    """The judge model's input context window, in tokens."""
    return CONTEXT_WINDOWS.get(model, DEFAULT_CONTEXT_WINDOW)


def context_guard(input_tokens, model, max_tokens=DEFAULT_MAX_TOKENS):
    """Return an error string if a packet of `input_tokens` (plus room for the
    model's output) would exceed the judge model's context window — i.e. the
    provider would reject it with a 400 and, on the metered route, still bill it.
    Return None if it fits.

    This is a HARD limit, orthogonal to --max-cost: a huge packet can sit UNDER
    the dollar ceiling yet blow past the context window (474k tokens at Fable's
    $10/M is $4.74 — under a $5 ceiling — but 2.4x over the 200k window). The
    cost guard alone let exactly that 400 through and still get metered."""
    window = context_window(model)
    needed = input_tokens + max_tokens
    if needed > window:
        over = needed - window
        return (f"packet is {input_tokens:,} input tokens; {model}'s context window "
                f"is {window:,} (need ~{needed:,} with output headroom — over by "
                f"{over:,}). The judge call would be rejected (400 'prompt is too "
                "long') and still metered, so Side-Eye is not sending it. Escalate "
                "a shorter session or a narrower scope; --no-code drops the code "
                "diff if the narrative alone fits.")
    return None


RECORD_VERDICT_TOOL = {
    "name": "record_verdict",
    "description": "Record the structured grading verdict for the response under review.",
    "input_schema": {
        "type": "object",
        "properties": {
            "answered_what_was_asked": {
                "type": "boolean",
                "description": "Did the response address the actual request, all parts of it?",
            },
            "correctness": {
                "type": "string",
                "enum": list(CORRECTNESS_VALUES),
                "description": "Overall technical correctness of the response.",
            },
            "claims_supported": {
                "type": "boolean",
                "description": (
                    "False if the answer makes any claim its own content cannot "
                    "support or contradicts (e.g. 'tests pass' with no evidence)."
                ),
            },
            "score": {
                "type": "integer",
                "minimum": 1,
                "maximum": 5,
                "description": "1 (fundamentally wrong) to 5 (correct, complete, well-supported).",
            },
            "issues": {
                "type": "array",
                "description": "Specific defects found; empty if none.",
                "items": {
                    "type": "object",
                    "properties": {
                        "description": {"type": "string"},
                        "severity": {"type": "string", "enum": list(SEVERITY_VALUES)},
                    },
                    "required": ["description", "severity"],
                },
            },
            "overall_severity": {
                "type": "string",
                "enum": list(OVERALL_SEVERITY_VALUES),
                "description": "Severity of the worst issue present.",
            },
            "summary": {"type": "string", "description": "One-sentence verdict."},
        },
        "required": [
            "answered_what_was_asked",
            "correctness",
            "claims_supported",
            "score",
            "issues",
            "overall_severity",
            "summary",
        ],
    },
}


# Friendly short names for the judge, so `--model fable` works from the CLI and
# skills without memorizing the full string. Full model ids pass through unchanged.
MODEL_ALIASES = {
    "fable": "claude-fable-5",
    "opus": "claude-opus-4-8",
    "sonnet": "claude-sonnet-5",
    "haiku": "claude-haiku-4-5-20251001",
}


def resolve_model(name):
    """Map a friendly alias (fable/opus/sonnet/haiku) to its full model id; leave
    an already-full id (e.g. claude-opus-4-8) untouched."""
    return MODEL_ALIASES.get((name or "").strip().lower(), name)


def load_rubric(path) -> str:
    return pathlib.Path(path).read_text(encoding="utf-8")


def rubric_version(path) -> str:
    """The rubric version is the file stem, e.g. rubric_v1.md -> 'rubric_v1'."""
    return pathlib.Path(path).stem


def build_request_body(rubric_text, asked, produced, model=DEFAULT_MODEL,
                       max_tokens=DEFAULT_MAX_TOKENS):
    """Build the Anthropic Messages request. Rubric goes in the system prompt;
    the pair goes in the user turn; the tool call is forced."""
    user = (
        "## What was asked\n\n"
        + asked.strip()
        + "\n\n## The response under review\n\n"
        + produced.strip()
        + "\n\nGrade the response using the rubric, then call record_verdict."
    )
    return {
        "model": model,
        "max_tokens": max_tokens,
        "system": rubric_text,
        "messages": [{"role": "user", "content": user}],
        "tools": [RECORD_VERDICT_TOOL],
        "tool_choice": {"type": "tool", "name": "record_verdict"},
    }


def parse_verdict(response_json):
    """Extract and validate the verdict from the judge's tool_use block."""
    for block in response_json.get("content", []) or []:
        if block.get("type") == "tool_use" and block.get("name") == "record_verdict":
            return validate_verdict(block.get("input"))
    # VerdictError (a ValueError subclass): judge() distinguishes an invalid
    # verdict worth one corrective retry from a refusal that isn't.
    raise VerdictError("no record_verdict tool_use block found in judge response")


def cost_usd(model, usage) -> float:
    cin, cout = PRICING.get(model, PRICING[DEFAULT_MODEL])
    return round(usage.get("input_tokens", 0) * cin + usage.get("output_tokens", 0) * cout, 6)


# Route fingerprints of KNOWN CHEAP-MODEL endpoints. The judge must never grade
# a session with the very model under measurement — a Qwen judge reviewing a
# Qwen session is a self-graded scoreboard, and vLLM serves the Anthropic API
# natively, so the mistake fails silently (no 404, no crash).
_JUDGE_FORBIDDEN_ROUTES = (
    ("127.0.0.1:818", "the local GLM gateway"),
    ("localhost:818", "the local GLM gateway"),
    ("ai-gateway-qwen", "the dogfood Qwen route"),
)


def resolve_judge_route():
    """Resolve the judge's (base_url, api_key) — thin (base, key) shim over
    sideeye.config.resolve_judge_route, kept for existing callers and tests.

    This is THE isolation point: a Side-Eye CLI is often run from inside a cheap-
    model session whose ANTHROPIC_BASE_URL points at Qwen/GLM. The judge must not
    inherit that. Full chain (flag > SIDEEYE_JUDGE_* env > config file > ambient
    ANTHROPIC_*) lives in config.py; new callers should use
    config.require_judge_route, which additionally validates the route. Callers
    of this shim must still pass the result through judge_route_guard."""
    from sideeye.config import resolve_judge_route as _resolve
    base, key, _source = _resolve()
    return base, key


def judge_route_guard(base_url):
    """Return an error string if base_url is a known cheap-model route, else None.
    The single source of truth for 'which routes may grade sessions' — called by
    the CLIs (fail fast before the gate) and by call_judge (last line of defense)."""
    for needle, what in _JUDGE_FORBIDDEN_ROUTES:
        if needle in (base_url or ""):
            return (f"judge route points at {what} ({base_url}); the judge must use the "
                    "real Claude route. Set SIDEEYE_JUDGE_BASE_URL to the dogfood "
                    "Anthropic route.")
    return None


# User-Agent for Side-Eye's own gateway calls. The metering dashboard derives the
# "client" column from this header, so a distinct value makes escalate traffic
# show as its own client (not "Python SDK") and filterable for cost. Overridden
# per surface: "sideeye-review" (escalate-all) / "sideeye-advise" (escalate-last).
DEFAULT_USER_AGENT = "sideeye/1.0"


def call_judge(base_url, api_key, body, timeout=60, user_agent=DEFAULT_USER_AGENT):
    prob = judge_route_guard(base_url)
    if prob:
        raise ValueError(prob)
    url = _anthropic_base_url(base_url) + "/v1/messages"
    headers = {
        "x-api-key": api_key,
        "anthropic-version": ANTHROPIC_VERSION,
        "content-type": "application/json",
        "user-agent": user_agent,
    }
    resp = requests.post(url, headers=headers, json=body, timeout=timeout)
    if resp.status_code >= 400:
        # Surface the provider's error body. raise_for_status() throws it away and
        # leaves an opaque "400 Bad Request"; Anthropic's body says WHY (e.g.
        # "prompt is too long: 474074 tokens > 200000 maximum"), which is the
        # difference between a legible failure and a mystery.
        try:
            payload = resp.json()
            detail = (payload.get("error") or {}).get("message") or json.dumps(payload)
        except ValueError:
            detail = (resp.text or "")[:500]
        raise requests.HTTPError(
            f"{resp.status_code} {resp.reason} from {url}: {detail}", response=resp)
    return resp.json()


def count_tokens(base_url, api_key, body, timeout=30, user_agent=DEFAULT_USER_AGENT):
    """Ask the Anthropic count_tokens endpoint for the exact input token count of
    a request body. Free call. Returns (input_tokens, ok, reason): ok=True and
    reason="" on success; ok=False with a human-readable reason on failure so the
    caller can tell an empty base URL from a route that doesn't serve the path
    from a transient network error, instead of silently falling back."""
    if not base_url or not base_url.strip():
        return 0, False, "no judge base URL set (SIDEEYE_JUDGE_BASE_URL)"
    url = _anthropic_base_url(base_url) + "/v1/messages/count_tokens"
    headers = {
        "x-api-key": api_key,
        "anthropic-version": ANTHROPIC_VERSION,
        "content-type": "application/json",
        "user-agent": user_agent,
    }
    # count_tokens accepts model/system/messages/tools/tool_choice (the tool
    # schemas DO count as billed input) but not sampling params like max_tokens
    # — strip it so the request isn't rejected with a 400.
    ct_body = {k: v for k, v in body.items() if k != "max_tokens"}
    try:
        resp = requests.post(url, headers=headers, json=ct_body, timeout=timeout)
    except requests.RequestException as exc:
        # MissingSchema (empty/unroutable URL), connection refused, timeout, etc.
        return 0, False, f"network error: {exc.__class__.__name__}"
    if resp.status_code != 200:
        return 0, False, f"HTTP {resp.status_code} from count_tokens route"
    return resp.json().get("input_tokens", 0), True, ""


def _fallback_token_count(body):
    """chars/4 estimate of the billed input tokens in a request body, used when
    count_tokens is unreachable. Counts everything count_tokens would bill:
    the system prompt, the message text, AND the tool schemas (a fixed ~500-token
    overhead this previously ignored, causing a systematic undercount)."""
    import json as _json

    sys_len = len(body.get("system", "") or "")
    msg_len = sum(
        len(c.get("text", "")) if isinstance(c, dict) else len(str(c))
        for m in body.get("messages", [])
        for c in (m.get("content") if isinstance(m.get("content"), list) else [m.get("content")])
    )
    # Tool definitions and the forced tool_choice are billed input tokens too —
    # a JSON-schema tool like record_verdict runs ~2KB, ~500 tokens, every call.
    tools_len = len(_json.dumps(body.get("tools", []))) + len(_json.dumps(body.get("tool_choice")))
    return (sys_len + msg_len + tools_len) // 4


def estimate_cost(base_url, api_key, body, model, max_tokens=DEFAULT_MAX_TOKENS,
                  user_agent=DEFAULT_USER_AGENT):
    """Pre-judge cost estimate. Input side is exact (count_tokens on the real
    payload); output is bounded by max_tokens and labeled an estimate. Returns
    (input_tokens, est_cost_usd, exact, reason): exact=True means the input
    count came from count_tokens; False means it fell back to chars/4 and reason
    explains why (so the caller can label it honestly or fix the route)."""
    input_tokens, exact, reason = count_tokens(base_url, api_key, body, user_agent=user_agent)
    if not exact:
        input_tokens = _fallback_token_count(body)
    cin, cout = PRICING.get(model, PRICING[DEFAULT_MODEL])
    # The budget rides IN the body (advice and verdict differ); estimating with
    # the function default while sending ADVICE_MAX_TOKENS would understate
    # advice estimates ~8x. The body is the single source of truth.
    budget = body.get("max_tokens") or max_tokens
    # Output is unknown until the model responds; bound it by max_tokens and use
    # half of it as the working estimate (verdicts rarely fill the budget).
    est_output = budget // 2
    est_cost = round(input_tokens * cin + est_output * cout, 6)
    return input_tokens, est_cost, exact, reason


def judge(asked, produced, rubric_text, *, base_url, api_key,
          model=DEFAULT_MODEL, max_tokens=DEFAULT_MAX_TOKENS, timeout=60,
          user_agent=DEFAULT_USER_AGENT):
    """Grade one (asked, produced) pair. Returns (verdict, meta).

    Forced tool_choice isn't always honored — on long inputs the judge
    sometimes omits a required field (e.g. `summary`). Rather than throw
    away a paid call, retry once with a corrective nudge that carries the
    model's prior turn so it re-emits record_verdict with every field. The
    meta accumulates input/output tokens and cost across both calls and
    records the retry count, so the reported spend is always the true spend.
    If the retry also produces an invalid verdict, the VerdictError propagates
    so the caller can record the failure honestly instead of crashing blind.
    """
    body = build_request_body(rubric_text, asked, produced, model=model, max_tokens=max_tokens)
    responses = [call_judge(base_url, api_key, body, timeout=timeout, user_agent=user_agent)]
    try:
        verdict = parse_verdict(responses[-1])
    except VerdictError as exc:
        if responses[-1].get("stop_reason") == "refusal":
            # The judge refused the packet (content policy) — a corrective
            # retry would just refuse again on a second charge. Fail with
            # the diagnosis; escalate.py records the failure honestly.
            raise VerdictError(
                f"{exc} ({_empty_response_detail(responses[-1])})") from exc
        # The judge returned a malformed record_verdict. Re-send the prior
        # assistant turn + a correction so it can fix the specific defect,
        # rather than re-judging from scratch. Forced tool_choice makes it
        # re-emit the tool call.
        #
        # CRITICAL: the assistant turn we echo back contains a `tool_use` block
        # (the malformed record_verdict). Anthropic requires the NEXT message to
        # answer it with a `tool_result` for that exact id — a plain-text nudge
        # instead triggers "tool_use ids were found without tool_result blocks"
        # (400), which silently burned a paid first call. So the correction rides
        # inside the tool_result.
        correction = (f"Your record_verdict call was invalid: {exc}. "
                      "Call record_verdict again, this time including ALL required "
                      f"fields: {', '.join(JUDGE_FIELDS)}.")
        tool_use_id = next(
            (b.get("id") for b in responses[-1].get("content", [])
             if isinstance(b, dict) and b.get("type") == "tool_use"), None)
        if tool_use_id:
            user_content = [{"type": "tool_result", "tool_use_id": tool_use_id,
                             "content": correction, "is_error": True}]
        else:
            user_content = correction   # no tool_use to answer (shouldn't happen)
        retry_body = {
            "model": model,
            "max_tokens": max_tokens,
            "system": body["system"],
            "messages": body["messages"] + [
                {"role": "assistant", "content": responses[-1].get("content", [])},
                {"role": "user", "content": user_content},
            ],
            "tools": body["tools"],
            "tool_choice": body["tool_choice"],
        }
        responses.append(call_judge(base_url, api_key, retry_body, timeout=timeout, user_agent=user_agent))
        verdict = parse_verdict(responses[-1])  # may raise — caller handles

    # Accumulate spend across every call (the original + any retry), so the
    # reported cost reflects what was actually billed.
    total_input = sum((r.get("usage", {}) or {}).get("input_tokens", 0) for r in responses)
    total_output = sum((r.get("usage", {}) or {}).get("output_tokens", 0) for r in responses)
    total_cost = sum(cost_usd(model, r.get("usage", {}) or {}) for r in responses)
    meta = {
        "judge_model": model,
        "input_tokens": total_input,
        "output_tokens": total_output,
        "cost_usd": round(total_cost, 6),
        "retries": len(responses) - 1,
    }
    return verdict, meta


def build_advice_body(rubric_text, asked, produced, model=DEFAULT_MODEL,
                      max_tokens=ADVICE_MAX_TOKENS):
    """Build the request for ADVICE mode: a free-form second opinion, no forced
    verdict tool. The rubric (advice-flavored) is the system prompt; the light
    packet (last exchange + optional question) is the user turn."""
    user = (
        "## Context (the last exchange)\n\n"
        + produced.strip()
        + (("\n\n## The question I want your opinion on\n\n" + asked.strip())
           if asked and asked.strip() else "")
        + "\n\nGive your second opinion per the rubric."
    )
    return {
        "model": model,
        "max_tokens": max_tokens,
        "system": rubric_text,
        "messages": [{"role": "user", "content": user}],
    }


def _extract_text(response_json) -> str:
    """Concatenate the assistant's text blocks (skip thinking blocks)."""
    out = []
    for block in response_json.get("content", []) or []:
        if block.get("type") == "text" and block.get("text"):
            out.append(block["text"])
    return "\n".join(out).strip()


def _empty_response_detail(response_json) -> str:
    """Explain a 2xx judge response that carried no text. Without this,
    structurally different failures — the judge REFUSING the packet
    (stop_reason="refusal", e.g. content policy tripping on credential-ish
    code) vs the response being CUT OFF mid-stream (stop_reason="max_tokens")
    vs a thinking-only completion — all collapse into one opaque "returned
    no text" and every debugging session starts from zero. Each hints its
    own fix because they are genuinely different problems."""
    stop = response_json.get("stop_reason") or "unknown"
    blocks = [b.get("type", "?") for b in response_json.get("content", []) or []]
    out_tokens = (response_json.get("usage", {}) or {}).get("output_tokens", "?")
    hint = {
        "refusal": "judge refused the packet (content policy) — try --no-code or a narrower --turns",
        "max_tokens": "response was cut off before emitting text — raise max_tokens",
        "tool_use": "judge answered with a tool call only — unexpected in advice mode",
    }.get(stop)
    if stop == "end_turn" and blocks and all(b == "thinking" for b in blocks):
        hint = "thinking-only completion — no visible text was produced"
    detail = (f"stop_reason={stop}, blocks={blocks or ['none']}, "
              f"output_tokens={out_tokens}")
    return detail + (f" — {hint}" if hint else "")


def advise(asked, produced, rubric_text, *, base_url, api_key,
           model=DEFAULT_MODEL, max_tokens=ADVICE_MAX_TOKENS, timeout=60,
           user_agent=DEFAULT_USER_AGENT):
    """ADVICE mode: one free-form second opinion on the last exchange. Returns
    (advice_text, meta). No retry loop — advice has no required-field schema to
    violate, unlike the verdict path."""
    body = build_advice_body(rubric_text, asked, produced, model=model, max_tokens=max_tokens)
    resp = call_judge(base_url, api_key, body, timeout=timeout, user_agent=user_agent)
    text = _extract_text(resp)
    if not text:
        raise ValueError(f"judge returned no text for advice ({_empty_response_detail(resp)})")
    usage = resp.get("usage", {}) or {}
    meta = {
        "judge_model": model,
        "input_tokens": usage.get("input_tokens", 0),
        "output_tokens": usage.get("output_tokens", 0),
        "cost_usd": cost_usd(model, usage),
        "retries": 0,
    }
    return text, meta


def judge_session(transcript, rubric_text, *, base_url, api_key,
                  model=DEFAULT_MODEL, max_tokens=DEFAULT_MAX_TOKENS, timeout=90,
                  code_artifact=None):
    """Grade a whole SessionTranscript. Renders it to (asked, produced) and
    reuses the pair judge unchanged — the judge doesn't care whether `produced`
    is one answer or a full transcript.

    If `code_artifact` (text) is given, it is appended to `produced` so the
    judge reviews the actual code (git diff with markers), not just the
    conversation narrative. Without it, the judge falls back to narrative-only
    — honestly, and version-stamped as blind (v0) so it never pools with
    sighted (v1) verdicts in the scoreboard."""
    asked = first_user_ask(transcript)
    produced = render(transcript)
    if code_artifact:
        produced = produced + "\n\n" + code_artifact
    return judge(asked, produced, rubric_text, base_url=base_url, api_key=api_key,
                 model=model, max_tokens=max_tokens, timeout=timeout)
