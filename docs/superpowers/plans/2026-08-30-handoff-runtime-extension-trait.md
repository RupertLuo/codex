# 2026-08-30 Runtime Extension Trait Handoff

- Added `codex_extension_api::RuntimeExtension<C>` as a documented, `Debug + Send + Sync` process-local installer trait.
- The trait only receives an `ExtensionRegistryBuilder`; it has no configuration persistence or global mutable-state access. Hosts are expected to install it once per runtime registry.
- This is the prerequisite for the upstream process-local contributor injection patch; no contributor ordering or production wiring changed yet.
- Validation: direct rustfmt and `git diff --check` passed. Cargo build/lint remains blocked by disk exhaustion before dependency resolution.
- Next action: adapt upstream runtime-extension injection into `ThreadManagerRuntimeOptions` and `extensions.rs`, preserving Catalyst’s existing transport/required-instructions fields and contributor ordering.
