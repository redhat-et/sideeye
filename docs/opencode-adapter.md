# OpenCode V2 adapter spike

The native adapter targets OpenCode V2's plugin and local API surfaces. It does
not use MCP.

## Verified capabilities

Against OpenCode `v2.0.16`:

- server plugins can register commands; command execution receives the active
  `sessionID`;
- the session API exposes projected context and session diff operations;
- the experimental session export endpoint returns complete projected history
  and accepts `sanitize=true`;
- the export contains session identity, model/provider metadata, token totals,
  user messages, assistant text, tool parts/results, and timestamps;
- the V2 TUI plugin API can register slash commands, dialogs, toasts, routes,
  and session panels;
- VCS APIs expose repository status and diff through the resolved project
  location.

The local probe found an important boundary: this development session's edits
were made by external shell tools, so OpenCode's session diff was empty even
though the repository had changes. A production adapter must combine session
history with repository VCS state; it must not treat session diff alone as the
complete code artifact.

## Normalization contract

`adapters/opencode.py` currently converts a sanitized export into the existing
Python `SessionTranscript` contract:

- user messages become `user` turns;
- visible assistant text becomes `assistant` turns;
- completed tool text becomes `tool` turns;
- hidden reasoning, system messages, compaction summaries, and synthetic
  messages are excluded;
- provider/model and aggregate token usage are preserved;
- touched files are supplied separately from VCS/session-diff capture.

The adapter has fixture coverage but does not yet call a real judge.

## Native command design

The next native plugin should register `/sideeye review` and:

1. obtain the active `sessionID`;
2. export/sanitize the session and resolve the repository root;
3. collect the working-tree diff and test/tool evidence;
4. construct a `ReviewPacket` and call `ReviewPacket::redacted()`;
5. invoke the existing Python review engine through its stdin bridge, with the
   normal route guard, cost ceiling, and rubric;
6. display the structured verdict in a TUI panel or synthetic message without
   asking the current model to paraphrase it.

The plugin must remove or exclude its own review command and result from the
packet. The current model is the generator, never the judge. Dogfood results
remain separate from random benchmark samples.

## Acceptance tests before dogfooding

- active session ID is deterministic;
- export captures user intent, visible assistant claims, tool results, and
  model/token metadata;
- external edits and OpenCode edits both appear in the code artifact;
- secrets are redacted and no raw credentials enter the judge request;
- the plugin works with no judge credential by failing before network spend;
- judge output is rendered verbatim and is not sent back through the model;
- review invocation/result are absent from the reviewed packet;
- current OpenCode configuration remains unchanged in an isolated test HOME.
