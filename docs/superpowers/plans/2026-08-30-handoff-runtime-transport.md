# Runtime transport propagation handoff (2026-08-30)

## Scope

The HTTP transport override now propagates from `ThreadManager` runtime options
through `SessionSpawnArgs` into the session-owned `ModelClient`. Existing
constructors remain compatible: the runtime option defaults to `None`, and the
route-aware Reqwest transport remains the default.

Child native-agent sessions copy the parent ModelClient's HTTP handle. The
override remains HTTP-only; websocket setup retains its provider-specific
connector, proxy, and TLS behavior.

## Validation

- `git diff --check` passes.
- Direct rustfmt was attempted on all touched Rust files. It is blocked by the
  pre-existing unclosed delimiter in `core/src/session/turn_context.rs`; no
  formatter changes were made to that unrelated file.
- Rust compilation and tests remain pending the host filesystem capacity gate.

## Risk and next action

The runtime option setter uses `Arc::get_mut` and must be called before the
manager is shared. Add a focused ThreadManager/session propagation test once a
compile-capable environment is available, then run the `codex-core` target
tests. Only after that should a public embedding API or app-server runtime
options be exposed.
