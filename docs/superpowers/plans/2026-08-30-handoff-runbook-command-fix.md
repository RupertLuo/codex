# Handoff: runbook command correction (2026-08-30)

- Corrected the baseline formatting command from `just fmt --check` to
  `cargo fmt --all -- --check`.
- The repository `just` file exposes `fmt` as a recipe without a `--check`
  argument; the previous command would fail before invoking Rust formatting.
- The corrected command was already exercised in this environment. It reports
  known workspace formatting drift without modifying files.
