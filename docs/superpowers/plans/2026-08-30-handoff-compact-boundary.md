# Handoff — Compact baseline boundary (2026-08-30)

- Branch: `sync/20260830-rust-v0.151.0`
- HEAD: `2d239cedc0`
- Stable target: `rust-v0.151.0` (`78c290807ce710180111df227df3b7a4fe845452`)

Applied compact preparation commits:

- `f7080996bf`: local compact model configuration
- `9413dfd801`: sanitize images before compaction
- `9271f039f5`: route compaction through configured text model
- `6e7e300c34`: keep compact model request-local
- `1e893a805f`: atomic compact baseline transition
- `624b46d252`: restore baseline on abort

The next commit, `19ae802892`, is the first broad local-compaction transaction
commit. Its conflicts span compact orchestration, session lifecycle, turn error
paths, remote compaction, and test infrastructure. A dry-run was aborted cleanly;
it must be merged as one semantic unit before later transactionality commits.

Generated app-server schemas were intentionally retained from the stable target
after detecting a large generated-baseline mismatch. Regenerate them after the
Rust build environment has enough free space.

Validation is still pending: the filesystem has only about 67 MB free, so tests
cannot start safely. Task-specific scratch logs were removed; unrelated global
and provider caches were not touched.
