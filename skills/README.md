# Harness skills

Side-Eye ships native skills for Claude Code and Codex. They call the Rust
`sideeye` binary; no Python path is used.

## Claude Code

`sideeye setup` installs `~/.claude/skills/sideeye/SKILL.md`, invoked as
`/sideeye`.

The Claude Code plugin marketplace also ships `/sideeye:ask` from
`skills/ask/SKILL.md`:

```text
/plugin marketplace add redhat-et/sideeye
/plugin install sideeye@sideeye
```

Both skills have the same modes and present the judge's stdout verbatim.

## Codex

`sideeye setup` installs `~/.agents/skills/sideeye/SKILL.md` plus
`agents/openai.yaml` with implicit invocation disabled. Invoke it with
`/skills sideeye` or `$sideeye`.

## Modes

| User intent | Command |
|---|---|
| free setup check | `sideeye doctor --verify` |
| full session review | `sideeye session --client claude\|codex --current` |
| recent second opinion | `sideeye advise --client claude\|codex --current --question "..."` |
| more history | add `--turns N` to advise |
| narrative only | add `--no-code` |

The skill must present stdout verbatim and never invent a verdict or advice.
