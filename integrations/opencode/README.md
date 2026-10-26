# Side-Eye OpenCode plugin

This V2 plugin registers the native `sideeye-review` command and the TUI
`/sideeye` slash command. It uses the Python bridge by default, or the Rust
engine when `options.engine` is `"rust"` and `options.rust` points at the
`sideeye` command. `sideeye-py setup` selects Rust automatically when
`sideeye` is on PATH. The command receives the active OpenCode session ID,
reads the supported projected session context,
resolves the nested Git repository from the session/tool paths, collects the
branch/worktree code artifact, and invokes the existing Python review bridge
with `--opencode-export - --yes --json`.

The first invocation only displays a cost estimate. The user must invoke
`/sideeye --confirm` to submit the packet. The final result is inserted as a
synthetic session message containing the structured record. It is not sent back
through the current model for paraphrasing.

This package is intentionally not auto-installed by the current PR. Setup will
eventually add it to the user's OpenCode plugin configuration. Until then it is
safe to inspect and test without modifying the developer's OpenCode config.
