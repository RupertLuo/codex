# Handoff: native-agent extension factory (2026-08-30)

- Branch: `sync/20260830-rust-v0.151.0`
- Upstream target: `rust-v0.151.0` (`78c290807ce710180111df227df3b7a4fe845452`)
- Adapted commit: `30fc042037` (`feat(extensions): inject native agent spawner factories`)
- Result: `ThreadManagerRuntimeOptions` carries process-local native-agent extension factories; app-server installs them after existing runtime extensions using the native spawner. Core re-exports the factory trait and spawner alias.
- Conflict policy: omitted upstream `model_catalog` and `skill_provider_sources` changes because those fields are not part of this Catalyst baseline; retained required instructions, HTTP transport, queue, and existing extension behavior.
- Tests: retained the runtime-options factory retention test and app-server extension-factory probe coverage.
- Validation: conflict-marker scan/source inspection completed. Formatting and crate tests remain blocked while the filesystem is full; Cargo previously failed checking out pinned crossterm with `No space left on device`.
- Next: adapt `92728d97dc`, `a92f3b7e61`, `8167a15f90`, and `b62626aafa` in order. Preserve current `SpawnAgentOptions`, `spawn_agent_with_metadata`, fork snapshot variants, and extension init data.
