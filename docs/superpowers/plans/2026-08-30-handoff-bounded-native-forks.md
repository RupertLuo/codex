# Handoff: bounded native-agent forks (2026-08-30)

- Adapted upstream `92728d97dc` after the native-agent factory seam.
- Added `NativeAgentSpawnRequest`, `ForkSnapshot::LastNTurns`, and bounded
  history truncation through the existing `thread_rollout_truncation` helper.
- Added an app-server native spawner that upgrades the process-local
  `ThreadManager` and calls `spawn_subagent_with_snapshot`.
- Preserved Catalyst's existing internal-session spawner and interruption
  metadata (`active_turn_started_at`); the upstream patch's older call shape
  was not copied verbatim.
- Formatting and conflict-marker checks pass. Cargo tests remain blocked by
  the full filesystem during pinned git dependency checkout.
- Next commit: adapt `a92f3b7e61` to current `AgentControl::spawn_agent_with_metadata`.
