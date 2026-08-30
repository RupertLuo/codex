# 2026-08-30 Native Turn Extension Handoff

- Upstream change adapted: `0f68dcfcdd` (`feat(app-server): let RPC extensions start native turns`).
- `AppServerRpcContext` now carries an optional process-local native-turn gateway. Extension calls are serialized through the existing per-thread exclusive request queue and preserve the current Catalyst four-argument `TurnRequestProcessor::turn_start` API (the upstream-only form-elicitation argument was intentionally omitted).
- Added gateway delegation coverage and tracing integration coverage for an extension that starts one real native turn. Context debug output reports availability without exposing handles.
- All context construction sites use `AppServerRpcContext::new`; no global mutable state, persisted config, or cache was introduced.
- Validation: direct rustfmt and `git diff --check` pass. Targeted tests remain blocked before compilation by the full root filesystem while Cargo fetches pinned git dependencies.
- Next action: review native-turn cancellation/queue behavior against Catalyst’s existing serialization invariants, then continue the next stable patch group.
