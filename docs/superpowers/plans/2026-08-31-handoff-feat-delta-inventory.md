# Catalyst delta inventory — 2026-08-31

This is a static inventory of the Catalyst-authored commits reachable from
`feat/yanjiang` but not as direct ancestors of `sync/20260830-rust-v0.151.0`.
The sync branch intentionally carries equivalent changes under new commits
where the stable 0.151.0 APIs required a semantic port rather than a cherry-pick.

## Already carried or superseded

- Qwen/incremental request batches, compact-model routing, provider error
  classification, PowerShell encoding, runtime transport injection, process-local
  RPC extensions, native-agent lifecycle/forking, and test backfills are present
  on the sync branch.
- Several small upstream-facing changes are already in `rust-v0.151.0`; they
  were recorded as no-ops and must not be replayed.
- The obsolete agent-job cleanup change was explicitly removed in
  `0088a8aa98`, because stable 0.151.0 drops those tables and state types.

## Deferred or still requiring a coherent port

- Thread-store title generation (`d59b47d723`, `4d7ad8ce77`, `18ef0969c4`):
  do not port this old 325-line store-level implementation. Stable 0.151.0
  already contains the current TUI `app/thread_title.rs` flow, while the
  thread-store title-generator API is absent; compare behavior and add only a
  narrowly justified compatibility fix if a regression is demonstrated.
- Skills/provider commits (`5793b0dad6`, `eaf21f5a50`, `6b61d98fe2`,
  `dbbd0d59ec`, `d631fdd9fa`) require no additional port at this target:
  `git diff rust-v0.151.0..HEAD -- codex-rs/ext/skills codex-rs/codex-mcp`
  is empty, and the stable tree already contains the newer
  `SkillProviderSource`, budgeted-tool, authorization, and generation-binding
  implementation. Do not cherry-pick the older commits; only revisit if a
  concrete regression appears.
- Legacy CRLF checksum repair (`e85f2c1dbe`): revisit with the state migration
  group, not as an isolated checksum edit.
- App-server reusable serve CLI (`a2f8befc6d`) remains deferred: its old
  `cli.rs` extraction targets a different process/transport layout and should
  be re-designed against the current binary before porting. The native client
  method list from `b0b17bf811` is already present in
  `app-server-protocol/src/protocol/common.rs`; do not replay it.
- TUI model/credential onboarding commits: compare against the stable TUI
  runtime first; preserve stable provider abstractions and add snapshots only
  for behavior that is actually retained.
- The remaining compaction transactionality work needs failure-order tests and
  rollout/history verification; the current CAS and buffering boundaries are
  partial safeguards, not final proof.

## Validation gate

`cargo check -p codex-state --lib` passes with one job and debuginfo disabled.
The broader `codex-thread-store` test build and
`cargo check -p codex-app-server-protocol --lib` are killed by the host's 2 GiB
memory limit. Continue static ports and small checks here; run full targeted
tests on a larger machine or CI worker.
