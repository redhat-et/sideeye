# OpenCode V2 adapter spike

The native adapter targets OpenCode V2's plugin and local API surfaces. It does
not use MCP.

## Verified capabilities

Against OpenCode `v2.0.16`:

- server plugins can register commands; command execution receives the active
  `sessionID` and can read the projected session context;
- the session API exposes projected context and session diff operations;
- the experimental session export endpoint returns complete projected history
  and accepts `sanitize=true`;
- the export contains session identity, model/provider metadata, token totals,
  user messages, assistant text, tool parts/results, and timestamps;
- the V2 TUI plugin API can register slash commands, dialogs, toasts, routes,
  and session panels;
- VCS APIs are present for repository status and diff, but still need to be
  exercised against the resolved repository root by the native plugin.

The local probe found an important boundary: the session location was a parent
workspace directory with no `.git`; the actual repository was nested below it.
The session diff was therefore empty. External shell edits also would not be
represented as OpenCode session turns. A production adapter must resolve the
actual repository root (or receive it explicitly), then combine session history
with repository VCS state; it must not treat session diff alone as the complete
code artifact.

## Normalization contract

`adapters/opencode.py` currently converts a sanitized export into the existing
Python `SessionTranscript` contract:

- user messages become `user` turns;
- visible assistant text becomes `assistant` turns;
- completed tool text becomes `tool` turns;
- hidden reasoning, system messages, compaction summaries, and synthetic
  messages are excluded;
- provider/model and aggregate token usage are preserved, including cache read
  and write counts and per-turn generator model ids;
- touched files are supplied separately from VCS/session-diff capture.

The adapter has fixture coverage but does not yet call a real judge. The export
is sanitized by OpenCode and then redacted again by Side-Eye before judging;
`sanitize=true` is not treated as a substitute for Side-Eye redaction.

## Native command design

The next native plugin should register `/sideeye review` and:

1. obtain the active `sessionID`;
2. export/sanitize the session and resolve the repository root;
3. collect the working-tree diff and test/tool evidence;
4. construct a `ReviewPacket` and call `ReviewPacket::redacted()` (the current
   Python bridge applies the equivalent redactor before rendering);
5. reject self-grading if the selected judge model appears in any generator
   turn, then invoke the existing Python review engine through its stdin bridge, with the
   normal route guard, cost ceiling, and rubric;
6. display the structured verdict in a TUI panel or synthetic message without
   asking the current model to paraphrase it.

The plugin must remove or exclude its own review command and result from the
packet. The current model is the generator, never the judge. Dogfood results
remain separate from random benchmark samples.

The stdin bridge wrapper is:

```json
{
  "repo": "/absolute/path/to/repository",
  "diff_base": "merge-base-or-session-start-commit",
  "touched_files": [{"path": "src/parser.rs", "count": 1}],
  "data": {"info": {}, "messages": []}
}
```

`repo` is required when the OpenCode location is a workspace parent or contains
multiple repositories. The plugin must pass `--yes` for non-interactive stdin
invocation; otherwise Side-Eye prompts on `/dev/tty`-equivalent interactive
input and aborts safely at EOF.

The first native plugin implementation lives under `integrations/opencode/` and
provides both the server `sideeye-review` command and the TUI `/sideeye` slash
command. The server plugin uses the supported `ctx.session.context` API and
projects it into the adapter envelope; the generated client export endpoint is
available to external/TUI clients but is not assumed to exist on server-plugin
context. It is not automatically installed by this PR; setup will own
installation and isolated-config rollout after plugin review.

The package is pinned to the host API version used by this spike (`2.0.16`).
`npm audit` currently reports one high-severity advisory in a transitive
OpenCode/npm-registry dependency. CI typechecks and tests the plugin but does
not claim this dependency tree is release-ready. A release policy decision or
upstream remediation is required before publishing the installer.

The first invocation is estimate-only: it performs the free route/token/cost
check and inserts the estimate without judging. The user explicitly invokes
`/sideeye --confirm` before the plugin passes `--yes` to the review bridge.
The structured verdict is inserted verbatim as a synthetic message; it may be
visible to subsequent model context, but it is never routed through the model
to generate the displayed result.

## Acceptance tests before dogfooding

- active session ID is deterministic;
- export captures user intent, visible assistant claims, tool results, and
  model/token metadata, including per-turn model and cache usage;
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
