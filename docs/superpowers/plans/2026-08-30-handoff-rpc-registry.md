# 2026-08-30 RPC Registry Handoff

- Added a process-local `AppServerRpcExtension` contract and `AppServerRpcRegistry` validation seam.
- Registry rejects malformed one-level namespaced methods, duplicate extension methods, and collisions with generated native `ClientRequest` methods.
- The registry is intentionally not wired into `MessageProcessor` yet. Dispatch must adapt both current JSON and typed in-process entry points; typed requests cannot represent unknown extension methods.
- No global mutable registry, persisted config field, or cache was introduced.
- Validation so far: direct rustfmt and `git diff --check`. Cargo tests remain unavailable before compilation because the root filesystem is full during pinned git dependency fetches.
- Next action: inject one `Arc` registry through process startup/in-process overrides and implement JSON plus explicit raw in-process dispatch with transport-context tests.
