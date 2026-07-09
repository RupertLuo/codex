# Task 5 report: native automatic-compaction enable switch

## Result

Implemented `model_auto_compact_enabled` as an optional TOML/app-server field and a resolved Core boolean that defaults to `true`.

- Enabled behavior keeps the configured proactive threshold clamped to 90% of the resolved model context window.
- Disabled behavior removes the proactive `auto_compact_scope_limit` for both `Total` and `BodyAfterPrefix`.
- The independent full-context safety boundary remains active for both scopes, including `tokens_until_compaction` reporting.
- `config/read` exposes the field and `config/batchWrite` persists and reloads it.
- Generated Core config schema and app-server JSON/TypeScript fixtures were refreshed with repository generators.

## RED

Production code was unchanged for these runs.

1. `cargo nextest run --no-fail-fast -p codex-core model_auto_compact_enabled`
   - 3 tests run, 0 passed, 3 failed.
   - Expected failures: resolved config lacked the default/explicit field and the config schema lacked the property.
2. `cargo nextest run --no-fail-fast -p codex-app-server config_read_serializes_model_auto_compact_enabled`
   - 1 test run, 0 passed, 1 failed.
   - Expected failure: `/result/config/model_auto_compact_enabled` was absent.
3. `cargo nextest run --no-fail-fast -p codex-core auto_compact_disabled_uses_full_context_as_safety_boundary`
   - Expected compile-time RED: `E0609`, no field `model_auto_compact_enabled` on Core `Config`.

The first attempts through `just test` were tooling-blocked because `just` and then `cargo-nextest` were not installed. Both were installed. On this Windows host, `just` recipes still failed to locate `cargo` with exit 9009, so subsequent test commands used the recipe-equivalent `cargo nextest` invocation with `RUST_MIN_STACK=8388608` where required.

## GREEN and verification

- Final focused Core verification:
  - `model_auto_compact`: 3/3 passed.
  - `auto_compact_enabled_by_default_reaches_proactive_threshold`: 1/1 passed.
  - `auto_compact_disabled_uses_full_context_as_safety_boundary`: 1/1 passed.
  - `config_schema_matches_fixture`: 1/1 passed.
  - `auto_compact_clamps_config_limit_to_context_window`: 1/1 passed.
  - `auto_compact_body_after_prefix_still_caps_at_context_window`: 1/1 passed.
- `cargo nextest run --no-fail-fast -p codex-app-server config_read_and_batch_write_round_trip_model_auto_compact_enabled`: 1/1 passed.
- `cargo nextest run --no-fail-fast -p codex-app-server-protocol`: 252/252 passed.
- Final app-server schema fixture verification: 4/4 passed.
- Core config-matching suite: 433/433 passed.
- Core compact-matching suite: 145/149 passed; see concerns below.
- Scoped `cargo fmt ... -- --check`: passed.
- `git diff --check`: passed.

Generators used:

- `cargo run -p codex-core --bin codex-write-config-schema`
- `cargo run -p codex-app-server-protocol --bin write_schema_fixtures --`

## Commit

Feature commit: `b7b541398e47cba3d7bbd54abb313c96d0624085`

## Concerns

1. The four failing compact-matching tests are hook-execution tests:
   - `manual_pre_compact_block_decision_does_not_block_compaction`
   - `compact_hooks_respect_matchers_and_post_runs_after_compaction`
   - `token_budget_compaction_runs_compact_hooks`
   - `remote_compaction_parity_manual_hooks`

   All invoke `python3` hook scripts. This host only has the Windows Microsoft Store `python3.exe` alias and no real Python interpreter, so scripts do not run and expected hook log files are absent. The remaining 145 compact-matching tests passed, including the focused proactive clamp and full-context tests.
2. Scoped Clippy is blocked by a pre-existing denied lint in `app-server-protocol/src/protocol/common.rs:197` (`clippy::expect_used`). The changed files did not introduce that diagnostic.
3. Rustfmt prints existing stable-toolchain warnings for the nightly-only `imports_granularity` setting, but formatting and `--check` both exit successfully.
