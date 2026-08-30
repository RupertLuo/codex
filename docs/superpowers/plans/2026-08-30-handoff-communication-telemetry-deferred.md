# Handoff: multi-agent communication telemetry deferred (2026-08-30)

- Reviewed upstream `129ea2aaf5` (332 additions/107 deletions across 15
  files).
- It changes `AgentControl` input signatures, introduces communication context
  plumbing, and rewrites spawn/execution paths. Catalyst currently has custom
  multi-agent v2 hints, parent/root turn IDs, and spawn metadata, so a wholesale
  cherry-pick would be unsafe and exceed the change-size guidance.
- Defer into separate stages: communication context type, send/result logging,
  spawn propagation, then integration tests. Preserve current `Op` and
  `SpawnAgentOptions` contracts while adapting.
