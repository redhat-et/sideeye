# Rust core

The Rust core is the product engine. Python remains only as a temporary
behavioral oracle for the not-yet-ported Claude Code, Codex, and advise paths.

## Workspace

- `sideeye-core` — `ReviewPacket`, `Verdict`, evidence/artifact types,
  validation, redaction, and budgeting.
- `sideeye-capture` — capture adapters: OpenCode V2 export with whole-session
  and last-exchange scopes, plus git diff and manifest evidence.
- `sideeye-providers` — the provider-neutral `JudgeProvider` trait, a
  deterministic fake provider, Anthropic Messages, and OpenAI-compatible Chat
  Completions (OpenAI, gateways, Ollama, LM Studio, vLLM).
- `sideeye-cli` — `setup`, `doctor`, packet validation, estimate, and review.

## Commands

```bash
sideeye setup
sideeye setup --provider openai-compatible \
  --judge-base-url http://127.0.0.1:11434 \
  --judge-api-key local --model qwen2.5-coder
sideeye doctor
sideeye doctor --verify
sideeye validate packet.json
sideeye packet --opencode-export export.json --redact \
  --scope last-exchange
sideeye estimate packet.json --provider anthropic --model claude-opus-4-8
sideeye review packet.json --provider anthropic --model claude-opus-4-8
sideeye review packet.json --provider openai-compatible --model qwen2.5-coder
sideeye review --provider fake --fixture verdict.json packet.json
```

`estimate` never calls the judge. Anthropic uses the free `count_tokens`
endpoint and exact pricing. OpenAI-compatible routes use a labeled heuristic
because there is no standard free count endpoint; their cost is reported as
unknown unless the gateway provides its own metering. Local routes need a
placeholder credential (`local`) only because the route guard requires one.

Both `estimate` and `review` refuse a judge model that also generated turns in
the packet, preventing self-grading. `doctor` is offline by default;
`doctor --verify` performs a free route check (`count_tokens` or `/v1/models`).

## Provider contract

Every provider returns the same validated `Verdict` with packet/rubric/judge
provenance, usage, issues, severity, and summary. The packet is redacted before
it leaves the process. OpenAI-compatible transport supports:

- `tool_calls[0].function.arguments` (primary);
- legacy `function_call.arguments`;
- plain JSON content, including fenced JSON, for local models without tools.

Routes without tool calling should fail closed with an actionable error. A
truncated verdict (`finish_reason: length` or `stop_reason: max_tokens`) is
rejected rather than retried with the same budget.

## Next

1. Port Claude Code and Codex capture plus session review and `advise` to Rust.
2. Run cross-harness conformance against the Python oracle.
3. Add the local judge-success ledger and `sideeye stats`.
4. Delete Python once all gates pass.

The judge result is stored in its original structured form and displayed
without an intermediate drafting model rewriting it. Random benchmark samples,
human escalations, and dogfood reviews remain separate streams.
