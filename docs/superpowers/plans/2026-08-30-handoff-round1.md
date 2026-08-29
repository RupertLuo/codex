# Handoff — Round 0 Complete, Ready for Round 1

## Branch State
- Branch: `feat/yanjiang`
- HEAD: `4d7ad8ce7` (fix(thread-title): publish generated names)
- Fork base (upstream): `ccdfb4f342a` (2026-06-28)
- Rounds completed: 0 (planning only)

## What Was Done (Round 0)
- Created `feat/yanjiang` branch from current `main`
- Analyzed all 111 fork patches, classified into 13 logical groups (A-M, Z)
- Identified 3 critical test coverage gaps via sub-agent deep analysis
- Wrote sync plan: `docs/superpowers/plans/2026-08-30-upstream-sync-plan.md`
- Wrote this handoff: `docs/superpowers/plans/2026-08-30-handoff-round1.md`

## Blocker for Rounds 2-7
`upstream/main` has not been fetched yet. The machine had network issues reaching GitHub.
On the new machine, run first:
```bash
# Ensure upstream remote uses SSH through github-second alias:
git remote set-url upstream git@github-second:openai/codex.git
# Or if that host alias isn't configured, use direct HTTPS:
git remote set-url upstream https://github.com/openai/codex.git
# Then:
git fetch upstream main
```
Upstream HEAD is approximately `6478a75` (PR #41477, 2026-08-29).

---

## Next: Round 1 — Test Gap Backfill

### Goal
Add minimum necessary tests for the 3 uncovered areas so that post-rebase
regression is detectable. Do NOT start rebasing yet.

### Commit Strategy
One commit: `test: backfill coverage for thread-store, realtime, state before upstream sync`

### Verification Commands
```bash
cd codex-rs
just test -p codex-thread-store
just test -p codex-core
just test -p codex-state
```

---

## Part 1: thread-store Tests

### Current State
The fork added +1,609 lines to `codex-rs/thread-store/` with these already-existing tests:
- `live_thread.rs` has 10 `#[tokio::test]` tests (lines 656-1285) covering transaction
  append reconciliation, checkpoint reconciliation, and title mutation permits
- `thread_metadata_sync.rs` has 2 new tests covering title dispatch and checkpoint projection
- `local/mod.rs` has 1 new test for title overwrite behavior
- `in_memory.rs` has 0 new tests (only pre-fork tests)

### Tests to Add

**File: `codex-rs/thread-store/src/live_thread.rs`** (add to existing `mod tests`)

1. **`sanitize_title_*`** — The `sanitize_title` function (line 637) is pure with ZERO tests:
   - `sanitize_title_passthrough` — plain text unchanged
   - `sanitize_title_strips_double_quotes` — `"Hello World"` → `Hello World`
   - `sanitize_title_strips_cjk_quotes` — `「标题」` → `标题`, `《标题》` → `标题`
   - `sanitize_title_strips_trailing_punctuation` — `Hello.` → `Hello`, `你好。` → `你好`
   - `sanitize_title_first_nonempty_line` — `\n\nHello\nWorld` → `Hello`
   - `sanitize_title_truncates_at_30_chars` — long string truncated
   - `sanitize_title_empty_input` — `"  "` → `""`
   - `sanitize_title_combined` — `"\"Very Long Quoted Title.\""` → stripped + truncated

2. **`start_rejects_when_title_generator_absent`** — `live_thread` with no generator →
   `maybe_dispatch_llm_title` is a no-op (verify `llm_title_dispatched` stays false)

**File: `codex-rs/thread-store/src/in_memory.rs`** (add to existing `mod tests`)

3. **`title_generator_round_trip`** — `set_title_generator()` then `title_generator()` returns `Some`
4. **`failed_append_may_become_durable_returns_false`** — confirms the override

### Implementation Notes
- `sanitize_title` is `fn sanitize_title(raw: &str) -> String` — private but testable from
  within the same module's `#[cfg(test)]` block
- Use existing test helpers: `StubTitleGenerator`, `create_params()`, `live_thread()`
- Estimated: ~120 lines of test code

---

## Part 2: realtime_conversation Tests

### Current State
The fork added +530 lines in 28 hunks to `codex-rs/core/src/realtime_conversation.rs`:
- Replaced fire-and-forget close with transactional claim-based close
- New types: `ManagedConversationState`, `RealtimeClosingState`, `RealtimeCloseClaim`
- New methods: `claim_close`, `complete_close`, `release_close_for_retry`
- Modified: `start` (rejects when Closing), `shutdown` (preserves Closing), `handle_close`

Existing test coverage:
- `realtime_conversation_tests.rs`: 206 lines covering handoff text extraction, delegation
  wrapping — **zero tests for new close/persistence logic**
- `tests/suite/compact_model.rs` lines 4870-5069: 4 integration tests using
  `RealtimeStartTestHook` that exercise durable close retry — **covers the happy path**

### Tests to Add

**File: `codex-rs/core/src/realtime_conversation_tests.rs`** (add to existing file)

Unit tests for `RealtimeConversationManager` state machine:

1. **`claim_close_active_transitions_to_closing`** — Active state → `claim_close(Current)` →
   returns `Some(claim)` with `conversation: Some(...)`, manager state becomes Closing
2. **`claim_close_returns_none_when_empty`** — no conversation → returns `None`
3. **`claim_close_closing_in_progress_returns_none`** — already Closing+in_progress →
   second `claim_close` returns `None`
4. **`claim_close_released_returns_retry_claim`** — claim → `release_close_for_retry` →
   second claim succeeds with `conversation: None`
5. **`complete_close_clears_state`** — claim → `complete_close(token)` → state is None
6. **`complete_close_wrong_token_no_op`** — `complete_close(wrong_token)` → state unchanged
7. **`start_rejects_when_closing`** — Closing state → `start()` returns error
8. **`running_state_none_when_closing`** — Closing → `running_state()` returns None
9. **`realtime_close_reason_maps_all_variants`** — exhaustive check of enum→string

### Implementation Notes
- **Key challenge**: `claim_close` requires the manager in Active state, which normally requires
  calling `start()` with a websocket. Two approaches:
  - Option A: Add a `#[cfg(test)]` helper that puts manager directly into Active state with a
    mock `ConversationState` (preferred — fast, no I/O)
  - Option B: Use mock websocket server from integration tests (heavier)
- The `RealtimeConversationManager::new()` constructor takes `(event_sender, ResponsesApiClient)`.
  For unit tests, use `tokio::sync::mpsc::channel` for sender and a mock client.
- `ManagedConversationState` and `RealtimeClosingState` are private, so tests must go inside
  the same module or use the public API surface
- Estimated: ~200 lines of test code

---

## Part 3: state/extract.rs Tests

### Current State
The fork changes to `codex-rs/state/` are **already well-tested** by 8 new tests in
`runtime/threads.rs` and 4 new tests in `migrations_tests.rs`. The remaining gaps are small:

### Tests to Add

**File: `codex-rs/state/src/extract.rs`** (add to existing `mod tests`)

1. **`apply_rollout_item_transaction_updates_metadata`** — `RolloutItem::Transaction` containing
   a `TokenCount` event → `metadata.tokens_used` is updated
2. **`rollout_item_affects_metadata_compacted_without_checkpoint`** — `Compacted { checkpoint: None }`
   → returns `Ok(false)`
3. **`compacted_with_reference_context_applies_turn_context`** — `Compacted` with
   `reference_context_item: Some(turn_context)` → metadata model/reasoning_effort are set

### Implementation Notes
- Follow existing pattern in `extract.rs` tests: build `ThreadMetadata` via `metadata_for_test()`,
  call `apply_rollout_item(&mut metadata, &item, "provider")`, assert fields
- For `Transaction` variant, construct `RolloutItem::Transaction(RolloutTransaction { ... })`
  wrapping inner items
- For `Compacted` with `reference_context_item`, build a `TurnContext` with known values
- Estimated: ~80 lines of test code

---

## Summary: Round 1 Deliverables

| Area | File to Edit | Tests to Add | Est. Lines |
|------|-------------|--------------|------------|
| thread-store | `live_thread.rs` | 10 tests (sanitize_title + no-generator) | ~120 |
| thread-store | `in_memory.rs` | 2 tests (generator round-trip, durable flag) | ~20 |
| realtime | `realtime_conversation_tests.rs` | 9 tests (claim/close state machine) | ~200 |
| state | `extract.rs` | 3 tests (Transaction, Compacted variants) | ~80 |
| **Total** | | **24 tests** | **~420 lines** |

After committing, run:
```bash
cd codex-rs
just test -p codex-thread-store
just test -p codex-core
just test -p codex-state
```

All must pass. Then update this handoff and proceed to Round 2 (or fetch upstream first).

---

## After Round 1: What's Next

### If upstream is fetched:
Proceed to Round 2 (rebase low-risk groups G/H/J/L/M/Z).
See `2026-08-30-upstream-sync-plan.md` for the full 7-round plan.

### If upstream is NOT yet fetched:
Run `git fetch upstream main` on the better-networked machine first.
The upstream remote should be set to:
```
git@github-second:openai/codex.git   (SSH via port 443)
# or
https://github.com/openai/codex.git  (HTTPS, may need proxy)
```
