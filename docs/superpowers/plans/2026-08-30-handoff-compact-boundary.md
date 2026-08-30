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

## Follow-up — response stream buffering

- HEAD: `9e4db5619f` (`fix(core): buffer compact output until response completion`).
- `drain_to_completed` now buffers `OutputItemDone`, server-reasoning, and
  rate-limit events and applies them only after `response.completed`.
- This prevents a failed or interrupted compact retry from adding partial model
  output or response-side state to the live history used by the next retry.
- `RawResponseCompleted` and token usage are still emitted at stream completion;
  auditing their rollback/commit semantics is the next transaction step.
- `git diff --check` passed and formatting had already completed. No crate test
  was run because the 20 GB filesystem has only ~62 MB free; prior isolated
  `just test -p codex-thread-store` attempts exhausted it before test execution.
- Next: couple completion/accounting with prepared-history and rollout commits,
  then migrate remote-compaction transaction paths and re-run targeted tests
  after safely reclaiming space.
