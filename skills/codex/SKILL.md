---
name: sideeye
description: Ask the Side-Eye judge to review this Codex session or give a second opinion on recent work. Use when the user explicitly invokes Side-Eye, asks for an independent review, or asks whether recent work is sound. Spends money unless the user runs --validate or --help.
---

# Side-Eye judge for Codex

Side-Eye sends this Codex session (or recent exchanges) to a stronger,
independent judge. The judge's output must be shown verbatim; never invent or
paraphrase a verdict.

## Modes

| User typed | Command |
|---|---|
| nothing or `help` | print the usage below; no spend |
| `--validate` | `sideeye doctor --verify` (free) |
| `--all` | `sideeye session --client codex --current` |
| anything else | `sideeye advise --client codex --current --question "<text>"` |

- `--turn N` / `--turns N` adds `--turns N` to advise.
- Pass through `--model`, `--provider`, `--no-code`, `--repo`, and `--base-url`
  when the user typed them.
- Run with a generous timeout (≥300000 ms) from the session project directory.
- Present stdout verbatim. On failure, show stderr and stop.
- Never write a verdict or advice yourself.

## Usage

```
/skills sideeye --validate            free route check
/skills sideeye --all                 full session review + diff
/skills sideeye is this change sound? second opinion on recent work
/skills sideeye --turn 3 <question>   second opinion over 3 exchanges
```

## Requirements

- `sideeye` on PATH (or use `$HOME/.local/bin/sideeye`).
- A configured judge route; `sideeye doctor --verify` checks it for free.
