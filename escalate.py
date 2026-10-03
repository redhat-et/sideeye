#!/usr/bin/env python3
"""Side-Eye escalate — the HUMAN stream ("I'm not sure, ask the expensive model").

The on-demand fourth trigger: a human escalates a specific session for review by
a stronger judge. Same judge machinery as the sampler, manually triggered.

Design decisions (from the Side-Eye review):
  - REVIEW, not re-answer: the expensive model critiques; it never takes over the
    turn. A review verdict is commensurable with the sampler's verdicts.
  - Stronger judge by default (Opus 4.8): the human already paid attention, so
    the prior of a real defect is elevated and volume is human-bounded.
  - SEPARATE stream (verdicts/escalated.jsonl): escalations are adversarially
    selected and must never be pooled into the random-sample scoreboard. They are
    high-yield defect discovery + a curated hard-set for calibrating the judge.
  - A TIER knob, not just a model knob: escalation buys SELECTION, not new
    capability. Tier 1 = transcript review (default). Tier 2 = agentic
    verification (run the code) — the only thing that catches execution-dependent
    defects; not built in this POC (it's a sandboxed worker, not a script).

Env: ANTHROPIC_BASE_URL, ANTHROPIC_API_KEY.

  python -m sideeye.escalate                 # latest session, tier 1, Opus 4.8
  python -m sideeye.escalate --rollout <path> --model claude-fable-5
"""
from __future__ import annotations

import argparse
import json
import os
import pathlib
import sys
import time

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent.parent))

from sideeye.adapters import latest_session, load_transcript, resolve_current_session  # noqa: E402
from sideeye.adapters.opencode import parse_export  # noqa: E402
from sideeye.config import RouteError, require_judge_route  # noqa: E402
from sideeye.judge.code_artifact import build_diff_entries, render_diff_artifact  # noqa: E402
from sideeye.judge.judge import (  # noqa: E402
    DEFAULT_MAX_TOKENS,
    build_request_body,
    context_guard,
    context_window,
    estimate_cost,
    judge,
    load_rubric,
    resolve_model,
    rubric_version,
)
from sideeye.judge.transcript import (  # noqa: E402
    first_user_ask,
    redact_text,
    redact_transcript,
    render,
    render_budgeted,
    strip_escalation,
)
from sideeye.record import (  # noqa: E402
    ADAPTER_VERSION_BLIND,
    ADAPTER_VERSION_BLIND_TIERED,
    ADAPTER_VERSION_SIGHTED,
    ADAPTER_VERSION_SIGHTED_TIERED,
    session_verdict_record,
)

# Headroom reserved below the context window when packing: output budget +
# a margin for tokenizer disagreement (our count vs the provider's) and JSON
# wrapping. Fail-closed — we'd rather tier one notch further than eat a 400.
_SAFETY_TOKENS = 3000
_INEXACT_EXTRA_MARGIN = 12000   # extra headroom when count_tokens is unavailable
_MAX_FIT_ITERS = 4              # estimate-then-verify iterations (converges in 1-2; free count_tokens)
# A full session review verdict (summary + several issues, each with a
# description) needs more output room than the 1024 default — a truncated
# record_verdict drops a required field and forces the retry. Give it headroom.
_REVIEW_MAX_TOKENS = 4096
_REVIEW_UA = "sideeye-review"    # metering "client" label for escalate-all traffic

REPO = pathlib.Path(__file__).resolve().parent
DEFAULT_RUBRIC = REPO / "rubric" / "rubric_session_v2.md"
DEFAULT_OUT = REPO / "verdicts" / "escalated.jsonl"
ESCALATION_MODEL = "claude-opus-4-8"  # stronger by default

# Placeholder verdict for the error path: the judge ran (money spent) but
# produced no valid verdict. session_verdict_record needs the verdict shape to
# build a record; these sentinel values make the failure record self-explanatory
# and ensure is_flagged() won't mistake it for a real pass.
_EMPTY_VERDICT = {
    "answered_what_was_asked": False,
    "correctness": "incorrect",
    "claims_supported": False,
    "score": 1,
    "issues": [],
    "overall_severity": "critical",
    "summary": "(judge produced no valid verdict)",
}


def fail(msg):
    from sideeye.judge import style
    print(style.render_error(msg), file=sys.stderr)
    sys.exit(1)


def _same_model_family(left, right):
    """Match exact model ids and version-suffixed ids across providers."""
    if not left or not right:
        return False
    left = str(left).rsplit("/", 1)[-1].lower()
    right = str(right).rsplit("/", 1)[-1].lower()
    return left == right or left.startswith(right + "-") or right.startswith(left + "-")


def _generator_model_conflict(transcript, judge_model):
    models = set(transcript.get("generator_models") or [])
    models.update(turn.get("model") for turn in transcript.get("turns", []) if turn.get("model"))
    return sorted(model for model in models if _same_model_family(model, judge_model))


def _git_root(path):
    path = pathlib.Path(path).expanduser().resolve()
    if path.is_file():
        path = path.parent
    for candidate in (path, *path.parents):
        if (candidate / ".git").exists():
            return candidate
    return None


def _resolve_repo_root(repo_hint, touched):
    """Find one git root, including when a workspace contains nested repos."""
    base = pathlib.Path(repo_hint or pathlib.Path.cwd()).expanduser().resolve()
    direct = _git_root(base)
    if direct:
        return direct
    roots = set()
    for entry in touched or []:
        raw_path = entry.get("path") if isinstance(entry, dict) else None
        if not raw_path:
            continue
        candidate = pathlib.Path(raw_path)
        if not candidate.is_absolute():
            candidate = base / candidate
        root = _git_root(candidate)
        if root:
            roots.add(root)
    return roots.pop() if len(roots) == 1 else base


def _fit_packet(transcript, asked, make_diff, rubric_text, *, base_url, api_key, model,
                max_tokens=DEFAULT_MAX_TOKENS, user_agent=_REVIEW_UA):
    """Fit the judge packet into `model`'s context window.

    make_diff(budget_chars=None) -> the code artifact (str) or None. Passing a
    budget rebuilds the diff smaller (least-edited files degrade to manifest-only),
    which is the global diff-overflow rung.

    Returns (produced, input_tokens, est_cost, exact, reason, coverage):
      - coverage is None when the FULL packet fit (normal session, unchanged);
      - otherwise it describes what tiering kept/dropped (+ coverage['diff_degraded']).
    Calls fail() only if the packet can't be made to fit at all.

    The packet is built ONCE here and returned, so the cost estimate and the real
    judge call see the identical bytes (the old path rendered twice — estimate in
    escalate, call in judge_session — which would mismatch under tiering).
    All probes use the free count_tokens endpoint — no judge call, no spend."""
    window = context_window(model)

    def margin(exact):
        # Fail-closed: reserve the (review) output budget + a safety margin, plus
        # extra headroom when count_tokens is unavailable and we're on a chars/4 guess.
        return max_tokens + _SAFETY_TOKENS + (0 if exact else _INEXACT_EXTRA_MARGIN)

    def measure(transcript_text, diff):
        # One free count_tokens round-trip for a candidate packet. Returns the
        # built packet + its REAL token count (+ cost/exact/reason).
        produced = transcript_text + (("\n\n" + diff) if diff else "")
        body = build_request_body(rubric_text, asked, produced, model=model)
        it, ec, ex, rs = estimate_cost(base_url, api_key, body, model, user_agent=user_agent)
        return produced, it, ec, ex, rs

    full_text = render(transcript)
    full_diff = make_diff()

    def fit_transcript_to(diff):
        # Estimate-then-verify: trim the transcript (render_budgeted keeps human +
        # final turn, then tool-result EVIDENCE, then narration) so transcript+diff
        # fits the window. Each measurement gives the REAL chars/token, so we jump
        # to the budget and verify — converging in 1-2 calls, vs a ~10-probe binary
        # search that re-uploaded the multi-MB packet each time (latency + a phantom
        # 0/0 metering row per probe). Returns (produced,it,ec,ex,rs,coverage);
        # coverage is None if the FULL transcript fit. Returns None if even the
        # floor (all human turns) + diff overflows.
        produced, it, ec, ex, rs = measure(full_text, diff)
        if it + margin(ex) <= window:
            return produced, it, ec, ex, rs, None
        cov = None
        for _ in range(_MAX_FIT_ITERS):
            target = window - margin(ex)
            cpt = len(produced) / max(it, 1)                    # real chars/token, this packet
            diff_chars = len("\n\n" + diff) if diff else 0
            tx_budget = max(1, int((len(produced) - diff_chars) - (it - target) * cpt * 1.05))
            text, cov = render_budgeted(transcript, tx_budget)
            produced, it, ec, ex, rs = measure(text, diff)
            if it + margin(ex) <= window:
                return produced, it, ec, ex, rs, cov
            if not cov["fits"]:
                return None                                     # floor+diff overflows → shrink diff
        return None

    # 1) Full diff: ship whole if it fits, else tier the transcript to it.
    r = fit_transcript_to(full_diff)
    if r is not None:
        produced, it, ec, ex, rs, cov = r
        return produced, it, ec, ex, rs, (None if cov is None else {**cov, "diff_degraded": False})

    # 2) Even the floor + full diff overflows → the diff is too big. Size it down to
    # the room left after the sacred floor (estimate-then-verify; aim UNDER so
    # make_diff's whole-file drops land below the window, not on the edge), then
    # regrow the transcript into any remaining room so tool evidence isn't lost.
    floor_text, floor_cov = render_budgeted(transcript, 0)
    produced, floor_tokens, ec, ex, rs = measure(floor_text, None)
    if floor_tokens + margin(ex) > window:
        fail(f"the human turns alone are ~{floor_tokens:,} tokens vs {model}'s {window:,} "
             "window — this session is too large to review in one call even narrative-only. "
             "Escalate a shorter/fresher session or a narrower scope.")
    if full_diff is not None:
        cpt_diff = 2.6                                          # dense; refined from real counts
        for _ in range(_MAX_FIT_ITERS):
            room = window - margin(True) - floor_tokens
            d = make_diff(max(0, int(room * cpt_diff * 0.95)))
            produced, it, ec, ex, rs = measure(floor_text, d)
            if it + margin(ex) <= window:
                grown = fit_transcript_to(d)                    # refill tool evidence into headroom
                if grown is not None:
                    p, i, e, x, rr, cov = grown
                    return p, i, e, x, rr, {**(cov or floor_cov), "diff_degraded": len(d) < len(full_diff)}
                return produced, it, ec, ex, rs, {**floor_cov, "diff_degraded": len(d) < len(full_diff)}
            cpt_diff = len(d) / max(it - floor_tokens, 1)       # recalibrate from measurement

    # 3) Can't fit even the reduced diff → refuse rather than ship over-budget.
    fail(f"cannot fit the packet into {model}'s {window:,} window even with the diff "
         "reduced. Re-run with --no-code for a narrative-only review, or narrow the change set.")


def main():
    ap = argparse.ArgumentParser(description="Side-Eye on-demand escalation (human stream)")
    ap.add_argument("--rollout", default=None, help="session file (default: latest in scope)")
    ap.add_argument("--opencode-export", default=None,
                    help="OpenCode V2 session export JSON, or '-' for stdin; intended for the native plugin")
    ap.add_argument("--client", choices=("claude", "codex"), default="claude",
                    help="which client's latest session to escalate (default claude)")
    ap.add_argument("--project", default=None,
                    help="claude only: a specific project dirname under "
                         "~/.claude/projects/ (or a path). Defaults to the "
                         "current working directory's project, so 'escalate' "
                    "reviews the session you're in, not a random live one.")
    ap.add_argument("--all-projects", action="store_true",
                    help="search every project for the latest session (the old "
                         "default — a coin flip across all live sessions).")
    ap.add_argument("--current", action="store_true",
                    help="review the current project's latest session (this is the "
                         "default; explicit for the skill surface).")
    ap.add_argument("--yes", action="store_true",
                    help="skip the pre-judge confirmation (judge calls cost real money)")
    ap.add_argument("--max-cost", type=float, default=5.0,
                    help="abort if the estimated cost exceeds this ($). The safety "
                         "ceiling that survives --yes — a huge session can't silently overspend.")
    ap.add_argument("--tier", type=int, choices=(1, 2), default=1)
    ap.add_argument("--model", default=ESCALATION_MODEL)
    ap.add_argument("--rubric", default=str(DEFAULT_RUBRIC))
    ap.add_argument("--out", default=str(DEFAULT_OUT))
    ap.add_argument("--json", action="store_true",
                    help="emit the persisted structured verdict as the final stdout line")
    ap.add_argument("--repo", default=None,
                    help="git repo root for the code diff (default: cwd). The "
                         "session's touched files are diffed against HEAD here.")
    ap.add_argument("--diff-base", default=None,
                    help="git diff base for tracked files (default: HEAD; native plugins may provide a session/branch base)")
    ap.add_argument("--no-code", action="store_true",
                    help="judge narrative-only (the old blind mode). For "
                         "blind-vs-sighted comparison; the verdict is stamped "
                         "v0-blind and must not pool with sighted verdicts.")
    # The judge must NEVER share the session's cheap route (self-grading).
    # Resolution chain in config.py: flag > SIDEEYE_JUDGE_* > config file >
    # ambient ANTHROPIC_* (last one guarded against cheap routes).
    ap.add_argument("--base-url", default=None,
                    help="judge route override; default resolves from "
                         "SIDEEYE_JUDGE_* env, the sideeye config file, or ANTHROPIC_*")
    args = ap.parse_args()
    args.model = resolve_model(args.model)   # accept fable/opus/sonnet aliases

    if args.rollout and args.opencode_export:
        fail("--rollout and --opencode-export are mutually exclusive")

    if args.tier == 2:
        fail("tier-2 (agentic: check out the code and RUN it) is not implemented "
             "in this POC. Tier 2 is a sandboxed judge-worker with tools, not a "
             "script — it is the only tier that catches execution-dependent "
             "defects. Use --tier 1 for transcript review.")

    try:
        args.base_url, api_key, _route_source = require_judge_route(args.base_url)
    except RouteError as e:
        fail(str(e))

    # Native OpenCode passes a sanitized V2 export directly. This avoids a
    # second terminal and avoids guessing where OpenCode stores its database.
    # The plugin also passes touched_files in the wrapper so the normal sighted
    # diff path remains shared with Claude Code and Codex.
    if args.opencode_export:
        try:
            if args.opencode_export == "-":
                export = json.load(sys.stdin)
            else:
                export = json.loads(pathlib.Path(args.opencode_export).read_text(encoding="utf-8"))
            extra = export.get("touched_files") if isinstance(export, dict) else None
            wrapper_repo = export.get("repo") if isinstance(export, dict) else None
            if args.repo is None and isinstance(wrapper_repo, str) and wrapper_repo.strip():
                args.repo = wrapper_repo
            wrapper_diff_base = export.get("diff_base") if isinstance(export, dict) else None
            if args.diff_base is None and isinstance(wrapper_diff_base, str) and wrapper_diff_base.strip():
                args.diff_base = wrapper_diff_base
            transcript = parse_export(export, touched_files=extra)
        except (OSError, json.JSONDecodeError, TypeError, ValueError) as exc:
            fail(f"invalid OpenCode export: {exc}")
        rollout = None
        scope = None
        session_label = "OpenCode export"
    # Session selection. Default scope = the current project (cwd), so escalate
    # reviews the session you're actually in — not the global-latest, which is a
    # coin flip across every live Claude session on the machine.
    elif args.rollout:
        scope = None
        rollout = pathlib.Path(args.rollout)
        session_label = str(rollout)
    elif args.all_projects:
        scope = "all"
        rollout = latest_session(args.client, scope="all")
        session_label = str(rollout) if rollout else "no session"
    elif args.project:
        scope = args.project
        rollout = latest_session(args.client, scope=args.project)
        session_label = str(rollout) if rollout else "no session"
    else:
        scope = "cwd"
        # Robust to a drifted shell cwd (e.g. a skill's Bash tool running from
        # /tmp): try the cwd project, else fall back to the most-recently-written
        # session — the one you're actually in.
        rollout, fell_back = resolve_current_session(args.client)
        if fell_back and rollout:
            print(f"(cwd has no {args.client} session; using the most recently "
                  "active session instead)")
        env_id = os.environ.get("CLAUDE_CODE_SESSION_ID")
        if env_id and rollout and rollout.stem != env_id:
            print(f"(harness session {env_id[:8]} not found on disk; judging the "
                  "most recently active session instead)")
        session_label = str(rollout) if rollout else "no session"
    if not args.opencode_export and (not rollout or not rollout.exists()):
        if scope == "cwd":
            fail(f"no {args.client} session in this project "
                 f"({latest_session.__module__}.claude_project_dir). "
                 "Pass --all-projects to search everywhere, or run from the "
                 "session's working directory.")
        fail(f"no {args.client} session found")
    if not args.opencode_export:
        transcript = load_transcript(rollout)
        if transcript is None:
            fail(f"no usable turns in {rollout}")

    # Exclude Side-Eye's own escalation machinery BEFORE anything else reads the
    # transcript. Without this the packet is self-referential by construction:
    # the injected /escalate-* skill body rides in as a sacred user ask the
    # session "failed", and the snapshot ends mid-call (the review's own result
    # can't exist until the judge answers), so every escalate-all verdict carries
    # a false critical about the review not having happened yet.
    raw_transcript = transcript
    transcript, esc_dropped = strip_escalation(transcript)
    if transcript is not raw_transcript:
        print(f"Scope   : escalation machinery excluded"
              + (f" ({esc_dropped} turn(s))" if esc_dropped else " (annotated in-packet)")
              + " — this review grades the work, not the review request itself")

    conflicts = _generator_model_conflict(transcript, args.model)
    if conflicts:
        fail(
            f"judge model {args.model!r} also generated turns in this session "
            f"({', '.join(conflicts)}); choose a different judge to avoid self-grading"
        )
    transcript = redact_transcript(transcript)

    rubric_text = load_rubric(args.rubric)
    rv = rubric_version(args.rubric)

    # Show exactly what we're about to judge before spending money on it. The
    # first user turn is the cheapest recognition signal: if it's not the session
    # you meant, Ctrl-C now, before the judge call.
    first_ask = first_user_ask(transcript).replace("\n", " ")[:120]
    print(f"Session : {session_label}")
    print(f"           {transcript['session_id'][:24]}  ({len(transcript['turns'])} turns)")
    if first_ask:
        print(f"First ask: {first_ask}")
    print(f"Judge   : {args.model}, tier {args.tier}")

    # Pre-judge cost estimate on the REAL payload (the rendered transcript +
    # rubric + tool schema — the exact bytes the judge will receive). Input side
    # is exact via count_tokens; output is bounded and labeled est. This is the
    # guard against the $3.38 surprise: see the price before you commit.
    asked = first_user_ask(transcript)

    # Build the code-review artifact lazily via make_diff(budget_chars): the judge
    # reviews the actual code (git diff with markers), not just narrative. Passing
    # a budget rebuilds a smaller diff (global diff-overflow rung). --no-code and
    # unresolvable/absent files fall back to narrative-only (blind), honestly.
    adapter_version = ADAPTER_VERSION_SIGHTED
    touched = transcript.get("touched_files") or []
    repo_root = _resolve_repo_root(args.repo, touched)
    if args.opencode_export and touched and not _git_root(repo_root):
        fail(
            "OpenCode export includes touched files but no git repository was "
            "resolved; pass a repository root in the export wrapper or --repo"
        )
    # Build the diff entries ONCE (git is the slow part); make_diff then re-renders
    # them cheaply at any budget for the overflow search — no re-shelling to git.
    diff_entries = ([] if args.no_code or not touched
                    else build_diff_entries(touched, repo_root, diff_base=args.diff_base or "HEAD"))

    def make_diff(budget_chars=None):
        artifact = render_diff_artifact(diff_entries, budget_chars) or None
        return redact_text(artifact) if artifact else None

    if args.no_code:
        print("Code    : --no-code (blind mode, v0) — narrative only")
        adapter_version = ADAPTER_VERSION_BLIND
    elif not touched:
        print("Code    : no touched files in transcript — narrative only")
        adapter_version = ADAPTER_VERSION_BLIND
    elif make_diff() is None:
        print("Code    : no diff produced (touched files unresolvable) — narrative only")
        adapter_version = ADAPTER_VERSION_BLIND
    else:
        print(f"Code    : {len(touched)} touched file(s) diffed (sighted, adapter v1)")

    # Fit the packet into the judge's window (builds `produced` once, shared by
    # the estimate and the real call). coverage is None if the full packet fit;
    # otherwise the transcript was salience-tiered and coverage says what dropped.
    produced, input_tokens, est_cost, exact, reason, coverage = _fit_packet(
        transcript, asked, make_diff, rubric_text,
        base_url=args.base_url, api_key=api_key, model=args.model,
        max_tokens=_REVIEW_MAX_TOKENS, user_agent=_REVIEW_UA)

    if coverage is not None:
        # Tiered: stamp a DISTINCT adapter_version (a partial-narrative verdict is
        # not commensurable with a full-session one) and show what was dropped.
        # The tiering itself protects the failure-evidence — every human turn and
        # tool result — that a naive recency window would have silently dropped.
        adapter_version = (ADAPTER_VERSION_SIGHTED_TIERED
                           if adapter_version == ADAPTER_VERSION_SIGHTED
                           else ADAPTER_VERSION_BLIND_TIERED)
        k, d = coverage["kept"], coverage["dropped"]
        has_code = adapter_version == ADAPTER_VERSION_SIGHTED_TIERED
        diff_desc = ("" if not has_code
                     else " + partial diff (least-edited files elided)" if coverage.get("diff_degraded")
                     else " + full diff")
        print(f"Packet  : overflows {args.model}'s {context_window(args.model):,} window "
              "— salience-tiered to fit")
        print(f"          kept: all {k['human']} human turn(s){diff_desc}"
              f" + {k['assistant']} assistant + {k['tool']} tool turn(s)")
        print(f"          dropped (lower-salience, newest kept): {d['assistant']} assistant "
              f"narration + {d['tool']} tool output(s)"
              + (f"; {coverage['human_capped']} human turn(s) capped" if coverage["human_capped"] else ""))
        print(f"          verdict stamped {adapter_version} (won't pool with full-session verdicts)")

    if exact:
        print(f"Cost    : ~${est_cost:.4f}  ({input_tokens:,} input tokens at judge rate; "
              "output est.)")
    else:
        # reason explains WHY count_tokens failed (empty URL, network error,
        # HTTP 404, …) so the user can fix the route rather than guess.
        print(f"Cost    : ~${est_cost:.4f}  (~{input_tokens:,} input tokens, chars/4 "
              f"estimate — count_tokens unavailable ({reason}); output est.)")

    # Context-window guard: a HARD limit that no --max-cost can override. With
    # tiering the packet should now fit; this is the final assertion that catches
    # any residual overflow the fit loop couldn't resolve — refuse rather than eat
    # a 400 that's still metered (474k tokens once cost ~$4.77 for a rejected call).
    if (prob := context_guard(input_tokens, args.model, max_tokens=_REVIEW_MAX_TOKENS)):
        fail(prob)

    # Cost ceiling: the anti-$3.38-surprise guard that survives --yes. Skips the
    # interactive prompt (needed for non-TTY skill use) but NOT the ceiling, so a
    # huge session can't overspend silently. Exits nonzero so the skill can react.
    if est_cost > args.max_cost:
        fail(f"estimated ${est_cost:.4f} exceeds --max-cost ${args.max_cost:.2f} "
             f"({input_tokens:,} input tokens). Re-run with --max-cost {est_cost + 1:.0f} to proceed.")

    if not args.yes:
        try:
            input("\nEscalate this session to the judge? [Enter to proceed, Ctrl-C to abort] ")
        except (KeyboardInterrupt, EOFError):
            print("\naborted before judge call (no spend).")
            return

    print(f"\nEscalating to {args.model}...\n")

    try:
        # Judge the packet we already fit + estimated — same bytes, no re-render
        # (judge_session would render the FULL transcript again, undoing tiering).
        verdict, meta = judge(asked, produced, rubric_text, base_url=args.base_url,
                              api_key=api_key, model=args.model, timeout=90,
                              max_tokens=_REVIEW_MAX_TOKENS, user_agent=_REVIEW_UA)
    except Exception as exc:
        # The judge ran (money was spent) but produced no valid verdict even
        # after retry. Don't crash and lose the evidence — record the failure
        # so the spend is documented and the session isn't silently retried.
        from sideeye.judge.schema import VerdictError
        kind = "malformed_verdict" if isinstance(exc, VerdictError) else "judge_error"
        record = session_verdict_record(transcript, _EMPTY_VERDICT, {"judge_model": args.model,
                                        "input_tokens": 0, "output_tokens": 0, "cost_usd": 0.0,
                                        "retries": 1}, rubric_version=rv, source="escalated",
                                        tier=args.tier, judged_at=int(time.time()),
                                        adapter_version=adapter_version)
        record["error"] = f"{kind}: {exc}"
        out_path = pathlib.Path(args.out)
        out_path.parent.mkdir(exist_ok=True)
        with open(out_path, "a", encoding="utf-8") as out:
            out.write(json.dumps(record) + "\n")
        # Failure goes to stderr (the skill reads stderr on nonzero) and we exit
        # nonzero — the spend is recorded, but there is no verdict, so the caller
        # must NOT treat this as success (and must not fabricate one).
        print(f"JUDGE FAILED after spend: {exc}", file=sys.stderr)
        print(f"failure recorded (no valid verdict) -> {out_path}", file=sys.stderr)
        sys.exit(1)

    record = session_verdict_record(transcript, verdict, meta, rubric_version=rv,
                                    source="escalated", tier=args.tier,
                                    judged_at=int(time.time()),
                                    adapter_version=adapter_version)

    out_path = pathlib.Path(args.out)
    out_path.parent.mkdir(exist_ok=True)
    with open(out_path, "a", encoding="utf-8") as out:
        out.write(json.dumps(record) + "\n")

    # Interactive: show the review prominently — the human asked to see it.
    # Colors render only on a real TTY; piped captures stay plain text.
    if args.json:
        print(json.dumps(record, ensure_ascii=False, separators=(",", ":")))
    else:
        from sideeye.judge import style
        print(style.render_verdict(record, str(out_path)))


if __name__ == "__main__":
    main()
