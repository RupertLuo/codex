# Handoff: reusable sync preflight (2026-08-30)

- Added `scripts/upstream-sync-preflight.sh`, a read-only shell preflight for
  future stable-version sync sessions.
- It validates required command presence/version, stable-tag ancestry, diff
  cleanliness, locked offline Cargo metadata, and repository process-artifact
  cleanup. It prints disk capacity but deliberately does not mutate caches or
  configuration.
- The runbook now documents the command and its non-mutating behavior.
