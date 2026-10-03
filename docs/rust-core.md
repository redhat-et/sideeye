# Rust core migration

The Rust migration is incremental. The existing Python engine remains the
behavioral oracle while the new core establishes the provider-neutral contracts
and native installation path.

## First slice

The workspace currently contains:

- `sideeye-core` — `ReviewPacket`, `Verdict`, evidence/artifact types, and
  validation.
- `sideeye-cli` — experimental `sideeye-rs doctor` and packet validation.

Run it with:

```bash
cargo run -p sideeye-cli -- doctor
cargo run -p sideeye-cli -- validate packet.json
```

The `doctor` command is intentionally offline. It reports configuration without
spending or sending a judge request. The packet validator accepts a JSON file or
`-` for stdin.

## Next slices

1. Add a fake judge provider and golden packet/verdict fixtures.
2. Add the Anthropic provider adapter with parity against the Python engine.
3. Add the OpenCode native adapter so Side-Eye can review its own development
   sessions.
4. Add Codex and Claude Code native integrations through the same packet API.
5. Add OpenAI-compatible and local judge providers.
6. Replace the experimental binary name with the release `sideeye` command once
   the conformance matrix is green.
