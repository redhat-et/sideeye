# Side-Eye

**Draft cheap. Review expensive.**

Side-Eye is an independent quality-control layer for AI coding agents. A cheap
model does the work; a stronger judge reviews the session, the diff, and the
evidence — then returns a structured verdict without letting the drafting model
grade itself.

- Native commands in **Claude Code** and **OpenCode**
- A **Codex** CLI adapter today
- A provider-neutral Rust core, with Python only as a behavioral oracle
- Estimate first, confirm before spend
- Self-grading, secret, context, and budget guards

**Demo:** https://redhat-et.github.io/sideeye/  
**Latest release:** https://github.com/redhat-et/sideeye/releases/tag/v0.6.19

---

## Install

```bash
uv tool install git+https://github.com/redhat-et/sideeye
# or: pipx install git+https://github.com/redhat-et/sideeye
# or: pip install git+https://github.com/redhat-et/sideeye
```

Then run the one-time setup:

```bash
sideeye setup                      # guided wizard
sideeye setup --repo /path/to/repo # or point at it explicitly
sideeye setup --model claude-opus-4-8 --max-tokens 4096
sideeye setup --yes                # accept defaults (scripts)
```

Setup is a guided flow:

- shows the detected harnesses;
- asks only for missing route/model/repo values;
- prompts once for a hidden judge credential;
- stores it in Side-Eye config with mode `600`;
- installs and configures the bundled OpenCode plugin;
- preserves unrelated JSON/JSONC settings;
- selects the Rust engine when `sideeye` is on `PATH`;
- verifies the route with a free `count_tokens` call when interactive.

Run `sideeye setup --help` for examples and automation flags.

> **Migration note:** setup and every harness path use the Rust `sideeye`
> binary. The Python CLI remains only as a behavioral oracle for conformance
> and research tools. See [`docs/rust-migration-plan.md`](docs/rust-migration-plan.md).

Check the judge route without spending:

```bash
sideeye doctor          # offline: route guard must say "passed"
sideeye doctor --verify # free route check (count_tokens or /v1/models)
```

### Judge route

The judge must be a real, stronger route — never the cheap model that drafted
the session. Providers share one packet/verdict contract:

- **Anthropic-compatible** (`--provider anthropic`): exact free `count_tokens`
  estimate and Claude pricing.
- **OpenAI-compatible / local** (`--provider openai-compatible`): OpenAI,
  gateways, Ollama, LM Studio, and vLLM. The estimate is labeled approximate
  because OpenAI has no free count endpoint; cost is reported as unknown unless
  the route provides its own metering. Tool calling is required.

Output caps are provider-aware: **8192** for Anthropic and **16384** for
OpenAI-compatible routes. GLM and DeepSeek reasoning tokens count toward the
output cap, so 4096 is too small for a real GLM review. Use
`sideeye setup --max-tokens N` to change it. The estimate prices the full cap,
so it is conservative.

Context windows are model-aware: **1,000,000** for GLM and DeepSeek,
**200,000** for Claude. Packets that exceed the window are auto-trimmed using
the free token count. Slow routes can be given a longer client timeout with
`SIDEEYE_JUDGE_TIMEOUT_SECS=600`.

Local example:

```bash
sideeye setup \
  --provider openai-compatible \
  --judge-base-url http://127.0.0.1:11434 \
  --judge-api-key local \
  --model qwen2.5-coder
```

Resolution order:

1. `--base-url`
2. `SIDEEYE_JUDGE_BASE_URL` + `SIDEEYE_JUDGE_API_KEY`
3. `~/.config/sideeye/config.json` (or `$SIDEEYE_CONFIG`)
4. ambient `ANTHROPIC_API_KEY`, or `ANTHROPIC_BASE_URL` + `ANTHROPIC_AUTH_TOKEN`

A key alone means “Anthropic direct”. A gateway route needs an explicit base URL.
Ambient Anthropic credentials are not sent to a foreign base unless you set
`judge.trust_ambient_route`.

---

## Use it

### OpenCode

The plugin adds one native TUI command. With no arguments it first asks for
optional focus instructions (type and press Enter), then opens the scope
selector (arrow keys + Enter):

```text
/sideeye                    # pick Last exchange or Whole session
/sideeye last               # estimate the last request + response
/sideeye all                # estimate the whole session
/sideeye last --confirm     # submit the last exchange
/sideeye all --confirm      # submit the whole session
/sideeye all security and missing tests
```

The scope selector shows the free estimate and asks for confirmation before any
judge spend. If the packet is too large for the judge context, Side-Eye shrinks
the packet automatically using the free token count and retries; the estimate
says when it was auto-trimmed. Extra text is reviewer instruction on top of the
rubric; only `--confirm` is special. The old `sideeye-review` server command and alias were
removed — `/sideeye` is the single user-facing command.

Flow:

```text
OpenCode export -> ReviewPacket -> redact -> budget -> estimate -> judge -> verdict
```

The verdict opens in a native Side-Eye dialog: score, severity, summary,
issues with evidence, judge/cost, packet/rubric, and the saved raw-record path.
You then label it **Confirmed**, **False positive**, **Neutral**, or skip. The
label is stored locally for metrics; the verdict is not inserted into the
model’s context, so the assistant does not automatically respond to it.
`/sideeye show` reopens the latest verdict; `/sideeye label confirmed` labels
it later. Preparation and judging use a modal step dialog, not a toast.

### Claude Code

`sideeye setup` installs a personal `/sideeye` skill. The plugin marketplace
also provides `/sideeye:ask` for users who prefer plugin distribution.

```text
/sideeye --validate                     # free route check
/sideeye is my last change sound?       # scoped second opinion
/sideeye --all                          # full-session review
```

### Codex

`sideeye setup` installs the `sideeye` skill under `~/.agents/skills/sideeye`.
Invoke it from Codex with `/skills sideeye` (or `$sideeye`) and use the same
modes:

```text
/skills sideeye --validate
/skills sideeye is this change sound?
/skills sideeye --all
```

### CLI

```bash
sideeye setup                  # configure credentials and harnesses (Rust)
sideeye doctor                 # resolved route guard, no spend
sideeye session --client claude --current
sideeye session --client codex --current
sideeye advise --client claude --current --question "..."
sideeye label confirmed        # label the newest stored verdict
sideeye stats                  # local precision/coverage/cost
```

Session reviews persist to `~/.local/share/sideeye/<client>-verdicts.jsonl`.
Use `sideeye stats --ledger ~/.local/share/sideeye/claude-verdicts.jsonl` for a
non-OpenCode ledger, or pass the same `--ledger` to `label`.

### Judge-success metrics

Labels are local-only and privacy-preserving. The label ledger stores only the
record id, label, and timestamp:

```bash
sideeye label confirmed          # newest verdict
sideeye label false-positive --id <record_id>
sideeye label neutral --id <record_id>
sideeye stats
sideeye stats --json
```

`stats` reports coverage, confirmed/false-positive/neutral counts, precision,
false-positive rate, total judge cost, and cost per confirmed finding. Catch
rate is intentionally unavailable until there is a labeled defect benchmark.
No prompts, code, paths, or keys are uploaded.

---

## Rust engine

The Rust workspace is the migration target:

| Crate | Role |
|---|---|
| `sideeye-core` | `ReviewPacket`, `Verdict`, validation, redaction, budgeting |
| `sideeye-capture` | OpenCode, Claude Code, and Codex adapters; git diff/manifest |
| `sideeye-providers` | Anthropic, OpenAI-compatible/local, deterministic fake, advice |
| `sideeye-cli` | setup, doctor, session, advise, validate, packet, estimate, review |

```bash
sideeye doctor
sideeye validate packet.json
sideeye packet --opencode-export export.json --redact \
  --repo . --touched-file src/parser.rs
sideeye estimate packet.json --model claude-opus-4-8
sideeye review packet.json --provider anthropic --model claude-opus-4-8
```

Python remains only as a temporary behavioral oracle for conformance and the
research sampler/calibration tools. No harness path invokes it. See
[`docs/rust-core.md`](docs/rust-core.md).

---

## Safety invariants

- **No self-grading.** A judge that generated any turn in the packet is refused.
- **Redaction before remote judging.** Credential-shaped values are replaced.
- **Context guard.** Oversized packets fail before spend.
- **Budgeting.** Transcripts and diffs are trimmed by salience, and the packet
  says when its view is partial.
- **Estimate then confirm.** The first invocation is free and never calls the
  judge.
- **Separate streams.** Random benchmark samples, human escalations, and dogfood
  reviews are never pooled.
- **Raw verdicts.** Structured records are stored and displayed without model
  paraphrase.

---

## CI/CD

- CI: Rust format/tests/clippy/release build, Python tests/version/demo checks,
  OpenCode plugin typecheck/build/tests/package.
- CD: tag `v*` publishes Linux x86_64 and macOS arm64 binaries, SHA-256
  checksums, Python wheel/sdist, and the OpenCode plugin tarball.
- Pages: `docs/` publishes the demo and docs.

See [`docs/releasing.md`](docs/releasing.md).

---

## Development

```bash
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
pytest -q
(cd integrations/opencode && npm ci && npm run typecheck && npm test)
```

Current coverage: **169 Python tests**, **73 Rust tests**, **12 OpenCode plugin
tests**.

---

## Layout

```text
crates/                     Rust core, capture, providers, CLI
adapters/                   Python conformance-oracle adapters
judge/                      Python oracle judge, schema, transcript
integrations/opencode/      native OpenCode TUI plugin
rubric/                     grading rubrics
scripts/                    version, demo, and release checks
docs/                       demo, architecture, scope, releasing
configure.py                one-command setup
escalate.py, advise.py      Python oracle review/advice engines
```

---

## Status and roadmap

Working today: Claude Code plugin, OpenCode native plugin, Codex CLI adapter,
Rust contracts/capture/estimate/Anthropic judge, secure setup, releases, demo.

Next, in order:

1. provider-neutral judging: OpenAI-compatible, gateway, and local providers
   ([#3](https://github.com/redhat-et/sideeye/issues/3));
2. Rust session review + `advise`, then native Claude Code and Codex paths;
3. local judge-success ledger and `sideeye stats`
   ([#5](https://github.com/redhat-et/sideeye/issues/5));
4. cross-harness conformance CI
   ([#2](https://github.com/redhat-et/sideeye/issues/2));
5. raw verdict viewer and scoreboard;
6. OpenClaw adapter ([#7](https://github.com/redhat-et/sideeye/issues/7)).

Windows support is explicitly out of scope for now
([#32](https://github.com/redhat-et/sideeye/issues/32)).

---

## Docs

- [Demo](https://redhat-et.github.io/sideeye/)
- [Rust core](docs/rust-core.md)
- [OpenCode adapter](docs/opencode-adapter.md)
- [Releasing](docs/releasing.md)
- [MVP scope](docs/sideeye-mvp-scope.md)
- [Design questions](docs/sideeye-phase-c-questions.md)
- [Cost story](docs/cost-savings-distilled.md)

## License

Apache-2.0. See [LICENSE](LICENSE) and [NOTICE](NOTICE).
