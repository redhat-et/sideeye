# Side-Eye OpenCode plugin

This V2 plugin registers one user-facing TUI command: `/sideeye`.

- `/sideeye` opens a scope picker: **Last exchange** or **Whole session**, then
  asks for optional extra instructions.
- `/sideeye last [text]`, `/sideeye all [text]`, and `--confirm` are slash
  shortcuts.
- The plugin uses the Rust engine when `options.engine` is `"rust"` and
  `options.rust` points at the `sideeye` command. `sideeye setup` selects Rust
  automatically when `sideeye` is on PATH.

The command reads the full sanitized OpenCode session export, resolves the
repository, collects the branch/worktree code artifact, builds the packet with
the selected scope, and invokes the Rust engine. The first invocation is a free
estimate. The picker flow shows the estimate and asks for confirmation before
judging; the slash flow inserts the exact `/sideeye ... --confirm` command to
run next. The final verdict is inserted as a synthetic session message and is
never sent back through the drafting model for paraphrasing.

There is no server-side command. The package keeps an empty `server.js`
entrypoint because OpenCode 2.0.x expects both root entrypoints; the product
command lives in `tui.js`.

`sideeye setup` installs this package into the Side-Eye data directory and
writes the OpenCode plugin configuration.
