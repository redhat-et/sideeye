# Side-Eye

**Draft cheap. Review expensive.**

Side-Eye is an independent quality-control layer for AI coding agents. A cheap
model does the work; a stronger judge reviews the session, the diff, and the
evidence — then returns a structured verdict without letting the drafting model
grade itself.

- Native commands in **Claude Code** and **OpenCode**
- A **Codex** CLI adapter today
- A provider-neutral Rust core, with the Python engine as the behavioral oracle
- Estimate first, confirm before spend
- Self-grading, secret, context, and budget guards

**Demo:** https://redhat-et.github.io/sideeye/  
**Latest release:** https://github.com/redhat-et/sideeye/releases/tag/v0.6.4

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

> **Migration note:** setup and the Rust engine ship as `sideeye`. The Python
> CLI is `sideeye-py` and remains the session/advise fallback until those ports
> land. See [`docs/rust-migration-plan.md`](docs/rust-migration-plan.md).

Check the judge route without spending:

```bash
sideeye doctor          # offline: route guard must say "passed"
sideeye doctor --verify # free count_tokens check of route, key, and model
```

### Judge route

The judge must be a real, stronger route — never the cheap model that drafted
the session. Resolution order:

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

The plugin adds one native TUI command. With no arguments it opens a scope
picker, then asks for optional extra instructions:

```text
/sideeye                    # pick Last exchange or Whole session
/sideeye last               # estimate the last request + response
/sideeye all                # estimate the whole session
/sideeye last --confirm     # submit the last exchange
/sideeye all --confirm      # submit the whole session
/sideeye all security and missing tests
```

The scope picker shows the free estimate and asks for confirmation before any
judge spend. Extra text is reviewer instruction on top of the rubric; only
`--confirm` is special. The old `sideeye-review` server command and alias were
removed — `/sideeye` is the single user-facing command.

Flow:

```text
OpenCode export -> ReviewPacket -> redact -> budget -> estimate -> judge -> verdict
```

The verdict is inserted as a structured record. The drafting model does not
paraphrase it.

### Claude Code

```text
/sideeye:ask --validate                     # free route check
/sideeye:ask is my last change sound?       # scoped second opinion
/sideeye:ask --all                          # full-session review
```

### Codex

Codex sessions are supported through the CLI adapter:

```bash
sideeye-py review --client codex
```

A native Codex skill/plugin is tracked work; manual rollout paths are not the
target UX.

### CLI

```bash
sideeye setup                  # configure credentials and harnesses (Rust)
sideeye doctor                 # resolved route guard, no spend
sideeye-py review              # legacy full-session review
sideeye-py advise              # legacy recent scoped opinion
```

---

## Rust engine

The Rust workspace is the migration target:

| Crate | Role |
|---|---|
| `sideeye-core` | `ReviewPacket`, `Verdict`, validation, redaction, budgeting |
| `sideeye-capture` | harness adapters, currently OpenCode V2 export and git diff |
| `sideeye-providers` | `JudgeProvider`, deterministic fake, Anthropic Messages |
| `sideeye-cli` | `doctor`, `validate`, `packet`, `estimate`, `review` |

```bash
sideeye doctor
sideeye validate packet.json
sideeye packet --opencode-export export.json --redact \
  --repo . --touched-file src/parser.rs
sideeye estimate packet.json --model claude-opus-4-8
sideeye review packet.json --provider anthropic --model claude-opus-4-8
```

Python remains the fallback and the behavioral oracle until cross-harness
parity is proven. See [`docs/rust-core.md`](docs/rust-core.md).

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

Current coverage: **169 Python tests**, **50 Rust tests**, **10 OpenCode plugin
tests**.

---

## Layout

```text
crates/                     Rust core, capture, providers, CLI
adapters/                   Python Claude Code, Codex, OpenCode adapters
judge/                      Python judge, schema, transcript
integrations/opencode/      native OpenCode TUI plugin
rubric/                     grading rubrics
scripts/                    version, demo, and release checks
docs/                       demo, architecture, scope, releasing
configure.py                one-command setup
escalate.py, advise.py      Python review/advice engines
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
