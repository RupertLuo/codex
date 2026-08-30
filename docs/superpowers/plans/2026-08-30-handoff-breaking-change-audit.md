# Handoff: native-agent breaking-change audit (2026-08-30)

Reviewed the native-agent migration against app-server APIs, CLI parameters,
configuration loading, raw response events, and rollout resume behavior.

- No app-server protocol method or payload changed.
- No CLI argument, persisted config field, raw response event, or rollout file
  shape changed.
- Runtime handles and factories remain process-local and are not serialized.
- Existing `ThreadManager::spawn_subagent` callers retain the old signature via
  a compatibility wrapper.
- `ForkSnapshot::LastNTurns` is an additive public enum variant and can require
  downstream exhaustive matches to add a branch. This matches the upstream
  stable API and is the only identified source-level compatibility concern.
- Restored the warning FIFO test's accurate name after conflict resolution;
  its behavior and assertions were already intact.

No external wire/schema regeneration is required for this patch group.
