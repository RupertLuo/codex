# Handoff: native-agent lifecycle (2026-08-30)

- Adapted upstream `b62626aafa` to expose a process-local native runtime with
  status lookup and parent-owned interruption.
- Status fallback uses the existing thread-store turn API and maps persisted
  statuses to protocol `AgentStatus`; interruption remains routed through the
  parent `AgentControl`.
- Preserved Catalyst internal-session/event-sink behavior and removed obsolete
  model/skill runtime-option test fragments from the upstream patch.
- Validation: conflict-marker, formatting, and diff checks pass. Cargo tests
  remain unavailable because the filesystem filled during dependency checkout;
  the task-owned `/tmp/codex-sync-cargo` cache was removed to recover space.
- Next: inspect the resulting API with `cargo check`/targeted `just test` when
  more disk is available, then proceed to later upstream patch groups.
