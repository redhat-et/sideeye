# Rust core migration

The Rust migration is incremental. The existing Python engine remains the
behavioral oracle while the new core establishes the provider-neutral contracts
and native installation path.

## First slice

The workspace currently contains:

- `sideeye-core` — `ReviewPacket`, `Verdict`, evidence/artifact types, and
  validation and packet redaction.
- `sideeye-providers` — the provider-neutral `JudgeProvider` trait and a
  deterministic fake provider for hermetic tests.
- `sideeye-cli` — experimental `sideeye-rs doctor` and packet validation.

Run it with:

```bash
cargo run -p sideeye-cli -- doctor
cargo run -p sideeye-cli -- validate packet.json
```

The `doctor` command is intentionally offline. It resolves the same route chain
as the Python oracle: `--base-url`, `SIDEEYE_JUDGE_*`,
`$SIDEEYE_CONFIG` (or `~/.config/sideeye/config.json`), then ambient
`ANTHROPIC_*`. It reports configuration without spending or sending a judge
request, and blocks ambient private/non-Anthropic routes unless
`judge.trust_ambient_route` is explicitly enabled. The packet validator accepts
a JSON file or `-` for stdin.

The Rust `Verdict` accepts the Python judge's raw core result during migration.
Runner-added metadata such as `packet_id`, `rubric_version`, judge identity,
usage, and cost is optional at the raw boundary but should be populated before
persisting a comparable record. Issue evidence is optional for compatibility
with existing Python verdicts and should be supplied by new providers.

## Next slices

1. Add golden fixtures from real Python `SessionTranscript` and judge output;
   round-trip them through Rust before freezing the contract.
2. Extend the fake provider contract tests with cost limits, raw-verdict
   preservation, and rubric/packet provenance.
3. Spike OpenCode capabilities: active session identity, complete history/tool
   results, changed-file evidence, and output that reaches the user without
   model paraphrase. Record the result before choosing plugin APIs.
4. Build an OpenCode adapter v0 that emits a packet to the existing Python
   engine. This enables dogfooding before the Rust provider adapter exists.
   Call `ReviewPacket::redacted()` immediately before any packet leaves the
   local process; redaction is mandatory, not an adapter-specific option.
5. Add the Rust Anthropic adapter with parity against the Python engine, then
   OpenAI-compatible and local providers.
6. Add Codex and Claude Code native integrations through the same packet API,
   followed by hermetic conformance CI.
7. Define the release distribution path (prebuilt binaries and installer) and
   replace the experimental binary name with the release `sideeye` command only
   after the conformance matrix is green.

The judge result must be stored in its original structured form and displayed
without an intermediate drafting model rewriting it. A future `show` command
can render the stored record, but it must not replace the raw verdict. Random
benchmark samples, human escalations, and dogfood reviews remain separate
streams.
