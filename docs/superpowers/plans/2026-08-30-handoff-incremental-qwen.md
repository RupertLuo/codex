# Handoff — Incremental and Qwen migration (2026-08-30)

## Branch state

- Branch: `sync/20260830-rust-v0.151.0`
- HEAD: `99b85c83c9`
- Stable base: `rust-v0.151.0` / `78c290807ce710180111df227df3b7a4fe845452`

## Completed

- Incremental HTTP batches A/B/C are applied in order. They add HTTP delta
  requests, cross-turn/session baseline ownership, model capability gates,
  body-size history estimation, expired-id fallback, and atomic baseline moves.
- Qwen functional changes are applied: tool-output image relocation, interrupt
  baseline preservation, and HTTP wire-shape integration coverage.
- Catalyst-specific access-program/routing fields were preserved while resolving
  client conflicts; formatting left no uncommitted Rust diff.

## Deferred

The target stable branch already contains newer skills/provider APIs, so older
Catalyst skill-handle commits must be ported semantically. Thread-title generation
also remains deferred as a complete feature group because its prerequisite API is
not in the stable target.

## Validation

Targeted tests remain blocked before execution by the 20GB filesystem capacity
gate. Toolchain and cache setup are documented in `AGENTS.md`; no unrelated
global/provider cache was inspected or deleted.

## Next action

Compare the remaining skills/extensions/native-agent and compaction groups against
the stable APIs, migrate only missing behavior, then free/allocate safe build
space and run crate-level tests before any final merge or cleanup.
