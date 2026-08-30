# Handoff: sanitized global cache inventory (2026-08-30)

Read-only inventory (no config contents, credentials, proxy values, or provider
cache contents were inspected):

- Rust/Cargo: Rust and Cargo `1.95.0`; `~/.cargo/config*` absent; registry is
  approximately `1.6G`; git cache approximately `134M`.
- Build caches: `~/.cache/bazel` approximately `187M`; `~/.cache/bazelisk`
  approximately `63M`.
- Task Python: `/tmp/codex-python` contains Python `3.11.9` (about `102M`),
  while the host default `python3` is `3.6.8` and must not be used for scripts.
- Installed helpers: `just 1.58.0`, `cargo-nextest`, and `cargo-insta` are
  present. `dotslash` and `uv` are still unavailable.
- Sensitive cache category names were not enumerated or opened; they remain
  out of scope for cleanup.
- Root filesystem has only about `5M` free, so no build/test was attempted in
  this round. Use a task-scoped cache/output root after capacity is restored.
