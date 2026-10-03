# Side-Eye skill (Claude Code plugin source)

The canonical skill definition, shipped as part of the `sideeye` Claude Code
plugin. Install it with the plugin — no file copying:

```
/plugin marketplace add redhat-et/sideeye
/plugin install sideeye@sideeye
```

It appears as **`/sideeye:ask`** (personal-scope `cp -r` installs of the old
`escalate-*` skills are the pre-plugin flow — delete them; a duplicate command
shadows the plugin one).

For plugin development, install the marketplace from a local checkout:

```
/plugin marketplace add /path/to/sideeye/checkout
```

## What it does

One router command; the first matching rule wins:

| You type | What runs | Cost |
|---|---|---|
| `/sideeye:ask` or `--help` | prints usage | $0 |
| `/sideeye:ask --validate` | `sideeye route` — route resolution + guards | $0 |
| `/sideeye:ask --all [flags]` | `sideeye review --current --yes` — full sighted review (transcript + `git diff`) | median ~$0.75 |
| `/sideeye:ask [–turn N] <text> [flags]` | `sideeye advise --current --yes --question "<text>"` — scoped opinion on recent work | ~$0.02–$1 |

Pass-through flags: `--model fable|opus|sonnet|haiku`, `--no-code`, `--max-cost N`.
The authoritative lists are `sideeye review --help` / `sideeye advise --help`.

The skill is **user-invoked only** (`disable-model-invocation: true`) because it
spends real money, and it **presents the tool output verbatim and never lets the
model improvise a verdict** — a review invented by the cheap model is the exact
failure Side-Eye exists to prevent.

## Requirements at runtime
- The `sideeye` engine. The skill self-installs it on first use (one-time ~30s,
  needs `uv` or `pipx` on PATH); or pre-install: `uv tool install
  git+https://github.com/redhat-et/sideeye`.
- A usable judge route — see "Configure the judge route" in the root
  [README](../README.md) (env vars, `~/.config/sideeye/config.json`, or ambient
  ANTHROPIC_* with an Anthropic host). The skill's shell environment is the
  session's own, so in a cheap-model session the route must be explicit or the
  guard blocks the review — `/sideeye:ask --validate` (free) shows exactly what
  resolves and why.
