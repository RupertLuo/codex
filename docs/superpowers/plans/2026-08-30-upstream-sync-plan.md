# Upstream Sync Plan — 2026-08-30

## Situation

- **Fork base**: `ccdfb4f342a` (upstream 2026-06-28, PR #30508)
- **Fork patches**: 111 functional commits; 113 total branch commits including
  the sync plan and Round 1 test-backfill commits (+28,253 / -1,768 lines across
  243 files)
- **Upstream target**: `upstream/main` at `28327355b` (fetched 2026-08-30)
- **Upstream gap**: 2,098 commits from the common ancestor to the target
- **Branch**: `feat/yanjiang` (currently at `188f89112`)
- **Goal**: Rebase fork patches onto latest upstream `main`

## Patch Inventory (111 commits in 13 logical groups)

### A — HTTP Transport Injection (3 commits)
Lets Catalyst intercept all model requests for provider routing.
- `002e7d3` feat(client): add type-erased HTTP transport handle
- `31e6489` feat(core): inject custom HTTP transport into agent turns
- `a5cde87` feat(core): expose transport override state
- **Files**: `codex-client/src/transport_handle.rs` (NEW), `client.rs`, `session/mod.rs`, `thread_manager.rs`
- **Conflict risk**: MEDIUM — mostly additive, but touches session constructor

### B — Model Catalog & TUI Provider Runtime (14 commits)
Provider-first model selection, generic credential workflows in TUI.
- `0fdb28a`..`668a8ef` (see full list in git log)
- **Files**: `tui/src/model_runtime.rs` (NEW), `tui/src/lib.rs`, `tui/src/app.rs`, `chatwidget/credential_popups.rs` (NEW), `chatwidget/model_popups.rs`
- **Conflict risk**: MEDIUM — TUI is actively evolving upstream (key chords, model picker refresh)

### C — Runtime Extensions & Native Agent Spawning (18 commits)
RPC extension registry, native agent forks, plugin root management.
- `a08b015`..`92f5c2a` + merges
- **Files**: `app-server/src/rpc_extension.rs` (NEW), `app-server/src/extensions.rs`, `core/src/thread_manager.rs`, `thread-store/src/live_thread.rs`
- **Conflict risk**: HIGH — upstream has Guardian, multi-agent, plugin marketplace changes in same areas

### D — Skills Provider & Prompt Debug (11 commits)
Host skill providers, mandatory instructions, standalone tool detection.
- `770346c`..`dccd966`
- **Files**: `ext/skills/`, `core-skills/`, `core/src/prompt/`
- **Conflict risk**: MEDIUM — upstream has skill catalog rendering refactor, Cursor migration

### E — Qwen Incremental Requests (11 commits)
`previous_response_id` delta requests, byte estimation, image cache alignment.
- `72a08cf`..`9150b66` + docs
- **Files**: `core/src/client.rs`, `core/src/session/mod.rs`, `core/src/realtime_conversation.rs`
- **Conflict risk**: HIGH — `client.rs` reasoning function modified, `session/mod.rs` deeply touched

### F — Compaction Transactionality (16 commits)
9 atomicity fixes + DeepSeek Flash text-only compaction + auto-compact switch.
- `b930d14`..`fbaa58b` + docs
- **Files**: `core/src/compact.rs`, `core/src/session/mod.rs`, `core/src/tasks/mod.rs`, `protocol/src/protocol.rs`, `rollout/src/policy.rs`
- **Conflict risk**: CRITICAL — upstream has context rollover, auto-compaction fallback in same files

### G — Platform Fixes (6 commits)
PowerShell UTF-8, CRLF migration, managed shell profile.
- `674f717`..`e30f508`
- **Files**: `core/src/shell_spawn.rs`, `state/src/migrations.rs`
- **Conflict risk**: LOW — mostly additive, isolated platform code

### H — Thread Title Generation (3 commits)
LLM-generated thread titles.
- `d59b47d`..`4d7ad8c`
- **Files**: `thread-store/src/title_generator.rs` (NEW), `thread-store/src/live_thread.rs`, `thread-store/src/thread_metadata_sync.rs`
- **Conflict risk**: LOW — new module + additive methods

### I — Thread Store & Runtime Options (3 commits)
Thread source preservation, runtime options propagation.
- `604be1c`, `94fa124`, `a7a9dbd`
- **Files**: `thread-store/src/in_memory.rs`, `app-server/`, `tui/src/lib.rs`
- **Conflict risk**: MEDIUM — upstream has paginated history, thread section APIs

### J — Image Runtime Extensions (2 commits + merge)
Product image (Seedream) visibility in extensions.
- `cc218e3`, `9d45c52`, `8e7160a`
- **Conflict risk**: LOW

### K — Auto-Compact Switch (3 commits, overlap with F)
Already counted in F.

### L — Misc Fixes (2 commits)
Provider error classification, recursion limit.
- `2bc72e4`, `1398489`
- **Conflict risk**: LOW

### M — Reasoning Effort Decoupling (1 commit)
- `0c04dec` feat(core): decouple reasoning effort from summaries
- **Conflict risk**: LOW — upstream added Ultra effort but shouldn't collide

### Z — Housekeeping (10 commits)
Tests, style, merge reconciliation, metadata refresh.
- **Conflict risk**: LOW-MEDIUM — test fixtures may need updating

---

## Test Coverage Gaps (must fix BEFORE rebase)

| Gap | Files | Lines | Priority |
|-----|-------|-------|----------|
| `thread-store` crate | `live_thread.rs`, `in_memory.rs`, `thread_metadata_sync.rs`, `title_generator.rs` | +1,609 | **P0** |
| `realtime_conversation.rs` | 28 hunks, +530 lines | +530 | **P1** |
| `state/runtime/threads.rs` | SQL query changes | +391 | **P1** |
| `app-server/src/cli.rs` | new file | +159 | P2 |

## Current Conflict Hotspots (measured against `upstream/main@28327355b`)

The common ancestor is `ccdfb4f342a`; 213 files are touched by both sides. The
largest semantic collision surfaces are:

| File | Fork delta | Upstream delta | Handling |
|------|------------|----------------|----------|
| `core/src/session/mod.rs` | +1,318/-166 | +1,431/-954 | upstream lifecycle first, then reapply invariants |
| `thread-store/src/live_thread.rs` | +1,025/-9 | +126/-31 | preserve upstream metadata/title lifecycle |
| `core/src/realtime_conversation.rs` | +648/-69 | +1,067/-306 | port state machine onto upstream reconnect/attach flow |
| `core/src/compact.rs` | +559/-68 | +177/-90 | migrate transaction invariants one commit at a time |
| `protocol/src/protocol.rs` | +486/-88 | +799/-577 | regenerate protocol/schema after semantic merge |
| `core/src/thread_manager.rs` | +482/-18 | +807/-358 | retain upstream plugin/Guardian ownership boundaries |
| `state/src/runtime/threads.rs` | +433/-42 | +663/-191 | preserve SQL behavior with old-data round-trip tests |
| `app-server/src/extensions.rs` | +226/-1 | +424/-17 | retain upstream security and capability scoping |
| `tui/src/app.rs` | +78/0 | +181/-592 | rebase onto current TUI orchestration, then snapshots |

Because the overlap is broad, high-risk groups E/F should use behavior-level
reapplication after the upstream structure is established, rather than blindly
accepting either side or forcing a 113-commit mechanical rebase.

---

## Execution Plan: 7 Rounds

### Pre-Round 0: Infrastructure + Fetch
**Owner**: Human/agent with a provisioned build environment
**Task**: Provision the Rust/Bazel toolchain, then keep `upstream/main` fetched locally
```bash
# Use proxy that works:
git -c http.proxy=http://127.0.0.1:12334 fetch upstream main
# Or from a faster network:
git fetch upstream main
```
**Deliverable**: `upstream/main` ref pointing to the selected stable commit,
plus working `rustc`, `cargo`, `just`, Bazel and (where needed) rusty-v8 artifacts.

**Current status**: `upstream/main` is already at `28327355b`, but this
environment has no `rustc`, `cargo`, or `just`, so test execution and Rust
conflict validation cannot begin until infrastructure is provisioned.

---

### Round 1: Test Gap Backfill (on feat/yanjiang, before rebase)
**Goal**: Add missing tests so we have a safety net for rebase
**Scope**:
1. `thread-store` tests: `live_thread.rs` transaction + checkpoint, `title_generator.rs` basic
2. `realtime_conversation.rs` — at least constructor + key event handler tests
3. `state/runtime/threads.rs` — SQL round-trip test for new fields/queries

**Estimated effort**: 800-1200 lines of test code
**Verification**: `just test -p codex-thread-store && just test -p codex-core && just test -p codex-state`
**Commit**: "test: backfill coverage for thread-store, realtime, state before upstream sync"

---

### Round 2: Rebase — Low-Risk Groups (G, H, J, L, M, Z)
**Goal**: Rebase the easy patches first to establish a stable base
**Groups**: G (platform), H (thread-title), J (image-ext), L (misc), M (reasoning), Z (housekeeping)
**Commit count**: ~27 commits
**Strategy**: `git rebase --onto upstream/main <base> <last-commit-of-these-groups>`
**Expected conflicts**: Minimal — mostly new files and isolated additive changes
**Verification**: `just test -p codex-core -p codex-tui -p codex-thread-store -p codex-state`

---

### Round 3: Rebase — Transport + Model Catalog (A, B)
**Goal**: Re-land the Catalyst transport injection and TUI model runtime
**Groups**: A (3 commits), B (14 commits)
**Commit count**: 17 commits
**Expected conflicts**: 
- `tui/src/lib.rs` — parameter threading (mechanical)
- `tui/src/app.rs` — new fields (mechanical)
- `core/src/session/mod.rs` — constructor additions (needs care)
**Verification**: `just test -p codex-tui -p codex-core -p codex-client`

---

### Round 4: Rebase — Skills & Extensions (C, D)
**Goal**: Re-land RPC extensions, native agent spawning, skill providers
**Groups**: C (18 commits), D (11 commits)
**Commit count**: 29 commits
**Expected conflicts**:
- `app-server/src/extensions.rs` — upstream has Guardian/multi-agent additions
- `core/src/thread_manager.rs` — upstream has plugin lifecycle changes
- `ext/skills/` — upstream has skill catalog refactor
**Verification**: `just test -p codex-app-server -p codex-core -p codex-ext-skills`

---

### Round 5: Rebase — Qwen Incremental (E)
**Goal**: Re-land delta requests, byte estimation, image cache
**Groups**: E (11 commits)
**Expected conflicts**:
- `core/src/client.rs` — reasoning function (needs manual resolution)
- `core/src/session/mod.rs` — incremental baseline (HIGH risk, interleaved with upstream context management)
- `core/src/realtime_conversation.rs` — 28 hunks (tedious)
**Verification**: `just test -p codex-core` (specifically incremental + image cache tests)

---

### Round 6: Rebase — Compaction Transactionality (F)
**Goal**: Re-land the hardest group — 16 commits of atomicity fixes + DeepSeek Flash compaction
**Groups**: F (16 commits)
**Expected conflicts**:
- `core/src/session/mod.rs` — persistence lifecycle rewrite (CRITICAL)
- `core/src/compact.rs` — function signatures + logic (CRITICAL)
- `core/src/tasks/mod.rs` — compaction task scheduling (HIGH)
- `protocol/src/protocol.rs` — RolloutTransaction types (HIGH)
**Strategy**: Rebase commit-by-commit (`git rebase -i`), resolve each atomically
**Verification**: `just test -p codex-core` — especially `compact_model.rs` suite (5,972 lines)

---

### Round 7: Full Validation + Cleanup
**Goal**: Confirm everything works together
**Tasks**:
1. `just fmt` (codex-rs directory)
2. `just fix` (full workspace)
3. `just test` (full workspace — ask human permission first)
4. Review any new upstream features that need Catalyst adaptation
5. Update `docs/superpowers/plans/` with sync record

**Deliverable**: Clean `feat/yanjiang` branch rebased on latest upstream

---

## Handoff Protocol

Each round is designed to be **independently executable** by a fresh agent session. The handoff document for each round should contain:

1. **Branch state**: Current HEAD commit hash + what's been completed
2. **Next round number and scope**: Which group(s) to rebase next
3. **Known conflicts from previous round**: What was resolved and how
4. **Test status**: Which test suites passed/failed
5. **Blockers**: Anything that needs human intervention

### Handoff Template

```markdown
## Handoff — Round N Complete

### Branch State
- Branch: `feat/yanjiang`
- HEAD: `<commit-hash>`
- Upstream base: `<upstream-commit-hash>`
- Rounds completed: 1..N

### What Was Done
- Rebased groups: <list>
- Conflicts resolved: <list with resolution notes>
- Tests passing: <list of test commands and results>

### Next Round: N+1
- Groups to rebase: <list>
- Expected conflict files: <list>
- Strategy: <notes>

### Blockers
- <any issues needing human attention>
```

---

## Risk Mitigation

1. **Before each round**: Tag the current state (`git tag pre-round-N`)
2. **After each round**: Run targeted tests before proceeding
3. **Escape hatch**: If a round is too messy, `git reset --hard pre-round-N` and try a different approach
4. **Nuclear option**: If rebase is unworkable for groups E/F, consider `git merge upstream/main` instead and cherry-pick the fork patches on top of the merged result

## Estimated Total Effort

| Round | Effort | Risk |
|-------|--------|------|
| Pre-0 | 5 min (human) | None |
| 1 | 2-3 hours | Low |
| 2 | 30 min | Low |
| 3 | 1-2 hours | Medium |
| 4 | 2-3 hours | Medium-High |
| 5 | 3-4 hours | High |
| 6 | 4-6 hours | Critical |
| 7 | 1-2 hours | Low |
| **Total** | **~14-21 hours** | |
