# Handoff: reasoning-max patch deferred (2026-08-30)

- Reviewed upstream `80f54d1266` but did not merge it.
- The Catalyst branch already has custom Ultra fallback logic, persistent
  reasoning handling, and corresponding tests/snapshots. The upstream patch
  conflicts across protocol, core client, Bedrock catalog, and TUI snapshots.
- The trial was reset before commit; no source or index changes remain.
- Revisit as an isolated reasoning migration after native-agent validation,
  preserving Catalyst's fallback semantics and updating snapshots deliberately.
