"""Shared verdict-record shape for session judging.

Both the sampler (random stream) and escalate (human stream) produce this same
record — but write to SEPARATE files, because escalated verdicts are an
adversarially selected sample and must never be pooled into the random-sample
quality/savings aggregate (they would skew the scoreboard). The record carries
the generation usage the cost report needs for the counterfactual savings math.

adapter_version stamps which judge-input artifact produced the verdict:
  - "v0-blind"   : narrative-only (the adapter truncated code to 400 chars;
                   the judge never saw the code). All pre-fix verdicts.
  - "v1-sighted" : the judge received the git-diff code artifact (full files
                   with markers). Verdicts under the fixed adapter.
  - "*-tiered"   : the full packet overflowed the judge's context window, so the
                   transcript was salience-tiered to fit — all human turns + the
                   full diff + failure evidence kept, lower-salience assistant/
                   tool noise evicted. A tiered verdict saw a PARTIAL narrative
                   (though the complete human intent + code), so it must not pool
                   with a full-session verdict — it is a different, honestly
                   weaker, observation of the session.
Blind and sighted verdicts must NEVER pool in a savings/quality aggregate — a
narrative-only score and a code-aware score are not commensurable. Nor may tiered
pool with full. The cost report partitions on this field; absence (old records)
is treated as v0-blind.
"""
from __future__ import annotations

ADAPTER_VERSION_BLIND = "v0-blind"
ADAPTER_VERSION_SIGHTED = "v1-sighted"
# Tiered variants: same artifact class, but the transcript was degraded to fit
# the window. Distinct so a partial-narrative verdict never averages in with a
# whole-session one.
ADAPTER_VERSION_BLIND_TIERED = "v0-blind-tiered"
ADAPTER_VERSION_SIGHTED_TIERED = "v1-sighted-tiered"


def session_verdict_record(transcript, verdict, meta, *, rubric_version, source,
                           tier, judged_at, adapter_version=ADAPTER_VERSION_BLIND):
    gu = transcript.get("generation_usage") or {}
    return {
        "session_id": transcript["session_id"],
        "source": source,               # "sampled" | "escalated"
        "tier": tier,                   # 1 = transcript review, 2 = agentic (future)
        "rubric_version": rubric_version,
        "adapter_version": adapter_version,   # blind vs sighted — never pool across
        "judge_model": meta["judge_model"],
        "score": verdict["score"],
        "correctness": verdict["correctness"],
        "claims_supported": verdict["claims_supported"],
        "issues": verdict["issues"],
        "overall_severity": verdict["overall_severity"],
        "summary": verdict["summary"],
        "judge_input_tokens": meta["input_tokens"],
        "judge_output_tokens": meta["output_tokens"],
        "judge_cost_usd": meta["cost_usd"],
        # Generation side — what the cheap/free model actually spent. Drives the
        # counterfactual "what would this have cost on an expensive model" math.
        "generation_model": transcript.get("model"),
        "generation_input_tokens": gu.get("input_tokens", 0),
        "generation_output_tokens": gu.get("output_tokens", 0),
        "generation_total_tokens": gu.get("total_tokens", 0),
        "generation_cache_read_tokens": gu.get("cache_read_tokens", 0),
        "generation_cache_write_tokens": gu.get("cache_write_tokens", 0),
        "judged_at": judged_at,
    }
