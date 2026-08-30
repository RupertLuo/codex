# 2026-08-30 RPC Dispatch Boundary Handoff

- Current HEAD: `b5ae347131` on `sync/20260830-rust-v0.151.0`.
- Audit result: `MessageProcessor` has separate typed and JSON native paths. Typed in-process messages can only represent `ClientRequest`; custom extension methods therefore require an explicit raw JSON request API if they must be supported in-process.
- Safe seam: keep the registry process-local and injected through startup/in-process overrides. Do not add it to persisted config, global mutable state, or global caches.
- JSON dispatch order: inspect the raw method before native deserialization; reject uninitialized registries, dispatch registered methods with a transport/session context, and return method-not-found for unknown methods inside a registered namespace.
- Required tests before registry migration: registry validation (nonempty namespace, native collision, duplicates), JSON success/error/not-initialized/unknown-method integration cases, and raw in-process coverage if that API is added. Transport context mapping must be tested separately.
- Next action: design and implement the smallest registry + JSON/raw in-process dispatch seam together; do not cherry-pick the larger upstream dispatch commit without adapting these two current entry points.
