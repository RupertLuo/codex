# Handoff — Round 1 Complete

## Branch State
- Branch: `feat/yanjiang`
- HEAD: (see git log after commit)
- Fork base (upstream): `ccdfb4f342a` (2026-06-28)
- Rounds completed: 0 (planning) + 1 (test backfill + baseline)

## What Was Done

### Round 0 — Planning
- Created `feat/yanjiang` branch from current `main`
- Analyzed all 111 fork patches, classified into 13 logical groups (A-M, Z)
- Identified 3 critical test coverage gaps via deep analysis
- Wrote sync plan: `docs/superpowers/plans/2026-08-30-upstream-sync-plan.md`

### Round 1 — Test Backfill + Baseline

#### 1. Bug Fix: codex-api compilation errors
Fork added `previous_response_id` to `ResponsesApiRequest` and 3 new fields to `ModelInfo`
but did not update test initializers.

**Files fixed:**
- `codex-rs/codex-api/tests/clients.rs` — added `previous_response_id: None` to 3 sites
- `codex-rs/codex-api/tests/models_integration.rs` — added 3 missing `ModelInfo` fields

#### 2. New Tests: thread-store (10 tests)
**File: `codex-rs/thread-store/src/live_thread.rs`** — 8 tests for `sanitize_title`:
- `sanitize_title_passthrough`
- `sanitize_title_strips_double_quotes`
- `sanitize_title_strips_cjk_quotes`
- `sanitize_title_strips_trailing_punctuation`
- `sanitize_title_first_nonempty_line`
- `sanitize_title_truncates_at_30_chars`
- `sanitize_title_empty_input`
- `sanitize_title_combined`

**File: `codex-rs/thread-store/src/in_memory.rs`** — 2 tests:
- `title_generator_round_trip`
- `failed_append_may_become_durable_returns_false`

#### 3. New Tests: realtime_conversation state machine (9 tests)
**File: `codex-rs/core/src/realtime_conversation.rs`** — added `#[cfg(test)]` helper:
- `TestActiveConversation` struct + `set_active_for_test()` on `RealtimeConversationManager`

**File: `codex-rs/core/src/realtime_conversation_tests.rs`** — 9 tests:
- `claim_close_from_active_transitions_to_closing`
- `claim_close_from_none_returns_none`
- `claim_close_while_closing_in_progress_returns_none`
- `complete_close_clears_state`
- `complete_close_wrong_token_is_noop`
- `release_close_for_retry_allows_reclaim`
- `claim_close_expected_target_rejects_mismatch`
- `quarantine_policy_set_for_persistence_quarantine`
- `running_state_returns_none_when_closing`

#### 4. New Tests: state/extract.rs (3 tests)
**File: `codex-rs/state/src/extract.rs`** — 3 tests:
- `transaction_with_token_count_updates_metadata`
- `compacted_without_checkpoint_does_not_affect_metadata`
- `compacted_with_reference_context_sets_model`

---

## Test Baseline (pre-rebase)

Results from running all fork-affected crate tests on this machine:

| Crate | Tests | Result | Notes |
|-------|-------|--------|-------|
| `codex-thread-store` | 113 | **ALL PASS** | +10 new tests |
| `codex-state` | 167 | **ALL PASS** | +3 new tests |
| `codex-protocol` | 246 | **ALL PASS** | |
| `codex-client` | 42 | **ALL PASS** | |
| `codex-api` | 131 | **ALL PASS** | +3 bug fixes |
| `codex-rollout` | 89 | **ALL PASS** | |
| `codex-app-server-protocol` | 253 | **ALL PASS** | |
| `codex-config` | 200 | **ALL PASS** | |
| `codex-mcp` | 107 | **ALL PASS** | |
| `codex-app-server` | 954 | **948 pass, 5 fail, 1 timeout** | Pre-existing failures (see below) |
| `codex-core` | — | **BUILD BLOCKED** | V8 crate download timeout (network) |
| `codex-tui` | — | **BUILD BLOCKED** | Same V8 issue (transitive dep) |
| `codex-skills-extension` | — | **BUILD TIMEOUT** | Likely V8-related |

### Pre-existing app-server failures (NOT caused by fork patches)
These 5 failures + 1 timeout exist in the upstream code:
- `executor_mcp::selected_executor_plugin_exposes_its_mcps_only_to_that_thread`
- `mcp_resource::orchestrator_skill_can_read_referenced_resource_without_an_executor`
- `selected_capability_stack::selected_capabilities_become_available_between_samples_in_one_turn`
- `selected_capability_stack::selected_capability_stack_tracks_environment_availability_and_resume`
- `web_search::standalone_web_search_round_trips_output`
- `external_agent_config::import_plugins_infers_external_official_marketplace_when_missing_from_settings` (timeout)

### V8 Download Blocker
`codex-core` depends on `codex-code-mode` which depends on the `v8` crate. The V8 build
script downloads a ~50MB binary from GitHub, which times out on this machine's network.
**On the new machine with better network, this will resolve automatically.**

To verify: `just test -p codex-core` must run the realtime_conversation tests.

---

## Next: Round 2 (Fetch + Rebase Low-Risk Groups)

### Prerequisites
1. **Fetch upstream**: `git fetch upstream main` (requires good network)
2. **Run codex-core tests**: `just test -p codex-core` to verify realtime tests pass
3. **Run codex-tui tests**: `just test -p codex-tui` to capture TUI baseline

### Round 2 Scope
Rebase low-risk groups G/H/J/L/M/Z (~27 commits) onto upstream/main.
See `2026-08-30-upstream-sync-plan.md` for full details.

### Rebase Strategy
```bash
# Tag current state for safety
git tag pre-round-2

# Interactive rebase onto upstream
git rebase -i upstream/main
```

Pick order for Round 2 (low-risk first):
- G: Platform fixes (PowerShell UTF-8, CRLF migration)
- H: Thread title generation
- J: Image runtime extensions
- L: Misc fixes (provider error, recursion limit)
- M: Reasoning effort decoupling
- Z: Housekeeping (tests, style, merge reconciliation)

### Expected Conflicts
Minimal — mostly new files and isolated additive changes.

### Verification After Round 2
```bash
just test -p codex-thread-store
just test -p codex-state
just test -p codex-core
just test -p codex-protocol
just test -p codex-rollout
```
