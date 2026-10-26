---
name: Ask
description: Ask the expensive Side-Eye judge about THIS Claude Code session. --all = full sighted review, --turn N = recent exchanges, free text = scoped opinion, --validate = free setup check. User-invoked; spends real money except --validate and --help.
disable-model-invocation: true
allowed-tools: Bash(sideeye *), Bash("$HOME/.local/bin/sideeye" *)
---

# /sideeye:ask — ask the expensive model

This session runs on a cheap model. This command sends this session (or part of
it) to a stronger judge for an independent second opinion. The user types what
they want checked; the judge's answer comes back verbatim.

## Decide the mode (first match wins, top to bottom)

| User typed | Mode | Command to run |
|---|---|---|
| nothing, or `--help` / `help` | usage | print the Usage section below and stop (no spend) |
| `--validate` | setup check | `sideeye doctor --verify` (free) |
| `--all` | full review | `sideeye session --client claude --current` |
| anything else (free text, `--turn N`) | scoped opinion | `sideeye advise --client claude --current --question "<their text>"` |

Routing rules:
- **`--validate` wins over everything** — run `sideeye doctor --verify` alone,
  print stdout verbatim, stop.
- **`--all` wins over free text** — run the session review and mention in one
  line that the text was ignored.
- `--turn N` or `--turns N` → add `--turns N` to the advise command (default 1).
- Free text becomes `--question "<text verbatim>"`.
- Pass through `--model <model>`, `--provider <provider>`, `--no-code`,
  `--repo <path>`, `--base-url <url>` unchanged when the user typed them.

## Run it

Use a generous Bash timeout (≥ 300000 ms). Run from the session's project
directory. If `sideeye` is not on PATH, use `"$HOME/.local/bin/sideeye"`.

Then:

1. **Present the command's stdout VERBATIM**, inside a fenced code block. Do
   not summarize, soften, reorder, re-score, or "clean up" any of it. For
   `advise --json`, present the JSON; the `advice` field is the judge's answer.
2. **On a nonzero exit code:** show stderr verbatim and STOP. Do not retry with
   different flags. For `--validate`, the diagnostic is on stdout; show stdout.
3. **NEVER write the verdict or advice yourself.** Only the real Side-Eye
   stdout is the judge's answer. If the command fails or is blocked, say so
   plainly and stop.

## Usage

```
/sideeye:ask --validate               free setup and route check
/sideeye:ask --help                   this text (no spend)
/sideeye:ask <question>               the judge's opinion on recent work
/sideeye:ask --turn 3 <question>      same, over the last 3 exchanges
/sideeye:ask --all                    full sighted review of this session + diff

flags: --model <id> · --provider anthropic|openai-compatible
       · --no-code · --base-url <url>
```

## Requirements

- `sideeye` on PATH, or at `~/.local/bin/sideeye`.
- A configured judge route. `sideeye doctor --verify` checks it for free.
