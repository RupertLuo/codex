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
  then audit remote-compaction response-side state and re-run targeted tests
  after safely reclaiming space.

## Follow-up — remote window CAS

- Commit: `71d92288e8` (`fix(core): commit remote compaction window atomically`).
- Remote compaction now calls `prepare_auto_compact_window` before processing
  output and installs history through
  `replace_compacted_history_with_prepared_window`.
- A concurrent window change rejects the commit instead of leaving the window
  advanced while the old history remains live.
- Static validation: `rustfmt --edition 2024` and `git diff --check` passed;
  tests remain blocked by the 20 GB filesystem capacity gate.

## Follow-up — contiguous rollout commit

- Commit: `682e0d62a8` (`fix(core): persist compaction rollout as one commit`).
- Compacted history, optional full world-state baseline, and reference turn
  context are assembled into one rollout batch. The state replacement, window
  CAS, pending session-start source, and batch append now share one commit
  boundary under the session lock.
- This narrows the cold-resume divergence window; persistence failures are still
  logged by the existing rollout interface and require eventual failure-aware
  plumbing if that API is upgraded.
- Validation: rustfmt and `git diff --check` passed; no Rust test was attempted
  while the filesystem remains at ~61 MB free.

## Follow-up — remote v2 completion ordering

- The v2 attempt now returns response ID and usage metadata instead of emitting
  `RawResponseCompleted` immediately. The event is sent only after the prepared
  history CAS commit succeeds; the compaction trace checkpoint is likewise
  recorded after installation.
- v2 also uses the prepared-window CAS path, matching legacy remote compaction.
- The remaining known gap is `record_rollout_budget_usage` mutating budget state
  before the history commit. The pinned upstream transaction patch preserves
  this order because the completed provider request has already consumed the
  shared budget; changing it requires an explicit accounting decision rather
  than a mechanical transaction migration.
- Static validation passed with rustfmt and `git diff --check`; tests remain
  blocked by the filesystem capacity gate.

## Follow-up — legacy trace ordering

- Commit: `8fda3806aa` (`fix(core): trace remote compaction after commit`).
- Legacy remote compaction now records `CompactionCheckpointTracePayload` only
  after the prepared-window CAS and history/rollout commit succeeds, matching
  the v2 ordering.
- A stale window can no longer leave an `installed` trace for history that was
  never made live.

## Follow-up — local completion and usage boundary

- Commit: `ecf56172e9` (`fix(core): defer local compact usage until commit`).
- Local compact attempts now return summary and response completion metadata;
  output items are used to derive the summary but are not recorded as live
  conversation items before replacement-history CAS.
- `RawResponseCompleted`, server-reasoning, rate limits, and provider token
  state are applied only after the history commit. A dedicated token updater
  avoids calling rollout-budget accounting twice; budget is recorded once when
  the provider request completes, before history commit, matching upstream
  resource-accounting semantics.
- Static validation: rustfmt and `git diff --check` passed. Tests remain
  blocked by the filesystem capacity gate.

## Follow-up — stale prepared-window regression

- Commit: `0bef1ce1f9` (`test(core): cover stale compact commit preservation`).
- Added a session-level async test that prepares a compact window, advances the
  live window to make the prepared tuple stale, and verifies rejection leaves
  history and the concurrent live window unchanged.
- The test drains startup events first and asserts the stale primitive emits no
  event. Full orchestration completion-order coverage remains pending.
- Test execution is still blocked by the 20 GB filesystem capacity gate.

## Follow-up — append failure injection primitive

- Commit: `ad3738b225` (`test(thread-store): add append failure injection`).
- `InMemoryThreadStore` now supports a one-shot `fail_next_append` hook, with a
  unit test proving the injected error is consumed by the next non-empty append
  and does not poison later appends.
- This is intentionally isolated from production persistence semantics. The
  remaining work is to propagate rollout append errors through compaction and
  add deterministic commit-pause integration coverage.
- Tests were not executed because the filesystem remains at the capacity gate.

## Follow-up — checked compaction append

- Commit: `6c8e962fdc` (`fix(core): propagate compaction rollout append failures`).
- Prepared compaction now validates the window, appends the complete compacted
  rollout batch through a checked API, and only then mutates in-memory history,
  baseline, and window state.
- Append errors propagate to the caller instead of being logged and discarded;
  durable-first ordering keeps cold-resume replay ahead of live-state mutation.
- The existing non-prepared replacement wrapper intentionally keeps its legacy
  fire-and-forget behavior for unrelated callers.
- Static validation passed; Rust tests remain blocked by disk capacity.

## Audit notes — next deterministic tests

- The remaining high-value coverage is orchestration-level: pause between the
  prepared-window CAS and rollout append, inject an append failure, and verify
  that live history/window state and completion events remain unchanged.
- A second case should resume from the persisted rollout and verify the compact
  boundary is contiguous after a simulated process restart. These require a
  small commit-pause/test-hook seam; importing the much larger upstream runtime
  options change is intentionally deferred until after the first compile pass.
- Remote v2 audit also identified the same invariant for response completion and
  trace ordering. The production paths now emit both only after a successful
  prepared commit; budget accounting remains pre-commit because the provider
  request has already consumed the shared budget.
