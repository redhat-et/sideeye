"""Check that every package surface reports the same version.

Usage:
    python scripts/check_versions.py            # compare all surfaces
    python scripts/check_versions.py v0.4.0     # also require this release tag
"""
from __future__ import annotations

import json
import re
import sys
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


def read_json(path: str) -> dict:
    return json.loads((ROOT / path).read_text(encoding="utf-8"))


def read_toml(path: str) -> dict:
    with (ROOT / path).open("rb") as handle:
        return tomllib.load(handle)


def main() -> int:
    expected = read_toml("pyproject.toml")["project"]["version"]
    tag = sys.argv[1] if len(sys.argv) > 1 else None
    if tag:
        if not tag.startswith("v"):
            print(f"release tag must start with 'v', got {tag!r}", file=sys.stderr)
            return 1
        expected = tag[1:]

    found = {
        "pyproject.toml": expected,
        ".claude-plugin/plugin.json": read_json(".claude-plugin/plugin.json")["version"],
        "integrations/opencode/package.json": read_json(
            "integrations/opencode/package.json"
        )["version"],
    }
    marketplace = read_json(".claude-plugin/marketplace.json")
    found[".claude-plugin/marketplace.json"] = next(
        (plugin["version"] for plugin in marketplace.get("plugins", []) if plugin["name"] == "sideeye"),
        "<missing>",
    )
    for cargo in sorted(ROOT.glob("crates/*/Cargo.toml")):
        found[str(cargo.relative_to(ROOT))] = read_toml(
            str(cargo.relative_to(ROOT))
        )["package"]["version"]

    mismatches = {path: version for path, version in found.items() if version != expected}
    if not re.fullmatch(r"\d+\.\d+\.\d+", expected):
        print(f"invalid version {expected!r}; expected MAJOR.MINOR.PATCH", file=sys.stderr)
        return 1
    if mismatches:
        print(f"version mismatch; expected {expected}", file=sys.stderr)
        for path, version in mismatches.items():
            print(f"  {path}: {version}", file=sys.stderr)
        return 1
    print(f"versions aligned at {expected}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
