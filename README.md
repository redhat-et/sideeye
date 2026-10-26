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
**Latest release:** https://github.com/redhat-et/sideeye/releases/tag/v0.4.1

---

## Install

```bash
uv tool install git+https://github.com/redhat-et/sideeye
# or: pipx install git+https://github.com/redhat-et/sideeye
# or: pip install git+https://github.com/redhat-et/sideeye
```

Then run the one-time setup:

```bash
sideeye setup
```

Setup:

- asks once for a hidden judge credential;
- stores it in Side-Eye config with mode `600`;
- detects Claude Code, Codex, and OpenCode;
- installs and configures the bundled OpenCode plugin;
- preserves unrelated JSON/JSONC settings;
- selects the Rust engine when `sideeye-rs` is on `PATH`.

Check the judge route without spending:

```bash
sideeye route          # must say: verdict: OK
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

The plugin adds a native TUI command and a server command:

```text
/sideeye                         # free estimate
/sideeye --confirm               # submit to the judge
/sideeye focus on security       # focus instructions
/sideeye --confirm focus on security
/sideeye-review                  # server command
```

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
sideeye review --client codex
```

A native Codex skill/plugin is tracked work; manual rollout paths are not the
target UX.

### CLI

```bash
sideeye review                 # full session + diff + evidence
sideeye advise                 # recent scoped second opinion
sideeye route                  # resolved judge route, no spend
sideeye setup                  # configure credentials and harnesses
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
sideeye-rs doctor
sideeye-rs validate packet.json
sideeye-rs packet --opencode-export export.json --redact \
  --repo . --touched-file src/parser.rs
sideeye-rs estimate packet.json --model claude-opus-4-8
sideeye-rs review packet.json --provider anthropic --model claude-opus-4-8
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

Current coverage: **169 Python tests**, **36 Rust tests**, **7 OpenCode plugin
tests**.

---

## Layout

```text
crates/                     Rust core, capture, providers, CLI
adapters/                   Python Claude Code, Codex, OpenCode adapters
judge/                      Python judge, schema, transcript
integrations/opencode/      native OpenCode server/TUI plugin
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

Next:

- cross-harness conformance CI ([#2](https://github.com/redhat-et/sideeye/issues/2))
- provider-neutral Rust engine completion ([#3](https://github.com/redhat-et/sideeye/issues/3))
- opt-in, privacy-preserving judge metrics ([#5](https://github.com/redhat-et/sideeye/issues/5))
- native Codex UX
- whole-repository review scope
- raw verdict viewer and scoreboard
- OpenClaw adapter ([#7](https://github.com/redhat-et/sideeye/issues/7))

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
