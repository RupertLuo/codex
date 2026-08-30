# 2026-08-30 Runtime Extension Injection Handoff

- Upstream change adapted: `a08b015895` (`feat(core): inject process-local runtime extensions`).
- `ThreadManagerRuntimeOptions` now carries process-local `RuntimeExtension<Config>` installers alongside Catalyst’s required host instructions and HTTP transport handle. `app-server::thread_extensions` installs them after built-in contributors, preserving built-in-first/Guardian ordering.
- Conflict resolution intentionally dropped upstream model-catalog fields/tests because this checkout already models catalog state through `Config`; no unrelated catalog API was imported.
- Added focused tests for runtime option visibility and contributor installation. No persisted config, global mutable state, or global cache was added.
- Validation: rustfmt and `git diff --check` pass. Cargo test/lint remains blocked before compilation by the exhausted root filesystem.
- Next action: review contributor ordering and runtime extension API compatibility, then migrate the next stable skills/native-agent group only after targeted app-server/core validation is possible.
