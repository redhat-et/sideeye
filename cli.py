#!/usr/bin/env python3
"""`sideeye` CLI dispatcher — the single console entry point.

    sideeye-py review [opts]   full-session review (sighted: transcript + code diff)
    sideeye-py advise [opts]   quick second opinion on the last exchange

Both subcommands are thin: they delegate to the existing engine modules
(sideeye.escalate / sideeye.advise). This exists only so users get a real
`sideeye` command instead of `python -m sideeye.escalate`.
"""
from __future__ import annotations

import sys

USAGE = """sideeye-py — on-demand session review (legacy Python engine) (draft cheap, review expensive)

  sideeye-py review [opts]   full-session review (sighted: transcript + code diff)
  sideeye-py advise [opts]   quick second opinion on the last exchange
  sideeye-py setup            configure judge credentials and detected harnesses
  sideeye-py route           show the resolved judge route + guards (spends $0)

Run `sideeye-py review -h` or `sideeye-py advise -h` for options.
"""


def main():
    argv = sys.argv[1:]
    if not argv or argv[0] in ("-h", "--help"):
        print(USAGE)
        return
    cmd, rest = argv[0], argv[1:]
    if cmd == "route":
        # Offline diagnostic: prints the same resolution + guards a review
        # would apply. Never prints the key itself.
        from sideeye.config import describe_route
        text, ok = describe_route()
        print(text)
        sys.exit(0 if ok else 1)
    if cmd == "setup":
        from sideeye.configure import main as run_setup
        sys.exit(run_setup(rest))
    # Re-shape argv so the delegated module's argparse sees only its own args,
    # with a sensible prog name in help/usage.
    sys.argv = [f"sideeye {cmd}", *rest]
    if cmd == "review":
        from sideeye.escalate import main as run
    elif cmd == "advise":
        from sideeye.advise import main as run
    else:
        print(f"unknown command: {cmd!r}\n\n{USAGE}", file=sys.stderr)
        sys.exit(2)
    run()


if __name__ == "__main__":
    main()
