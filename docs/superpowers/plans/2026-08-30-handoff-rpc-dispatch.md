# 2026-08-30 RPC Dispatch Handoff

- Upstream change adapted: `a02268cc6c` (`feat(app-server): inject process RPC extensions`).
- Catalyst resolution: preserved the existing request-serialization queue construction and runtime transport options while adding the upstream process override API, raw JSON extension dispatch, raw JSON responses, and transport-context mapping.
- Both production JSON requests and tracing-harness raw requests now exercise extension success, not-initialized rejection, and namespace method-not-found behavior. Typed in-process requests remain native-only; the public in-process extension list is registered at startup for a future raw request API.
- Registry remains process-local and is created once per app-server runtime. No persisted config, global mutable state, or global cache is used.
- Validation: rustfmt and `git diff --check` pass. Targeted Cargo tests are still blocked before compilation by the exhausted root filesystem during pinned git dependency fetch; no test binary ran.
- Next action: inspect the new process override API for public compatibility, add raw in-process request parity (or document JSON-only support), then run targeted app-server tests once disk is provisioned.
