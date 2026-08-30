# Handoff: preflight tool checks (2026-08-30)

- Extended `scripts/upstream-sync-preflight.sh` to require
  `cargo-nextest`/`cargo-insta` and validate Python >= 3.10.
- The script automatically prefers the task-scoped `/tmp/codex-python` Python
  when present, or accepts `SYNC_PYTHON_BIN`; it no longer accidentally treats
  a host Python 3.6 as usable.
- `dotslash` and `uv` are reported as optional formatter prerequisites rather
  than installed or accessed implicitly.
