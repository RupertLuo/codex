# ModelClient HTTP transport injection handoff (2026-08-30)

## Scope

Added the smallest transport-injection seam needed by the current stable
`codex-http-client` layout. `ModelClient::with_http_transport` stores a cloned
`HttpTransportHandle`; `build_api_transport` returns that handle when present,
and otherwise wraps the existing route-aware `ReqwestTransport` exactly as
before. WebSocket construction and provider routing are unchanged.

The existing focused `model_client_uses_injected_http_transport` test now has
the explicit `codex-http-client` and `http`/`bytes` imports required by this
seam. `HttpTransportHandle` has a redacted `Debug` implementation so the
`ModelClient` debug surface remains available without exposing callbacks.

## Verification

- Direct rustfmt on the three touched Rust files: completed.
- `git diff --check`: passed.
- Repository `just fmt` was attempted with the provisioned Python 3.11 path;
  Rust formatting reached unrelated pre-existing parse errors in
  `core/src/session/turn_context.rs` and `models-manager/src/manager.rs`, while
  optional `uv` and `dotslash` formatters are unavailable.
- No compile/test run was attempted here; the repository's current disk and
  unrelated syntax errors remain recorded in the parent handoff.

## Next action

When the tree is syntactically repaired and sufficient disk is available, run
the focused `codex-core` client test and `codex-http-client` tests. Only then
consider promoting this test-scoped seam into the broader ThreadManager or
production embedding constructor chain.
