# Rust migration and Python removal plan

## Goal

Ship **one** command: `sideeye`, implemented in Rust. Python becomes a
temporary behavioral oracle and is then deleted.

No `sideeye-rs`. No Python runtime in the product path.

## Why Python is still here

The Rust engine covers the OpenCode path, but several shipped surfaces still
depend on Python:

| Surface | Python dependency | Rust status |
|---|---|---|
| `sideeye setup` | config, JSONC, plugin install/build | not ported |
| Claude Code skill | `sideeye review` / `advise` session discovery | not ported |
| Codex CLI | rollout discovery + adapter | not ported |
| `sideeye advise` | recent-exchange scoping | not ported |
| `sideeye route` | route chain + guards | ported to `sideeye-rs doctor` |
| OpenCode review | bridge and fallback | Rust path available |
| sampler / cost report / calibration | research streams | not ported |
| Judge + verdict schema | Anthropic-specific | Rust contracts + provider |

Deleting Python today would break setup, Claude Code, Codex, and advise.

## Naming

During migration:

- `sideeye` — Rust binary, the product.
- `sideeye-py` — legacy Python CLI, temporary oracle/fallback.

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

Gate: `sideeye --version` resolves to Rust; `sideeye-py` remains the explicit
legacy command; OpenCode setup selects the right one.

`sideeye-py setup` remains only as a fallback for older installations.

### Phase 1 — Rust setup (complete in 0.6.1)

- [x] Side-Eye config path and atomic mode-600 write;
- [x] hidden credential prompt and noninteractive fail-closed;
- [x] base URL normalization;
- [x] preserve unrelated config keys;
- [x] detect Claude Code, Codex, and OpenCode;
- [x] OpenCode profile discovery, including the EnMaaS wrapper;
- [x] JSON/JSONC update without dropping comments;
- [x] embed/copy the OpenCode plugin and run `npm ci` + `npm run build`;
- [x] write plugin options: `engine`, `rust`, `repo`;
- [x] `sideeye setup` is the complete setup path;
- [x] explicit `--repo` for multi-repository workspaces.

Gate: isolated tests cover secure config, JSONC preservation, plugin install,
plugin options, and noninteractive failure. `sideeye setup` was run on a real
profile and produced the expected OpenCode configuration.

### Phase 2 — OpenCode completion (current)

Finish the OpenCode product path before touching Claude Code or Codex.

- [x] Native `/sideeye` and `/sideeye-review`.
- [x] Rust engine selected by setup.
- [x] Estimate then confirm, with free-text focus.
- [x] Diff base, redaction, budgeting, context guard, self-grading guard.
- [x] Plugin install and JSONC/profile detection.
- [x] Raw verdict inserted without paraphrase.
- [x] Show packet scope in the estimate: turns, diff artifact, complete/partial.
- [x] Persist the raw verdict record outside the model context.
- [x] Enforce a hard timeout on the plugin's engine/judge call.
- [ ] Isolated OpenCode smoke test with a fake judge (no network).
- [ ] Manual acceptance test by the maintainer: estimate, confirm, verdict,
      error paths, focus text, multi-repo selection.

Gate: the maintainer runs the OpenCode flow end to end and confirms that
nothing is missing. Claude Code and Codex work does not start until this gate
passes.

### Phase 3 — Claude Code and Codex capture parity

- Claude Code transcript adapter;
- Codex rollout adapter;
- current-session resolution (including `CLAUDE_CODE_SESSION_ID`);
- code artifact parity (manifests, partial views, generated files).

Gate: golden fixtures from the Python adapters parse identically in Rust.

### Phase 4 — Rust CLI surfaces

- `sideeye review` session mode (not just packet mode);
- `sideeye advise`;
- `sideeye route`;
- cost reporting used by the product;
- decide whether sampler/calibration stay as research tools or retire.

Gate: Claude Code, Codex, and OpenCode can run without Python.

### Phase 5 — conformance and cutover

- Run Python and Rust against the same fixtures and real sessions.
- Cross-harness CI with isolated HOME/config/repo per harness.
- Compare verdict semantic fields, evidence, provenance, and cost.
- Keep a one-release fallback to `sideeye-py`.

Gate: Rust tests cover at least the same behaviors as Python; all golden
fixtures pass; all three harnesses pass end to end; the release artifact
installs and runs.

### Phase 6 — delete Python

- Remove the Python console entrypoint and package.
- Delete Python adapters/judge/sampler/cost code.
- Keep only committed fixtures needed by Rust tests.
- Update CI to Rust + plugin only.
- Update README, docs, demo, and release notes.

## Test coverage target

Current:

- Python: 169 tests.
- Rust: 45 tests.
- OpenCode plugin: 7 tests.

The Rust suite must reach behavioral parity, not merely the same count. Track
coverage by surface:

- capture adapters;
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
5. Port Claude Code and Codex capture.
6. Port advise and session review.
7. Conformance matrix.
8. Delete Python.

## Versioning policy

Stay on the `0.6.x` line. Use patch bumps (`0.6.1`, `0.6.2`, ...) until the
maintainer explicitly authorizes a minor or major release.

## Keeping this plan current

Update this file in the same PR as any phase change. The README and demo must
also match the current command names and engine. CI checks the demo/version
drift; this plan is reviewed manually at each phase.
