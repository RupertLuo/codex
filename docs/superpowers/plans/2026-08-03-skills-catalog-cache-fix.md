# Skills Catalog Cache Fix Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Stop unchanged full Skills catalogs from being appended as developer messages on every user turn while preserving real catalog changes and explicit skill instructions.

**Architecture:** `SkillsThreadState` owns one bounded, mutex-protected snapshot of the last turn-level rendered catalog. The TurnInput contributor atomically replaces the snapshot and emits only a changed, present catalog; selected-skill bodies remain independent per-turn fragments.

**Tech Stack:** Rust, Tokio, codex extension API, codex-skills-extension integration tests, cargo-nextest.

## Global Constraints

- Begin with a behavioral assertion failure on the committed pre-fix implementation.
- Compare exact rendered content, not only a hash.
- The one-time thread context catalog must not seed or overwrite the turn snapshot.
- Explicit skill bodies, executor world state, warnings, and discovery behavior remain unchanged.
- Retained state is bounded to one already-bounded rendered catalog per thread.
- Run Codex tests through `just test`.

---

### Task 1: Reproduce unchanged catalog reinjection

**Files:**
- Modify/Test: `codex/codex-rs/ext/skills/tests/skills_extension.rs`

**Interfaces:**
- Consumes: `TurnInputContributor::contribute` with one shared thread store and separate turn stores.
- Produces: integration coverage that the first identical catalog is emitted and the second is absent.

- [ ] **Step 1: Add the failure-first integration test**

Use a static custom provider so the rendered catalog is deterministic:

```rust
#[tokio::test]
async fn unchanged_turn_catalog_is_not_reinjected() -> TestResult {
    let providers = SkillProviders::new().with_provider(SkillProviderSource::new(
        SkillSourceKind::Custom("test".to_string()),
        "test",
        Arc::new(StaticSkillProvider {
            catalog: SkillCatalog {
                entries: vec![test_entry(
                    SkillSourceKind::Custom("test".to_string()),
                    "test",
                    "test/demo",
                    "skill://test/demo/SKILL.md",
                )],
                warnings: Vec::new(),
            },
            read_requests: Arc::new(Mutex::new(Vec::new())),
            list_calls: None,
            fail_first_list: false,
        }),
    ));
    let mut builder = ExtensionRegistryBuilder::new();
    install_with_providers(&mut builder, providers, skills_extension_config);
    let registry = builder.build();
    let session_store = ExtensionData::new("session");
    let thread_store = ExtensionData::new("thread");
    let config = default_config();
    registry.thread_lifecycle_contributors()[0]
        .on_thread_start(ThreadStartInput {
            config: &config,
            session_source: &SessionSource::Cli,
            persistent_thread_state_available: true,
            environments: &[],
            session_store: &session_store,
            thread_store: &thread_store,
        })
        .await;

    let mut roles_by_turn = Vec::new();
    for turn_id in ["turn-1", "turn-2"] {
        let fragments = registry.turn_input_contributors()[0]
            .contribute(
                TurnInputContext {
                    turn_id: turn_id.to_string(),
                    user_input: vec![UserInput::Text {
                        text: "hello".to_string(),
                        text_elements: Vec::new(),
                    }],
                    environments: Vec::new(),
                },
                &session_store,
                &thread_store,
                &ExtensionData::new(turn_id),
            )
            .await;
        roles_by_turn.push(
            fragments
                .iter()
                .map(|fragment| fragment.role())
                .collect::<Vec<_>>(),
        );
    }

    assert_eq!(roles_by_turn, vec![vec!["developer"], Vec::<&str>::new()]);
    Ok(())
}
```

- [ ] **Step 2: Run RED**

```powershell
just test -p codex-skills-extension unchanged_turn_catalog_is_not_reinjected
```

Expected: FAIL because actual roles are `[["developer"], ["developer"]]`.

### Task 2: Add atomic per-thread catalog deduplication

**Files:**
- Modify: `codex/codex-rs/ext/skills/src/state.rs:28`
- Modify: `codex/codex-rs/ext/skills/src/extension.rs:254`
- Test: `codex/codex-rs/ext/skills/tests/skills_extension.rs`

**Interfaces:**
- Produces: `TurnCatalogSnapshot::{Uninitialized, Absent, Rendered(String)}`.
- Produces: `SkillsThreadState::replace_turn_catalog(TurnCatalogSnapshot) -> bool`.

- [ ] **Step 1: Add snapshot state**

In `state.rs`:

```rust
#[derive(Default, Eq, PartialEq)]
pub(crate) enum TurnCatalogSnapshot {
    #[default]
    Uninitialized,
    Absent,
    Rendered(String),
}
```

Add this field to `SkillsThreadState` and initialize it with `Default::default()`:

```rust
turn_catalog: Mutex<TurnCatalogSnapshot>,
```

Add the atomic transition:

```rust
pub(crate) fn replace_turn_catalog(&self, next: TurnCatalogSnapshot) -> bool {
    let mut current = self
        .turn_catalog
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let changed = *current != next;
    let present = matches!(&next, TurnCatalogSnapshot::Rendered(_));
    *current = next;
    changed && present
}
```

- [ ] **Step 2: Gate only the turn catalog fragment**

Import `TurnCatalogSnapshot` in `extension.rs` and replace the unconditional catalog push with:

```rust
match available_skills_fragment(
    &turn_catalog,
    skills_budget(model_info.as_deref()),
    include_usage,
) {
    Some(fragment) => {
        if thread_state.replace_turn_catalog(TurnCatalogSnapshot::Rendered(fragment.render())) {
            fragments.push(Box::new(fragment));
        }
    }
    None => {
        thread_state.replace_turn_catalog(TurnCatalogSnapshot::Absent);
    }
}
```

Keep selected-skill discovery and `SkillInstructions` generation outside this match.

- [ ] **Step 3: Run GREEN**

```powershell
just test -p codex-skills-extension unchanged_turn_catalog_is_not_reinjected
```

Expected: PASS.

### Task 3: Cover changes and explicit selection

**Files:**
- Modify/Test: `codex/codex-rs/ext/skills/tests/skills_extension.rs`

**Interfaces:**
- Verifies state transitions `Uninitialized -> Rendered(A) -> Rendered(B) -> Rendered(B)` and isolation from per-turn selected skill bodies.

- [ ] **Step 1: Add changed catalog coverage**

Create three host snapshots using `SkillMetadata`: turn 1 has `demo` description `first`; turns 2 and 3 have the same path with description `second`. Contribute each turn and record developer fragment count and text. Assert:

```rust
assert_eq!(developer_counts, vec![1, 1, 0]);
assert!(second_catalog.expect("changed catalog").contains("second"));
```

Name the test `changed_turn_catalog_is_emitted_once`.

- [ ] **Step 2: Add explicit skill coverage**

Following `installed_extension_uses_host_service_snapshot`, create a temporary `demo/SKILL.md` and one shared host snapshot. Submit a plain first turn to seed the catalog. Submit `$demo` on turn 2 with the same snapshot and assert the only fragment is the user skill body:

```rust
assert_eq!(
    fragments
        .iter()
        .map(|fragment| (fragment.role(), fragment.render()))
        .collect::<Vec<_>>(),
    vec![("user", expected_skill)]
);
```

Name the test `explicit_skill_is_injected_when_catalog_is_deduplicated`.

- [ ] **Step 3: Run transition tests**

```powershell
just test -p codex-skills-extension changed_turn_catalog_is_emitted_once
just test -p codex-skills-extension explicit_skill_is_injected_when_catalog_is_deduplicated
```

Expected: both PASS.

- [ ] **Step 4: Commit the complete Skills fix**

```powershell
git add codex-rs/ext/skills/src/state.rs codex-rs/ext/skills/src/extension.rs codex-rs/ext/skills/tests/skills_extension.rs
git commit -m "fix(skills): avoid repeating unchanged turn catalogs"
```

### Task 4: Verify the Skills fix

**Files:** none beyond Tasks 1-3.

- [ ] **Step 1: Run the extension suite**

```powershell
just test -p codex-skills-extension
```

Expected: PASS.

- [ ] **Step 2: Format and lint after tests**

```powershell
just fmt
just fix -p codex-skills-extension
```

- [ ] **Step 3: Audit final diff**

Run `git diff --check`. Confirm state retains only one bounded string, the thread context contributor is untouched, and selected skill bodies remain outside the dedupe gate.

- [ ] **Step 4: Ask before the repository-wide suite**

Because the paired image change touches core and protocol, request permission before repository-wide `just test`. Do not claim full completion until it succeeds or the user explicitly waives it.
