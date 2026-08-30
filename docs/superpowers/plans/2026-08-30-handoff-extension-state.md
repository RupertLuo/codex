# Handoff: native-agent extension state (2026-08-30)

- Adapted upstream `8167a15f90` to carry `ExtensionDataInit` through the
  current `ThreadManagerState`/`ThreadSpawnRequest` path and parent agent
  control.
- Existing Catalyst fields on `SpawnAgentOptions` and multi-agent v2 hint
  handling were preserved; ordinary tool spawns initialize extension data with
  an explicit default.
- Added propagation coverage to the native-agent test. Formatting and marker
  checks pass; compilation remains blocked by exhausted filesystem/Cargo fetch.
- Next: inspect and adapt lifecycle commit `b62626aafa`, then run a source-level
  API audit before attempting tests again.
