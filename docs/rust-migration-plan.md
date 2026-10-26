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

Transitional note: setup is still `sideeye-py setup` until Phase 1 lands.

### Phase 1 — Rust setup (in progress)

Ported:

- [x] Side-Eye config path and atomic mode-600 write;
- [x] hidden credential prompt and noninteractive fail-closed;
- [x] base URL normalization;
- [x] preserve unrelated config keys;
- [x] detect Claude Code, Codex, and OpenCode;
- [x] `sideeye setup --no-opencode`.

Pending:

- [ ] OpenCode profile discovery, including the EnMaaS wrapper;
- [ ] JSON/JSONC update without dropping comments;
- [ ] embed/copy the OpenCode plugin and run `npm ci` + `npm run build`;
- [ ] write plugin options: `engine`, `rust`, `repo`.

Gate: an isolated `HOME`/config setup produces the same files as Python; no
normal configuration writes.

### Phase 2 — Rust capture parity

Port:

- Claude Code transcript adapter;
- Codex rollout adapter;
- current-session resolution (including `CLAUDE_CODE_SESSION_ID`);
- recent-exchange scoping for `advise`;
- code artifact parity (manifests, partial views, generated files).

Gate: golden fixtures from the Python adapters parse identically in Rust.

### Phase 3 — Rust CLI surfaces

Port:

- `sideeye review` session mode (not just packet mode);
- `sideeye advise`;
- `sideeye route`;
- cost reporting used by the product;
- decide whether sampler/calibration stay as research tools or retire.

Gate: Claude Code, Codex, and OpenCode can run without Python.

### Phase 4 — conformance and cutover

- Run Python and Rust against the same fixtures and real sessions.
- Cross-harness CI with isolated HOME/config/repo per harness.
- Compare verdict semantic fields, evidence, provenance, and cost.
- Keep a one-release fallback to `sideeye-py`.

Gate: Rust tests cover at least the same behaviors as Python; all golden
fixtures pass; all three harnesses pass end to end; the release artifact
installs and runs.

### Phase 5 — delete Python

- Remove the Python console entrypoint and package.
- Delete Python adapters/judge/sampler/cost code.
- Keep only committed fixtures needed by Rust tests.
- Update CI to Rust + plugin only.
- Update README, docs, demo, and release notes.

## Test coverage target

Current:

- Python: 169 tests.
- Rust: 36 tests.
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
4. Port Claude Code and Codex capture.
5. Port advise and session review.
6. Conformance matrix.
7. Delete Python.

## Keeping this plan current

Update this file in the same PR as any phase change. The README and demo must
also match the current command names and engine. CI checks the demo/version
drift; this plan is reviewed manually at each phase.
