# Handoff: metadata and native-agent API audit (2026-08-30)

- `cargo metadata --no-deps --format-version 1` succeeds for all 143 workspace
  packages without network access or lockfile changes.
- Static call-site audit found all explicit `SpawnAgentOptions` literals either
  initialize `thread_extension_init` or use `..Default::default()`; native
  request/status/interrupt symbols have expected app-server/core consumers.
- `CARGO_NET_OFFLINE=true cargo check -p codex-core --lib` reaches dependency
  resolution but fails because the platform-specific
  `protoc-bin-vendored-linux-ppcle_64` archive is not cached. This confirms the
  remaining validation blocker is dependency capacity/cache completeness, not
  a metadata parse failure.
- Root filesystem remains effectively full; no build artifacts or global
  sensitive caches were removed.
