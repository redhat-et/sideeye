---
name: sideeye
description: Ask the expensive Side-Eye judge about this Claude Code session. --all = full sighted review, --turn N = recent exchanges, free text = scoped opinion, --validate = free setup check. User-invoked only.
disable-model-invocation: true
allowed-tools: Bash(sideeye *), Bash("$HOME/.local/bin/sideeye" *)
---

# /sideeye — ask the expensive model

This session runs on a cheap model. This command sends this session (or part of
it) to a stronger judge for an independent second opinion. The judge's answer
comes back verbatim.

## Modes

| User typed | Command |
|---|---|
| nothing or `--help` | print the usage below; no spend |
| `--validate` | `sideeye doctor --verify` (free) |
| `--all` | `sideeye session --client claude --current` |
| anything else | `sideeye advise --client claude --current --question "<text>"` |

- `--turn N` / `--turns N` adds `--turns N` to advise.
- Pass through `--model`, `--provider`, `--no-code`, `--repo`, and `--base-url`
  when the user typed them.
- Run with a generous timeout (≥300000 ms) from the session project directory.
- Present stdout verbatim. On failure, show stderr and stop.
- Never write a verdict or advice yourself.

## Usage

```
/sideeye --validate            free route check
/sideeye --all                 full session review + diff
/sideeye is this change sound? second opinion on recent work
/sideeye --turn 3 <question>   second opinion over 3 exchanges
```

## Requirements

- `sideeye` on PATH (or use `$HOME/.local/bin/sideeye`).
- A configured judge route; `sideeye doctor --verify` checks it for free.
