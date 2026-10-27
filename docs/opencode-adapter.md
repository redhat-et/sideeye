# OpenCode adapter

The native adapter targets OpenCode V2's plugin and local API surfaces. It does
not use MCP.

## Product surface

There is one user-facing command: `/sideeye` in the OpenCode TUI.

- `/sideeye` first asks for optional focus instructions in a text prompt
  (type, then Enter).
- It then opens a scope selector: **Last exchange** or **Whole session**
  (arrow keys, then Enter).
- It shows the free estimate and asks for confirmation before judging.
- `/sideeye last [text]`, `/sideeye all [text]`, and `--confirm` are slash
  shortcuts for power users.
- The old server-side `sideeye-review` command and the TUI alias were removed.

"Last exchange" means the last real user request plus the assistant work that
followed it. Side-Eye's own estimate/verdict output is excluded from the packet.

## Verified capabilities

Against OpenCode `v2.0.16`:

- the TUI plugin API can register slash commands, dialogs, toasts, routes, and
  session panels;
- the generated client exposes the full sanitized session export
  (`session.export({sanitize:true})`);
- the export contains session identity, model/provider metadata, token totals,
  user messages, assistant text, tool parts/results, and timestamps;
- the session API exposes projected context and session diff operations;
- VCS APIs exist for repository status and diff.

The session location can be a parent workspace with no `.git`; the actual
repository may be nested below it. The adapter resolves the repository root
from the session location and tool inputs, or receives it explicitly through
`sideeye setup --repo`. External shell edits are captured through the git diff,
not treated as OpenCode turns.

## Capture contract

The Rust capture adapter converts a sanitized OpenCode export into a
`ReviewPacket`:

- user messages become `user` turns;
- visible assistant text becomes `assistant` turns;
- completed tool text becomes `tool` turns;
- hidden reasoning, system messages, compaction summaries, synthetic messages,
  and Side-Eye's own output are excluded;
- provider/model and aggregate token usage are preserved, including cache read
  and write counts and per-turn generator model ids;
- the last-exchange scope keeps only the last real user request and subsequent
  assistant work; whole-session usage is not reused for a single exchange;
- the git diff and a `manifest` artifact list the reviewed files.

The export is sanitized by OpenCode and redacted again by Side-Eye before
judging. `sanitize=true` is not a substitute for Side-Eye redaction.

## Flow

```text
OpenCode export
  -> ReviewPacket (scope: session or last-exchange)
  -> redact
  -> budget
  -> free count_tokens estimate
  -> confirm
  -> judge
  -> validated verdict
  -> synthetic message + local JSONL record
```

The plugin passes `--scope session|last-exchange` to `sideeye packet`. The Rust
engine handles the packet, estimate, guards, judging, and persisted record.

The first invocation is estimate-only: it performs the free route/token/cost
check and does not judge. If the context guard rejects the packet, the plugin
reduces the character budget and retries with the exact free token count before
showing the estimate. The picker flow shows that estimate in a confirmation
dialog; the slash flow inserts it with the exact `/sideeye ... --confirm`
command to run next. The structured verdict is inserted verbatim as a synthetic
message and is never routed through the drafting model to generate the display.

## Setup

`sideeye setup` copies the package to the user data directory, builds its
JavaScript entrypoints, and adds it to the OpenCode plugin list. OpenCode 2.0.x
requires the package directory to expose root `server.js` and `tui.js`
entrypoints; the server entrypoint is intentionally empty because the product
command lives in the TUI.

The plugin package is pinned to the host API version used for this adapter
(`2.0.16`). `npm audit` currently reports one high-severity advisory in a
transitive OpenCode/npm-registry dependency. CI typechecks and tests the plugin
but does not claim this dependency tree is release-ready.

## Acceptance tests

- active session ID and full export are deterministic;
- the scope picker and `/sideeye last|all` both reach the same packet builder;
- last exchange excludes earlier turns and Side-Eye output;
- export captures intent, visible claims, tool results, and model/token metadata;
- external edits and OpenCode edits both appear in the code artifact;
- a nested repository is resolved explicitly rather than inferred from the
  workspace parent;
- secrets are redacted and no raw credentials enter the judge request;
- a session containing turns from the judge model is rejected before spend;
- tool inputs and failed-tool markers remain visible as evidence;
- the plugin works with no judge credential by failing before network spend;
- judge output is rendered verbatim and is not sent back through the model;
- review invocation/result are absent from the reviewed packet;
- current OpenCode configuration remains unchanged in an isolated test HOME.
