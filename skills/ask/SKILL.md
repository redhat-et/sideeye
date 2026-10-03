---
name: Ask
description: Ask the expensive Side-Eye judge about THIS session. --all = full sighted review, --turn N = recent exchanges, free text = scoped opinion, --validate = free setup check. User-invoked; spends real money except --validate and --help.
disable-model-invocation: true
allowed-tools: Bash(sideeye review *), Bash(sideeye advise *), Bash(sideeye route), Bash("$HOME/.local/bin/sideeye" review *), Bash("$HOME/.local/bin/sideeye" advise *), Bash("$HOME/.local/bin/sideeye" route), Bash(command -v sideeye*), Bash(uv tool install *), Bash(pipx install *), Bash(python3 -m pip install *)
---

# /sideeye:ask — ask the expensive model

This session runs on a cheap model. This command sends this session (or part of
it) to a real Claude judge for an independent second opinion. The user types what
they want checked; the judge's answer comes back verbatim.

## Decide the mode (first match wins, top to bottom)

| User typed | Mode | Command to run |
|---|---|---|
| nothing, or `--help` / `help` | usage | print the Usage section below and stop (no spend) |
| `--validate` (with or without other text) | setup check | `sideeye route` (free) |
| `--all` | full review | `sideeye review --current --yes` |
| anything else (free text, `--turn N`) | scoped opinion | `sideeye advise --current --yes --question "<their text>"` |

Routing rules:
- **`--validate` wins over everything** — it is the free diagnostic; run
  `sideeye route` alone, print its stdout verbatim, stop.
- **`--all` wins over free text** — if both appear, run the review and mention
  in one line that the text was ignored.
- `--turn N` or `--turns N` → add `--turns N` to the advise command (default 1).
- Free text becomes `--question "<text verbatim>"`. It may be empty
  (e.g. `/sideeye:ask --turn 2` with no text is a valid scoped opinion).
- Pass through these flags **unchanged** in whichever mode applies:
  `--model <fable|opus|sonnet|haiku|full-id>`, `--no-code`, `--max-cost N`.
  Do not invent flags the user didn't type.

## Engine (one-time, first use only)

The judge runs on the `sideeye` engine — usually already on PATH. Check:

```
command -v sideeye
```

If it prints a path, skip to "Run it". If it prints nothing, install it once
(~30s; the first available tool wins):

```
uv tool install git+https://github.com/redhat-et/sideeye || \
  pipx install git+https://github.com/redhat-et/sideeye || \
  python3 -m pip install --user git+https://github.com/redhat-et/sideeye
```

Two things can go wrong:
- Install succeeded but `command -v sideeye` is still empty (a fresh
  `~/.local/bin` is not on this shell's PATH) → run the command as
  `"$HOME/.local/bin/sideeye" <same subcommand and flags>` instead.
- The install line failed (none of uv / pipx / pip is available) → tell the user
  to install one (`brew install uv`) and stop.

## Run it

Run the command from the mode table with a **generous Bash timeout
(≥ 300000 ms)** — a full-session judge call can take a few minutes.

Then:

1. **Present the command's stdout VERBATIM**, inside a fenced code block. Do not
   summarize, soften, reorder, re-score, or "clean up" any of it. That output is
   the judge's verdict — the user must see it exactly as written. After the
   verbatim block, you may add one short line offering to address the issues it
   lists.
2. **On a nonzero exit code:** show the command's error output verbatim and
   **STOP**. Do not retry with different flags or arguments. Exception: in
   `--validate` mode the diagnostic report is on **stdout** (exit 1 just means
   "not yet ready") — show stdout there, not stderr.
3. **NEVER write the verdict yourself.** This is the whole point of the tool: a
   verdict invented here would be produced by the *cheap* model this session
   runs on — the exact plausible-but-fake output Side-Eye exists to catch. Only
   the real stdout of `sideeye review` / `sideeye advise` / `sideeye route` is a
   verdict. If the command fails, returns nothing, or is blocked, say so plainly
   and stop — do not fabricate scores, issues, or a summary under any
   circumstances.

## Usage (print when the user typed --help or nothing)

```
/sideeye:ask — ask the expensive model (the Side-Eye judge) about this session

  /sideeye:ask --validate               free setup check: is the judge route
                                        configured and unblocked? (free)
  /sideeye:ask --help                    this text (no spend)
  /sideeye:ask <question>               the judge's opinion on your question,
                                        scoped to recent work (~2¢–$1)
  /sideeye:ask --turn 3 <question>      same, over the last 3 exchanges
  /sideeye:ask --all                    full sighted review of the whole session
                                        + code diff (median ~75¢, up to ~$3.50)

  extra flags: --model fable|opus|sonnet|haiku · --no-code (skip the diff)
               · --max-cost N (abort above this ceiling)
```

## Notes
- The judge must hit a real Claude route — never this session's own (cheap)
  model. A "no judge route" or BLOCKED error means the route isn't configured:
  surface the message verbatim, don't work around it. `--validate` (free) shows
  exactly what resolves and why.
- If the estimate exceeds the cost ceiling, the command aborts with a message
  telling the user how to raise `--max-cost`. Show it; don't bypass it.
- The authoritative flag lists are always `sideeye review --help` and
  `sideeye advise --help`.
