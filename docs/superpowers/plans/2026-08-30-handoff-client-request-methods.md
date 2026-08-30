# 2026-08-30 Client Request Method Table Handoff

- Branch: `sync/20260830-rust-v0.151.0`
- Stable target: `rust-v0.151.0` (`78c290807ce710180111df227df3b7a4fe845452`)
- Change: generated `ClientRequest::client_request_methods()` returns the complete static wire-method slice; a focused test rejects duplicate definitions.
- Purpose: provide a single source of truth for future trusted RPC-extension namespace validation without allocating or maintaining a second list.
- Validation: `rustfmt --edition 2024` and `git diff --check` passed. `just test -p codex-app-server-protocol` reached dependency resolution but was blocked before compilation by the full filesystem while fetching the pinned crossterm git revision (`No space left on device`); the failed checkout was removed.
- Safety note: this is only protocol metadata. Runtime RPC registry/dispatch remains deferred until typed and JSON `MessageProcessor` paths have shared, tested context injection.
- Next action: implement the smallest runtime-extension seam (if still needed) behind trusted host-only process-local options, then add dispatch tests before migrating registry code.
