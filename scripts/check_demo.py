"""Guard the demo page against drift from the current product.

The page is user-facing and easy to forget. This check fails CI when it stops
matching the current version, commands, or engine claims.
"""
from __future__ import annotations

import sys
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
PAGE = ROOT / "docs" / "sideeye-demo.html"


def main() -> int:
    version = tomllib.loads((ROOT / "pyproject.toml").read_text())["project"]["version"]
    page = PAGE.read_text(encoding="utf-8")
    problems = []

    def require(condition: bool, message: str) -> None:
        if not condition:
            problems.append(message)

    require(f"v{version}" in page, f"demo page does not mention the current version v{version}")
    require(f"releases/tag/v{version}" in page, f"release link is not v{version}")
    require("/sideeye" in page, "demo page does not show the /sideeye command")
    require("--confirm" in page, "demo page does not show estimate/confirm")
    require("rust" in page.lower(), "demo page does not mention the Rust engine")
    require("opencode" in page.lower(), "demo page does not show the OpenCode integration")
    require("self-grading" in page.lower(), "demo page does not show the self-grading guard")

    for stale in ("/escalate-last", "/escalate-all", "adapter v2 · rubric v3"):
        require(stale not in page, f"demo page still contains stale text: {stale}")

    if problems:
        print("demo page drift detected:", file=sys.stderr)
        for problem in problems:
            print(f"  - {problem}", file=sys.stderr)
        return 1
    print(f"demo page matches Side-Eye {version}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
