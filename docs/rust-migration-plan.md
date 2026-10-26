# Rust migration and product plan

## Goal

Ship **one** command: `sideeye`, implemented in Rust. Python is a temporary
behavioral oracle and is then deleted.

The product is intentionally small:

1. Run `sideeye setup` once.
2. Open a coding session.
3. Run `/sideeye` in OpenCode.
4. Choose **Last exchange** or **Whole session**.
5. Add optional extra instructions on top of the rubric.
6. Confirm the free estimate; the judge's verdict is shown verbatim.

No `sideeye-rs`. No MCP. No Python runtime in the product path. No Windows
support in the current product; see issue #32.

## Current decisions

- One user-facing command in OpenCode: `/sideeye`. The server-side
  `sideeye-review` command and the TUI alias were removed in 0.6.4.
- `/sideeye` with no arguments opens a scope picker. Slash shortcuts remain:
  `/sideeye last [text]`, `/sideeye all [text]`, and `--confirm` to submit.
- "Last exchange" is the last real user request plus the assistant work that
  followed it. Side-Eye's own output is excluded.
- Setup is a guided wizard in a terminal: it asks only for missing values,
  installs the detected harness plugin, and verifies the route with a free
  `count_tokens` call. `--yes` supports automation; `--help` has examples.
- Provider-neutral judging is the next engine milestone: Anthropic-compatible,
  OpenAI-compatible, gateway, and local providers behind one contract.
- The judge-success metrics milestone starts local-only: labels, precision,
  catch rate, false-positive rate, and `sideeye stats`. No upload yet.
- Windows is explicitly out of scope for now (issue #32).

## Why Python is still here

The Rust engine covers the OpenCode path end to end, but these shipped surfaces
still depend on Python:

| Surface | Python dependency | Rust status |
|---|---|---|
| `sideeye setup` | config, JSONC, plugin install/build | ported (0.6.1) |
| OpenCode review | bridge and fallback | Rust path is the product |
| Claude Code skill | `sideeye review` / `advise` session discovery | not ported |
| Codex CLI | rollout discovery + adapter | not ported |
| `sideeye advise` | recent-exchange scoping | not ported |
| provider neutrality | OpenAI-compatible/local routes | not ported |
| sampler / cost report / calibration | research streams | not ported |
| judge-success metrics | labels and local stats | not started |

Deleting Python today would break Claude Code, Codex, advise, and the research
streams.

## Naming

During migration:

- `sideeye` — Rust binary, the product.
- `sideeye-py` — temporary Python oracle/research CLI; not a product path.

After the gates below:

- delete `sideeye-py` and the Python console entrypoint;
- keep Python fixtures/tests only if they serve as golden data, then delete.

## Phases and gates

### Phase 0 — naming and UX (done in 0.5.0)

- Renamed the Rust binary `sideeye-rs` → `sideeye`.
- Renamed the Python entrypoint `sideeye` → `sideeye-py`.
- Added `sideeye --version`.
- Updated plugin defaults, release artifacts, docs, and the demo.
- Fixed the misleading “focus” wording: `/sideeye <free text>` is the syntax.

Gate: `sideeye --version` resolves to Rust; `sideeye-py` is used only for
oracle/research checks; no harness path invokes it.

### Phase 1 — Rust setup (complete in 0.6.1)

- [x] Side-Eye config path and atomic mode-600 write;
- [x] hidden credential prompt and noninteractive fail-closed;
- [x] base URL normalization;
- [x] preserve unrelated config keys;
- [x] detect Claude Code, Codex, and OpenCode;
- [x] OpenCode profile discovery, including the EnMaaS wrapper;
- [x] JSON/JSONC update without dropping comments;
- [x] embed/copy the OpenCode plugin and run `npm ci` + `npm run build`;
- [x] write plugin options: `engine`, `rust`, `repo`, `model`, `maxTokens`;
- [x] `sideeye setup` is the complete setup path;
- [x] explicit `--repo` for multi-repository workspaces.

Gate: isolated tests cover secure config, JSONC preservation, plugin install,
plugin options, free route verification, and noninteractive failure.

### Phase 2 — OpenCode completion (current)

- [x] Native `/sideeye` command only; server command and duplicate alias removed.
- [x] Estimate then confirm.
- [x] Interactive scope picker: last exchange or whole session.
- [x] Slash shortcuts: `/sideeye last`, `/sideeye all`, optional instructions.
- [x] Rust engine selected by setup.
- [x] Diff base, redaction, budgeting, context guard, self-grading guard.
- [x] Plugin install and JSONC/profile detection.
- [x] Raw verdict inserted without paraphrase and persisted outside context.
- [x] Show packet scope and judge model in the estimate.
- [x] Manifest artifact for reviewed files.
- [x] Hard timeout on the plugin's engine/judge call.
- [x] Guided setup wizard and free `count_tokens` verification.
- [x] `sideeye --help`, `sideeye setup --help`, and `sideeye doctor --help`
      with examples.
- [x] Manual fake-judge emulation with the installed plugin and installed Rust
      binary: estimate, confirm, verdict, and JSONL persistence.
- [x] Hermetic OpenCode picker + fake-engine test in CI: scope, instructions,
      confirmation, packet arguments, and verdict insertion without network.
- [ ] Manual acceptance test by the maintainer in the real OpenCode TUI:
      picker, last exchange, whole session, instructions, confirm, verdict,
      error paths, multi-repo.

Gate: the maintainer runs the OpenCode flow end to end and confirms that
nothing is missing.

### Phase 3 — provider-neutral judging (item 11, 0.6.6)

- [x] OpenAI-compatible Chat Completions provider;
- [x] local provider support through the same OpenAI-compatible boundary
      (Ollama, LM Studio, vLLM, gateways);
- [x] provider selection in config, CLI, and setup;
- [x] provider-specific estimate behavior (exact count_tokens for Anthropic,
      labeled heuristic and unknown pricing for OpenAI-compatible);
- [x] same verdict contract, redaction, guards, and provenance across providers;
- [x] provider conformance tests: tool-call parsing, JSON-content fallback,
      truncation, CLI end to end, setup with a local route.

Gate: the same packet can be judged through Anthropic and an OpenAI-compatible
local/gateway route with the same validated verdict contract.

### Phase 4 — Rust session review, advise, Claude Code, and Codex (items 12–13, 0.6.7)

- [x] generic session capture for Claude Code JSONL and Codex rollouts;
- [x] current-session resolution, including `CLAUDE_CODE_SESSION_ID`;
- [x] code artifact capture, including a fallback to all changed/untracked files;
- [x] `sideeye session --client claude|codex --current`;
- [x] `sideeye advise --client claude|codex --current --question ...`;
- [x] native Claude Code skill installed by setup (`~/.claude/skills/sideeye`);
- [x] native Codex skill installed by setup (`~/.agents/skills/sideeye`);
- [x] remove Python from OpenCode, Claude Code, Codex, and setup paths.

Gate: Claude Code, Codex, and OpenCode run without Python. Golden-fixture
conformance and full Python deletion remain Phase 6.

### Phase 5 — conformance, metrics, and cutover (item 14, 0.6.7)

- [x] local judge-success labels: confirmed / false-positive / neutral;
- [x] `sideeye label` and `sideeye stats`;
- [x] precision, false-positive rate, coverage, and cost per confirmed finding;
- [x] local ledger contains only record ids, labels, and verdict cost; no
      prompts, code, paths, or keys;
- [ ] Run Python and Rust against the same fixtures and real sessions.
- [ ] Cross-harness CI with isolated HOME/config/repo per harness.
- [ ] Compare verdict semantic fields, evidence, provenance, and cost.
- [ ] Opt-in aggregate upload (deferred; local-only milestone first).

Gate: Rust tests cover the same behaviors as Python; all golden fixtures pass;
all three harnesses pass end to end; the local metrics ledger and stats work.

### Phase 6 — delete Python

- Remove the Python console entrypoint and package.
- Delete Python adapters/judge/sampler/cost code.
- Keep only committed fixtures needed by Rust tests.
- Update CI to Rust + plugin only.
- Update README, docs, demo, and release notes.

## Test coverage target

Current (0.6.7 development):

- Python: 169 tests.
- Rust: 71 tests.
- OpenCode plugin: 10 tests.

The Rust suite must reach behavioral parity, not merely the same count. Track
coverage by surface:

- capture adapters and scopes;
- session resolution;
- packet budget and redaction;
- providers and guards;
- setup and config;
- CLI end-to-end.

Python tests stay until their Rust equivalents and golden fixtures exist. Do
not delete the oracle before the gate.

## Order of work

1. Plan + wording (this document).
2. Rename binary and Python entrypoint.
3. Port setup to Rust.
4. Complete OpenCode and pass the maintainer acceptance gate.
5. Provider-neutral judging.
6. Port session review, advise, and Claude Code / Codex capture.
7. Conformance matrix and local metrics.
8. Delete Python.

## Out of scope

- Windows support (tracked in #32).
- MCP transport.
- Judge-success aggregate upload until the local ledger is proven.

## Versioning policy

Stay on the `0.6.x` line. Use patch bumps (`0.6.1`, `0.6.2`, ...) until the
maintainer explicitly authorizes a minor or major release.

## Keeping this plan current

Update this file in the same PR as any phase change. The README and demo must
also match the current command names and engine. CI checks the demo/version
drift; this plan is reviewed manually at each phase.
