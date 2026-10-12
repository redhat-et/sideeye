# Side-Eye

*Draft cheap, review expensive* — a cheap model does the work; its output is
sampled to an expensive **Claude** judge, asynchronously, outside the request
path. On demand, `/sideeye:ask` sends the session you're in (or part of it) to
the strong judge for an independent second opinion. The scoreboard answers
*"how much can we save at held quality?"*
(Pitch: [`docs/judge-sampling-pitch.md`](docs/judge-sampling-pitch.md).)

## Install (Claude Code)

```
/plugin marketplace add redhat-et/sideeye
/plugin install sideeye@sideeye
```

That's the whole install. The first time you run `/sideeye:ask`, the skill
installs the `sideeye` engine itself (one-time, ~30s, via `uv` or `pipx` if
present) — no terminal, no files to copy.

If you also want the CLI on PATH outside Claude Code (or your machine has no
uv/pipx and you'd rather install once by hand):

```bash
uv tool install git+https://github.com/redhat-et/sideeye   # or pipx / pip install
pip install -e .                                             # development checkout
```

Runtime requirements (judge route) are in the section below.

### Configure the judge route

The judge must hit a real Claude route — never the cheap model that drafted the
session (a judge sharing the drafted-with model is the self-grading failure
Side-Eye exists to catch). Resolution order:

1. `--base-url` flag
2. `SIDEEYE_JUDGE_BASE_URL` + `SIDEEYE_JUDGE_API_KEY` env vars
3. config file — `~/.config/sideeye/config.json` (or `$SIDEEYE_CONFIG`), chmod 600:
   ```json
   { "judge": { "base_url": "https://your-claude-route", "api_key": "sk-…" } }
   ```
4. ambient `ANTHROPIC_API_KEY` (direct Anthropic) or `ANTHROPIC_BASE_URL` +
   `ANTHROPIC_AUTH_TOKEN` — accepted only when the host is really Anthropic's;
   a gateway route needs `"judge": {"trust_ambient_route": true}` to opt in.

**A key alone is enough**: `api_key` without `base_url` (in the config file or
as `SIDEEYE_JUDGE_API_KEY`) means "talk to Anthropic direct"
(`https://api.anthropic.com`) — the common case for a user with an API key and
no gateway. Set `base_url` only when the judge sits behind a gateway.

Check what you'd get, without spending: `sideeye route` (exit 0 = ready).

### First run

```bash
sideeye route                      # must say "verdict: OK"
```
```
/sideeye:ask --validate                            # free: is the route ready? ($0)
/sideeye:ask is my last change sound?              # cents, scoped to recent turns
/sideeye:ask --all                                 # full-session review, median ~$0.75
```

Both spend real money. Observed on 20 dogfood reviews: `advise` ≈ $0.02–$1,
`review` ≈ median $0.75, up to ~$3.50 on big sessions (defaults: Opus 4.8 for
`review`, Fable 5 for `advise`; `--model fable|opus|sonnet|haiku` to choose,
`--max-cost N` as a ceiling).

The engine is still POC-grade in places, but it runs for real: 156 offline
tests, and the capture → judge → verdict → cost pipeline is verified on real
sessions. It is plain Python glue — deliberately: the perf-critical hot path is
the gateway (Rust/praxis); the judge worker is I/O-bound (one multi-second LLM
call), so language choice is irrelevant off-path. See
[`docs/sideeye-phase-c-questions.md`](docs/sideeye-phase-c-questions.md) for
the design decisions this implements.

## Layout

```
sideeye/
  rubric/rubric_v3.md            pair-grading rubric (Phase B)
  rubric/rubric_session_v2.md    session-grading rubric — default for `sideeye review`
  rubric/rubric_advice_v1.md     default for `sideeye advise`
  config.py                      judge-route resolution chain + cheap-route guards
  judge/schema.py                verdict schema + validation + is_flagged()
  judge/transcript.py            capture-agnostic SessionTranscript (the pipe)
  judge/judge.py                 forced-tool-call structured verdict; judge_session()
  adapters/codex_rollout.py      adapter #1: Codex rollout log -> SessionTranscript
  adapters/claude_code.py        adapter #2: Claude Code transcript -> SessionTranscript
  skills/                        Claude Code plugin skill (the /sideeye:ask router)
  docs/                          pitch, MVP scope, phase-C design questions, cost distillation
  record.py                      shared session-verdict record shape
  run_judge.py                   Phase B: grade a JSONL of (prompt, answer) pairs
  sampler.py                     Phase C: RANDOM stream — scan rollouts, judge, verdicts/sampled.jsonl
  escalate.py                    Phase C: HUMAN stream — "ask the expensive model", verdicts/escalated.jsonl
  cost_report.py                 the money story: counterfactual savings + quality, CLI + HTML
  tests/                         unit tests (no network): 156 passing
  data/                          seed_pairs, hard_tasks, ground_truth (fixtures)
  verdicts/                      run output (gitignored)
```

## The capture-agnostic pipe (the key design decision)

Every capture source produces a **`SessionTranscript`** (`judge/transcript.py`);
the judge, sampler, and cost report only ever see that shape. So capture
options are just adapters into one pipe:

- **`codex_rollout`** — reads Codex's own session logs. In-scope, no praxis
  changes, full session transcript. Caveat: it's the *client's* view — if the
  gateway mutated the request, the log wouldn't show it.
- **`claude_code`** — reads Claude Code's session transcripts
  (`~/.claude/projects/`). What the skills and the default sampler use.
- **gateway sampling tap** (production, not built): a praxis filter that
  samples at request time and emits events (Rust, lives in the proxy repo);
  the schema is designed so results carry over unchanged.

## Phase B — pair judging (falsification gate) — DONE

```bash
pytest tests -q                                         # 156 tests, no network
python -m sideeye.run_judge --pairs data/seed_pairs.jsonl \
    --ground-truth data/ground_truth.json               # needs a judge route
```
Result: judge caught 2/2 planted defects, 0/4 clean false-positives; on hard
field-evidence tasks it correctly passed GLM's safe answers and flagged the
truncated one. ~$0.008/pair on Sonnet 5.

## Phase C — automatic capture + escalation

Two verdict streams, kept **separate** (escalations are an adversarial sample and
must never skew the random-sample scoreboard):

```bash
# RANDOM stream — unbiased quality/savings estimate (Sonnet 5; --client claude|codex)
python -m sideeye.sampler --idle-min 10 --sample-rate 1.0

# HUMAN stream — "I'm not sure, ask the expensive model" (Opus 4.8 by default)
sideeye review                        # == python -m sideeye.escalate, tier-1
sideeye review --model fable          # the nasty ones

# The money story (counterfactual savings + held quality; CLI + HTML)
python -m sideeye.cost_report --html verdicts/cost-report.html
```

Escalation is **review, not re-answer**; it carries a **tier knob** (tier-1
transcript review = default; tier-2 agentic "run the code" = the only tier that
catches execution-dependent defects, and is a sandboxed worker, not a script —
not built in this POC, and it refuses honestly if asked).

The whole pipeline (capture → judge → verdict → cost) is verified end-to-end on
real Codex rollouts and Claude Code sessions. On the dogfood deployment the
judge route points at the metered gateway, so judge spend shows on the
dashboard; for any other user it's just their Claude route (see Configure).

## Rules honored

- Secrets from env or a local (untracked) key file only — never hardcoded,
  committed, or logged.
- In the reference deployment the judge routes through the metered gateway, so
  judge spend is visible on the dashboard; the draft model is self-hosted and
  costs $0.
- Counterfactual savings are labeled estimates, not measurements.
