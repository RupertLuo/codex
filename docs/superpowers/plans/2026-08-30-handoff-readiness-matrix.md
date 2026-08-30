# Handoff: synchronization readiness matrix (2026-08-30)

| Gate | Evidence | Status |
| --- | --- | --- |
| Official latest stable target | `git ls-remote` + version-sorted tags; `rust-v0.151.0` → `78c290807ce7` | pass |
| Stable ancestry | preflight `git merge-base --is-ancestor` | pass |
| Rust manifests/lock | `cargo metadata --no-deps --locked --offline` | pass |
| Rust formatting (changed files) | targeted `rustfmt --check` | pass |
| Diff/process cleanup | preflight + `git diff --check`; no `.rej/.orig/.snap.new` | pass |
| Global-state boundary | source audit; no new mutable global singleton | pass |
| Crate compilation | `cargo check -p codex-core --lib` | blocked: uncached platform protoc archive and no disk |
| Targeted integration tests | `just test -p codex-core` | blocked before test binary starts |
| Full workspace tests | `just test` | pending targeted gate and maintainer approval |
| Full formatter | `just fmt` | blocked: missing `dotslash`/`uv` |

When capacity is restored, run the preflight first, then targeted tests for
`codex-core`, `codex-app-server`, and `codex-tui`; run complete `just test` only
after those pass and approval is available. Do not interpret the current
blocked rows as source regressions.
