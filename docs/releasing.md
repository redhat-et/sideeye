# Releasing Side-Eye

Releases produce four artifacts:

- `sideeye-<arch>.tar.gz` — Rust CLI for Linux x86_64 and macOS arm64
- `sideeye-<arch>.tar.gz.sha256` — checksums
- `sideeye-*.whl` and `sideeye-*.tar.gz` — Python package
- `redhat-et-sideeye-opencode-*.tgz` — built OpenCode plugin

## Cut a release

1. Ensure `main` is green.
2. Update versions in:
   - `pyproject.toml`
   - `.claude-plugin/plugin.json`
   - `.claude-plugin/marketplace.json`
   - `integrations/opencode/package.json`
   - Rust crate versions, if changed
3. Run `python scripts/check_versions.py` and `python scripts/check_demo.py`
   to confirm every surface and the demo page match.
4. Commit the version bump.
5. Tag and push:

```bash
git tag v0.4.0
git push redhat v0.4.0
```

The `Release` workflow builds all artifacts and publishes a GitHub Release with
generated notes. `workflow_dispatch` builds the same artifacts without
publishing, which is useful for a dry run.

## Local artifact check

```bash
cargo build --release -p sideeye-cli
python -m build
(cd integrations/opencode && npm ci && npm run build && npm pack)
```
