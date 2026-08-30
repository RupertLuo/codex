# Handoff — Round 0 staged migration (2026-08-30)

## Branch state

- Branch: `sync/20260830-rust-v0.151.0`
- HEAD: `b9447bd6eb`
- Stable upstream base: `rust-v0.151.0` / `78c290807ce710180111df227df3b7a4fe845452`
- Safety tags: `pre-round-0-20260830`, `pre-upstream-sync-20260830`

## Completed

Applied and committed the self-contained low-risk patches for reasoning separation,
provider error classification, image runtime test adaptation, release recursion
limit, managed PowerShell profile and Windows gating, guarded retired-agent cleanup,
worktree ignore, and PowerShell UTF-8 piping (`250516c247`, `12237ae37`).

The thread-title trigger was tried and reverted (`c57a835962`): its prerequisite
`ThreadTitleGenerator` API is not present in the stable target, so the complete
title feature must be migrated as one coherent group later. The old HTTP delta
patch was also deferred because the stable target has replaced the client transport
layer and the patch does not apply safely as-is.

## Validation and blockers

- Rust 1.95, `just`, `cargo-nextest`, `cargo-insta`, Bazelisk/Bazel 9 and Python
  3.11 are provisioned.
- `just test -p codex-thread-store` was attempted twice with isolated cargo
  outputs; both exhausted the 20GB filesystem before test execution. Generated
  targets were cleaned after each attempt. No source regression is inferred.
- `just fmt` was invoked with the task Python path; Rust formatting completed.
  Optional `dotslash`/`uv` formatters are unavailable and produced no diff.

## Exact next action

Migrate the next coherent state/skills/extensions group one commit at a time,
recording conflicts in this file and `AGENTS.md`. Retry crate tests only after a
safe disk-capacity plan is available; never delete unrelated global/provider
caches.
