# Handoff: native fork integration coverage (2026-08-30)

- Added `fork_thread_last_n_turns_keeps_recent_turn` to the existing
  `core/tests/suite/fork_thread.rs` integration suite.
- The test drives two completed turns through `test_codex`, forks with
  `ForkSnapshot::LastNTurns(1)`, and asserts the persisted fork contains the
  recent turn while excluding the older one.
- This closes the testing-guidance gap identified in the breaking-change
  review: native agent/fork behavior now has an integration-level history
  assertion in addition to thread-manager unit coverage.
- Formatting and diff checks pass. Execution is pending disk recovery and the
  missing platform-specific Cargo archive.
