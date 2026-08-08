# DeepSeek V4 Flash Text-Only Compaction Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make local manual and automatic compaction remove every image payload, summarize through `deepseek/deepseek-v4-flash`, atomically replace history, and re-establish Qwen incremental caching on the first post-compact turn.

**Architecture:** Add a generic optional `compact_model` configuration to Codex and set its Catalyst product default in the private launcher overrides. Keep image sanitization in a small core module, derive a request-only compact turn context, and treat history replacement as the transaction commit point that clears the old incremental baseline.

**Tech Stack:** Rust 2024, Tokio, Codex core/app-server configuration, Responses API request mocks with wiremock, Catalyst unified model transport, `just`, Cargo, and generated JSON/TypeScript schemas.

## Global Constraints

- Generic Codex defaults `compact_model` to `None`, preserving current-model compaction outside Catalyst.
- Catalyst defaults `compact_model` to the exact product ID `deepseek/deepseek-v4-flash`.
- No DeepSeek compact request contains an image URL, base64 image, image-generation result, tool spec, or `previous_response_id`.
- Sanitization operates on a cloned request history; failed compaction does not mutate persisted history.
- Successful compaction clears the old Qwen baseline; failed compaction preserves and returns it.
- DeepSeek is request-local and must not update `previous_turn_settings` or inject a model-switch marker.
- Existing remote provider-managed compaction behavior is unchanged.
- Follow repository instructions: use `just test`, run `just fmt` after Rust edits, generate affected schemas, and ask before the complete `codex-rs` test suite.

## File Structure

### Public Codex repository (`D:/catalyst-source/codex`)

- `codex-rs/config/src/config_toml.rs`: deserialize top-level `compact_model`.
- `codex-rs/config/src/profile_toml.rs`: allow profile-scoped `compact_model`.
- `codex-rs/core/src/config/mod.rs`: carry effective compact-model selection.
- `codex-rs/app-server-protocol/src/protocol/v2/config.rs`: expose the field through config reads.
- `codex-rs/core/config.schema.json` and app-server schema fixtures: generated config contract.
- `codex-rs/core/src/compact_input.rs`: clone-only image sanitization.
- `codex-rs/core/src/compact_input_tests.rs`: sanitizer deep-equality tests.
- `codex-rs/core/src/compact.rs`: resolve the compact model, sanitize its request, and commit replacement history.
- `codex-rs/core/src/client.rs`: explicit incremental-baseline clearing API.
- `codex-rs/core/src/session/mod.rs`: clear session-owned baseline at compact commit.
- `codex-rs/core/src/session/turn.rs`: pass active baseline ownership into inline compaction and restore it on pre-commit errors.
- `codex-rs/core/tests/suite/compact_model.rs`: end-to-end request shape, context layout, and baseline lifecycle.
- `codex-rs/core/tests/suite/mod.rs`: register the new integration suite.

### Catalyst private repository (`D:/catalyst-source/catalyst-codex-private`)

- `crates/catalyst-codex/src/config_overrides.rs`: set and test the DeepSeek V4 Flash product default.
- `crates/catalyst-codex/src/anthropic/request.rs`: retain the existing test proving raw images are unsupported; no production adapter change is expected.

---

### Task 1: Add the generic compact-model configuration surface

**Files:**
- Modify: `codex-rs/config/src/config_toml.rs:154-175`
- Modify: `codex-rs/config/src/profile_toml.rs:18-38`
- Modify: `codex-rs/core/src/config/mod.rs:617-640, 2377-2400, 2966-2990, 3610-3640, 3760-3770`
- Modify: `codex-rs/app-server-protocol/src/protocol/v2/config.rs:241-270`
- Modify: `codex-rs/app-server-protocol/src/protocol/v2/tests.rs:1685-1760`
- Test: `codex-rs/core/src/config/config_tests.rs`
- Test: `codex-rs/core/src/config/schema_tests.rs`
- Generate: `codex-rs/core/config.schema.json`
- Generate: `codex-rs/app-server-protocol/schema/json/**`
- Generate: `codex-rs/app-server-protocol/schema/typescript/v2/Config.ts`

**Interfaces:**
- Produces: `ConfigToml::compact_model: Option<String>`.
- Produces: `ConfigProfile::compact_model: Option<String>`.
- Produces: effective `codex_core::config::Config::compact_model: Option<String>`.
- Produces: app-server v2 config field `compact_model`, serialized as `compact_model` to match config keys.
- Consumes: the existing layered configuration merge and schema generators.

- [ ] **Step 1: Write failing config-resolution and schema tests**

Add a config test that writes both a top-level value and a profile value, builds each configuration,
and compares the exact effective field:

```rust
#[tokio::test]
async fn compact_model_resolves_from_top_level_and_profile() -> std::io::Result<()> {
    let codex_home = tempfile::TempDir::new()?;
    std::fs::write(
        codex_home.path().join(CONFIG_TOML_FILE),
        r#"compact_model = "deepseek/deepseek-v4-flash""#,
    )?;
    let top_level = ConfigBuilder::without_managed_config_for_tests()
        .codex_home(codex_home.path().to_path_buf())
        .build()
        .await?;
    assert_eq!(
        top_level.compact_model.as_deref(),
        Some("deepseek/deepseek-v4-flash")
    );

    std::fs::write(
        codex_home.path().join(CONFIG_TOML_FILE),
        r#"
profile = "cheap-compact"

[profiles.cheap-compact]
compact_model = "deepseek/deepseek-v4-flash"
"#,
    )?;
    let profile = ConfigBuilder::without_managed_config_for_tests()
        .codex_home(codex_home.path().to_path_buf())
        .build()
        .await?;
    assert_eq!(
        profile.compact_model.as_deref(),
        Some("deepseek/deepseek-v4-flash")
    );
    Ok(())
}
```

Add this sibling schema test using the existing private schema serializer:

```rust
#[test]
fn config_schema_exposes_compact_model() {
    let schema_json = config_schema_json().expect("serialize config schema");
    let schema: serde_json::Value =
        serde_json::from_slice(&schema_json).expect("decode schema json");
    let properties = schema
        .get("properties")
        .expect("ConfigToml properties should exist")
        .as_object()
        .expect("ConfigToml properties should be an object");
    assert!(properties.contains_key("compact_model"));
}
```

- [ ] **Step 2: Run the focused tests and observe RED**

Run from `codex-rs`:

```powershell
just test -p codex-core compact_model_resolves_from_top_level_and_profile
just test -p codex-core config_schema_exposes_compact_model
```

Expected: compilation fails because `compact_model` is absent from the config structs, or the
schema assertion fails because the key is absent. Do not change snapshots before observing this
failure.

- [ ] **Step 3: Add the field to all hand-written config structs and merges**

Add the field adjacent to `review_model` in top-level, profile, effective, and app-server config:

```rust
/// Model used specifically for local conversation compaction.
pub compact_model: Option<String>,
```

In the core builder, resolve the already-layered value and assign it into `Config`:

```rust
let compact_model = cfg.compact_model.clone();

let config = Self {
    model,
    service_tier,
    review_model,
    compact_model,
    // existing fields unchanged
};
```

Do not add a CLI flag or positional override. Profiles and `-c compact_model=...` already flow
through the configuration layer, and YAGNI excludes another command-line surface.

- [ ] **Step 4: Update exhaustive app-server fixtures and generate schemas**

Set `compact_model: None` in exhaustive v2 `Config` literals. Then run from `codex-rs`:

```powershell
just write-config-schema
just write-app-server-schema
just write-app-server-schema --experimental
```

Expected: generated JSON and TypeScript config schemas contain nullable `compact_model` with the
same snake_case spelling; no unrelated schema fields change.

- [ ] **Step 5: Run focused GREEN verification**

```powershell
just test -p codex-config
just test -p codex-app-server-protocol
just test -p codex-core compact_model
```

Expected: all commands pass.

- [ ] **Step 6: Format and commit the public configuration contract**

```powershell
just fmt
git add codex-rs/config/src/config_toml.rs codex-rs/config/src/profile_toml.rs codex-rs/core/src/config/mod.rs codex-rs/core/src/config/config_tests.rs codex-rs/core/src/config/schema_tests.rs codex-rs/core/config.schema.json codex-rs/app-server-protocol
git commit -m "feat(config): add local compact model selection"
```

Expected: one commit containing only the generic public configuration surface and generated files.

---

### Task 2: Set the Catalyst DeepSeek V4 Flash default

**Files:**
- Modify: `../catalyst-codex-private/crates/catalyst-codex/src/config_overrides.rs:1-20`
- Test: `../catalyst-codex-private/crates/catalyst-codex/src/config_overrides.rs:35-95`

**Interfaces:**
- Consumes: public `ConfigToml::compact_model` from Task 1.
- Produces: launcher override `compact_model="deepseek/deepseek-v4-flash"`.
- Consumes: `DEEPSEEK_V4_FLASH_PRODUCT_ID`, whose exact value is already
  `deepseek/deepseek-v4-flash`.

- [ ] **Step 1: Write the failing private default test**

Extend the existing `catalyst_overrides_enable_native_search_and_retry_dropped_streams` test with:

```rust
assert!(
    overrides
        .iter()
        .any(|value| value == "compact_model=\"deepseek/deepseek-v4-flash\""),
    "Catalyst local compaction must default to DeepSeek V4 Flash"
);
```

- [ ] **Step 2: Run the private test and observe RED**

Run from `D:/catalyst-source/catalyst-codex-private`:

```powershell
cargo test -p catalyst-codex config_overrides::tests::catalyst_overrides_enable_native_search_and_retry_dropped_streams
```

Expected: assertion failure because the override vector does not contain `compact_model`.

- [ ] **Step 3: Add the exact product-model override**

Add this entry near `model_provider="catalyst"`:

```rust
"compact_model=\"deepseek/deepseek-v4-flash\"".to_string(),
```

Use the literal configuration string at this boundary. The test pins it to the product catalog ID;
do not use the upstream-only ID `deepseek-v4-flash`.

- [ ] **Step 4: Run GREEN verification and commit in the private repository**

```powershell
cargo fmt --all -- --check
cargo test -p catalyst-codex config_overrides::tests::catalyst_overrides_enable_native_search_and_retry_dropped_streams
git add crates/catalyst-codex/src/config_overrides.rs
git commit -m "feat: default compact model to DeepSeek V4 Flash"
```

Expected: formatting and the focused test pass; the private repository records its own commit.

---

### Task 3: Add clone-only image sanitization for compact requests

**Files:**
- Create: `codex-rs/core/src/compact_input.rs`
- Create: `codex-rs/core/src/compact_input_tests.rs`
- Modify: `codex-rs/core/src/lib.rs:18-28`

**Interfaces:**
- Produces: `pub(crate) fn sanitize_for_compaction(items: &[ResponseItem]) -> Vec<ResponseItem>`.
- Produces: fixed placeholder formatter `fn omitted_images(count: usize) -> String` returning
  `[N images omitted during compaction]`.
- Consumes: `ContentItem`, `FunctionCallOutputBody`, `FunctionCallOutputContentItem`, and
  `ResponseItem` from `codex_protocol::models`.

- [ ] **Step 1: Register the test module and write deep-equality RED tests**

Declare the private module in `lib.rs`:

```rust
mod compact_input;
```

Create `compact_input.rs` with only the test-module declaration so tests initially fail on the
missing function:

```rust
#[cfg(test)]
#[path = "compact_input_tests.rs"]
mod tests;
```

In `compact_input_tests.rs`, construct one vector containing:

- a mixed user message (`InputText`, two `InputImage`, `OutputText`);
- a `FunctionCallOutput` content array with text and an image;
- a `CustomToolCallOutput` content array containing only an image;
- an `ImageGenerationCall` with a large `result` and a `revised_prompt`.

Assert the entire sanitized vector, not individual fields. The expected shapes are:

```rust
vec![
    ResponseItem::Message {
        role: "user".to_string(),
        content: vec![
            ContentItem::InputText { text: "before".to_string() },
            ContentItem::InputText {
                text: "[2 images omitted during compaction]".to_string(),
            },
            ContentItem::OutputText { text: "after".to_string() },
        ],
        // copy the source metadata fields exactly
    },
    // FunctionCallOutput keeps text and inserts the one-image placeholder.
    // CustomToolCallOutput remains a content array containing the placeholder.
    ResponseItem::Message {
        role: "assistant".to_string(),
        content: vec![
            ContentItem::InputText {
                text: "Generated image prompt: a harbor at night".to_string(),
            },
            ContentItem::InputText {
                text: "[1 image omitted during compaction]".to_string(),
            },
        ],
        // preserve id/turn passthrough metadata from ImageGenerationCall
    },
]
```

Clone the source before calling the function and finish with `assert_eq!(source, original)` to prove
non-mutation. Add a separate test asserting that a vector with no images is returned unchanged.

- [ ] **Step 2: Run sanitizer tests and observe RED**

```powershell
just test -p codex-core compact_input
```

Expected: compilation fails because `sanitize_for_compaction` is undefined.

- [ ] **Step 3: Implement ordered placeholder insertion for message content**

Use one placeholder at the first removed image position:

```rust
fn sanitize_message_content(content: &mut Vec<ContentItem>) {
    let Some(first_image) = content
        .iter()
        .position(|item| matches!(item, ContentItem::InputImage { .. }))
    else {
        return;
    };
    let image_count = content
        .iter()
        .filter(|item| matches!(item, ContentItem::InputImage { .. }))
        .count();
    content.retain(|item| !matches!(item, ContentItem::InputImage { .. }));
    content.insert(
        first_image.min(content.len()),
        ContentItem::InputText {
            text: omitted_images(image_count),
        },
    );
}
```

Implement the same operation for `Vec<FunctionCallOutputContentItem>`, inserting an `InputText`
variant and preserving `EncryptedContent` and existing text.

- [ ] **Step 4: Implement exhaustive ResponseItem sanitization**

Clone first, then mutate only the clone:

```rust
pub(crate) fn sanitize_for_compaction(items: &[ResponseItem]) -> Vec<ResponseItem> {
    let mut sanitized = items.to_vec();
    for item in &mut sanitized {
        match item {
            ResponseItem::Message { content, .. } => sanitize_message_content(content),
            ResponseItem::FunctionCallOutput { output, .. }
            | ResponseItem::CustomToolCallOutput { output, .. } => {
                if let FunctionCallOutputBody::ContentItems(content) = &mut output.body {
                    sanitize_tool_content(content);
                }
            }
            ResponseItem::ImageGenerationCall {
                id,
                revised_prompt,
                internal_chat_message_metadata_passthrough,
                ..
            } => {
                let mut content = Vec::new();
                if let Some(prompt) = revised_prompt.take() {
                    content.push(ContentItem::InputText {
                        text: format!("Generated image prompt: {prompt}"),
                    });
                }
                content.push(ContentItem::InputText {
                    text: omitted_images(1),
                });
                *item = ResponseItem::Message {
                    id: id.take(),
                    role: "assistant".to_string(),
                    content,
                    phase: None,
                    internal_chat_message_metadata_passthrough:
                        internal_chat_message_metadata_passthrough.take(),
                };
            }
            ResponseItem::AdditionalTools { .. }
            | ResponseItem::AgentMessage { .. }
            | ResponseItem::Reasoning { .. }
            | ResponseItem::LocalShellCall { .. }
            | ResponseItem::FunctionCall { .. }
            | ResponseItem::ToolSearchCall { .. }
            | ResponseItem::CustomToolCall { .. }
            | ResponseItem::ToolSearchOutput { .. }
            | ResponseItem::WebSearchCall { .. }
            | ResponseItem::Compaction { .. }
            | ResponseItem::CompactionTrigger { .. }
            | ResponseItem::ContextCompaction { .. }
            | ResponseItem::Other => {}
        }
    }
    sanitized
}
```

Use an exhaustive match without a wildcard, following repository rules. If Rust's borrow checker
rejects in-place replacement, use `std::mem::replace` with a temporary placeholder item and return
the final replacement; do not serialize through JSON.

- [ ] **Step 5: Run GREEN tests and commit**

```powershell
just test -p codex-core compact_input
just fmt
git add codex-rs/core/src/lib.rs codex-rs/core/src/compact_input.rs codex-rs/core/src/compact_input_tests.rs
git commit -m "feat(core): sanitize images from compact input"
```

Expected: deep-equality and source-non-mutation tests pass.

---

### Task 4: Route all local compaction through the configured compact model

**Files:**
- Modify: `codex-rs/core/src/compact.rs:91-220, 220-370`
- Modify: `codex-rs/core/src/tasks/compact.rs:20-90`
- Create: `codex-rs/core/tests/suite/compact_model.rs`
- Modify: `codex-rs/core/tests/suite/mod.rs`

**Interfaces:**
- Consumes: `Config::compact_model` from Task 1.
- Consumes: `compact_input::sanitize_for_compaction` from Task 3.
- Produces: `async fn resolve_compact_turn_context(...) -> Arc<TurnContext>` local to
  `compact.rs`.
- Preserves: public/manual and inline compaction entry points; remote compaction selection remains
  outside this local implementation.

- [ ] **Step 1: Write a failing manual compact wire-shape integration test**

Register `mod compact_model;` in `core/tests/suite/mod.rs`. In the new suite, mount an SSE sequence
for a normal Qwen-compatible response, a compact summary, and the next normal response. Configure:

```rust
config.compact_model = Some("deepseek/deepseek-v4-flash".to_string());
config.compact_prompt = Some("Summarize the conversation as durable text state.".to_string());
config.model_provider.name = "Catalyst-compatible test provider".to_string();
```

Submit a first user turn containing a `UserInput::Image` data URL and text, wait for completion,
then submit `Op::Compact`. Capture the compact request and assert:

```rust
assert_eq!(compact_body["model"], "deepseek/deepseek-v4-flash");
assert!(compact_body.get("previous_response_id").is_none());
assert_eq!(compact_body["tools"], serde_json::json!([]));
assert!(!compact_body.to_string().contains("data:image/"));
assert!(!compact_body.to_string().contains("base64,"));
assert!(compact_body.to_string().contains("images omitted during compaction"));
```

Use `ResponseMock::requests()` or the server's captured `/responses` requests and identify compact
by its configured compact prompt, not by request index alone.

- [ ] **Step 2: Run the manual test and observe RED**

```powershell
just test -p codex-core manual_compact_uses_configured_text_only_model
```

Expected: compact request model is the active model, and/or its body still contains image data.

- [ ] **Step 3: Resolve a request-only compact turn context**

Add this helper in `compact.rs`:

```rust
async fn resolve_compact_turn_context(
    sess: &Session,
    turn_context: &Arc<TurnContext>,
) -> Arc<TurnContext> {
    let Some(compact_model) = turn_context.config.compact_model.as_deref() else {
        return Arc::clone(turn_context);
    };
    Arc::new(
        turn_context
            .with_model(
                compact_model.to_string(),
                &sess.services.models_manager,
            )
            .await,
    )
}
```

Call it only inside the local Responses compaction entry path, before building analytics metadata
and the request. Do not call `set_previous_turn_settings`, mutate session config, or alter the
remote-compaction branch decision.

- [ ] **Step 4: Sanitize the cloned history before adding the compact prompt**

In `run_compact_task_inner_impl`, replace the cloned snapshot's items, then append the summary
instruction so retry removal operates on the sanitized data:

```rust
let mut history = sess.clone_history().await;
history.replace(crate::compact_input::sanitize_for_compaction(
    history.raw_items(),
));
history.record_items(
    &[initial_input_for_turn.into()],
    turn_context.model_info.truncation_policy.into(),
);
```

Keep the request `Prompt` tool-free and schema-free exactly as it is today. The compact-owned
`ModelClientSession` remains separate from the Qwen turn session.

- [ ] **Step 5: Add an automatic compact test using the same assertions**

Add `auto_compact_uses_configured_text_only_model` to the same integration suite. Configure an
automatic threshold low enough that the second user submission triggers pre-turn local compact.
Assert the compact request has the exact DeepSeek model ID, no images, no tools, and no
`previous_response_id`; assert the normal post-compact request returns to the original active model.

- [ ] **Step 6: Run GREEN tests and ensure no internal model switch is persisted**

```powershell
just test -p codex-core compact_model
just test -p codex-core compact
```

Extend the captured post-compact request assertion:

```rust
assert_eq!(post_compact_body["model"], "gpt-5.4");
assert!(!post_compact_body.to_string().contains("<model_switch>"));
assert!(!post_compact_body.to_string().contains("deepseek/deepseek-v4-flash"));
```

Expected: both focused suites pass.

- [ ] **Step 7: Format and commit model routing**

```powershell
just fmt
git add codex-rs/core/src/compact.rs codex-rs/core/src/tasks/compact.rs codex-rs/core/tests/suite/compact_model.rs codex-rs/core/tests/suite/mod.rs
git commit -m "feat(core): compact through configured text model"
```

---

### Task 5: Make history replacement the incremental-baseline commit point

**Files:**
- Modify: `codex-rs/core/src/client.rs:1210-1225`
- Modify: `codex-rs/core/src/session/mod.rs:1225-1245, 3010-3055`
- Modify: `codex-rs/core/src/compact.rs:91-220, 320-370`
- Modify: `codex-rs/core/src/session/turn.rs:150-170, 350-375, 975-1047`
- Test: `codex-rs/core/tests/suite/compact_model.rs`

**Interfaces:**
- Produces: `ModelClientSession::clear_incremental_baseline(&mut self)`.
- Produces: `Session::clear_http_incremental_baseline(&self)`.
- Changes: inline local compact entry accepts `&mut ModelClientSession` solely to clear the active
  Qwen baseline at the history replacement commit point; DeepSeek still uses its own session.
- Preserves: pre-commit errors leave the active Qwen baseline available for restoration.

- [ ] **Step 1: Write the post-compact cache lifecycle RED test**

Extend the integration sequence to four meaningful requests:

1. initial Qwen request returns `resp-before-compact`;
2. DeepSeek compact request returns the summary;
3. first Qwen post-compact request returns `resp-after-compact`;
4. second Qwen request returns `resp-incremental-again`.

Enable `supports_incremental_requests` on the active Qwen model. Assert complete request behavior:

```rust
assert!(compact_body.get("previous_response_id").is_none());
assert!(post_compact_body.get("previous_response_id").is_none());
assert!(post_compact_body.to_string().contains(SUMMARY_PREFIX));
assert!(!post_compact_body.to_string().contains("data:image/"));
assert_eq!(
    second_post_compact_body["previous_response_id"].as_str(),
    Some("resp-after-compact")
);
```

Also assert the first post-compact body contains fresh environment context and the new user input,
proving `DoNotInject` caused full next-turn context reinjection.

- [ ] **Step 2: Write the pre-commit failure preservation RED test**

Mount a successful first Qwen response, then make DeepSeek compact return HTTP 500 or a terminal
provider error. Submit the next normal Qwen turn after the compact failure and assert it still uses:

```rust
assert_eq!(
    follow_up_body["previous_response_id"].as_str(),
    Some("resp-before-failed-compact")
);
```

Assert that the follow-up does not contain `SUMMARY_PREFIX`, proving history was not replaced.

- [ ] **Step 3: Run both tests and observe RED**

```powershell
just test -p codex-core compact_success_resets_then_reestablishes_incremental_baseline
just test -p codex-core compact_failure_preserves_incremental_baseline_and_history
```

Expected: at least the failed pre-turn path loses its baseline, and successful compact relies on an
implicit prefix mismatch rather than an explicit clear.

- [ ] **Step 4: Add explicit clear methods**

In `ModelClientSession`:

```rust
pub(crate) fn clear_incremental_baseline(&mut self) {
    self.http_session = HttpIncrementalSession::default();
}
```

In `Session`:

```rust
pub(crate) async fn clear_http_incremental_baseline(&self) {
    let mut state = self.state.lock().await;
    state.http_incremental_baseline = Default::default();
}
```

Do not reset the entire model client session, websocket routing state, or sticky routing state.

- [ ] **Step 5: Thread active baseline ownership into local inline compaction**

Change the local inline signature to accept the active session:

```rust
pub(crate) async fn run_inline_auto_compact_task(
    sess: Arc<Session>,
    turn_context: Arc<TurnContext>,
    active_client_session: &mut ModelClientSession,
    initial_context_injection: InitialContextInjection,
    reason: CompactionReason,
    phase: CompactionPhase,
) -> CodexResult<()>;
```

Propagate `Some(active_client_session)` through local compact internals. Standalone `/compact`
passes `None`. Continue creating a distinct compact-owned client session for DeepSeek requests.

- [ ] **Step 6: Clear both possible owners exactly at history replacement**

Immediately after `replace_compacted_history(...).await` succeeds and before post-compact hooks:

```rust
sess.clear_http_incremental_baseline().await;
if let Some(active_client_session) = active_client_session {
    active_client_session.clear_incremental_baseline();
}
```

Arrange borrows so this code executes only after persisted/in-memory replacement succeeds. If
`replace_compacted_history` cannot fail with its current return type, place the clear immediately
after it; do not clear earlier when the DeepSeek response first arrives.

- [ ] **Step 7: Restore the active baseline on inline pre-commit errors**

Before the pre-turn and mid-turn compact error branches return `Ok(None)`, move a still-present
baseline back to the session:

```rust
if client_session.has_incremental_baseline() {
    sess.store_http_incremental_baseline(client_session.take_incremental_baseline())
        .await;
}
```

Because Step 6 clears the active baseline at commit, a post-commit hook error cannot accidentally
restore the old Qwen baseline. A pre-commit DeepSeek failure leaves it present and therefore
restores it.

- [ ] **Step 8: Add the mid-turn context placement assertion**

Trigger a tool continuation and mid-turn compact. Assert the sanitized DeepSeek compact body, then
assert the Qwen continuation contains canonical context before the last real user/summary boundary,
matching `InitialContextInjection::BeforeLastUserMessage`. Compare the captured JSON item array
directly: locate the canonical context item, last real user item, and summary item by their text and
assert their indices are strictly increasing. Do not add a snapshot for these structural assertions.

- [ ] **Step 9: Run focused GREEN verification**

```powershell
just test -p codex-core compact_model
just test -p codex-core incremental_interrupt_cache
just test -p codex-core compact
```

Expected: compact lifecycle, existing interrupt-cache behavior, and existing compact suite all
pass. Review any `.snap.new` file directly and accept only the intentional compact-model layout.

- [ ] **Step 10: Run scoped lint/format and commit**

```powershell
just fix -p codex-core
just fmt
git add codex-rs/core/src/client.rs codex-rs/core/src/session/mod.rs codex-rs/core/src/session/turn.rs codex-rs/core/src/compact.rs codex-rs/core/tests/suite/compact_model.rs
git commit -m "fix(core): commit compact baseline transitions atomically"
```

Per repository instructions, do not rerun tests after `just fix` or `just fmt`.

---

### Task 6: Verify the public/private integration boundary

**Files:**
- Verify: `codex-rs/core/config.schema.json`
- Verify: `codex-rs/app-server-protocol/schema/json/**`
- Verify: `../catalyst-codex-private/crates/catalyst-codex/src/config_overrides.rs`
- Verify: `../catalyst-codex-private/crates/catalyst-codex/src/anthropic/request.rs:1693-1703`

**Interfaces:**
- Consumes: all prior task commits.
- Produces: evidence that Catalyst's override parses through public Codex config and DeepSeek never
  receives raw images.

- [ ] **Step 1: Run the public targeted test matrix**

From `D:/catalyst-source/codex/codex-rs`:

```powershell
just test -p codex-config
just test -p codex-app-server-protocol
just test -p codex-core compact_model
just test -p codex-core compact_input
just test -p codex-core incremental_interrupt_cache
```

Expected: all commands pass.

- [ ] **Step 2: Run the private targeted test matrix**

From `D:/catalyst-source/catalyst-codex-private`:

```powershell
cargo test -p catalyst-codex config_overrides
cargo test -p catalyst-codex anthropic::request::tests
```

Expected: the Catalyst override parses and the existing DeepSeek adapter test continues to reject a
raw `input_image` with `ADAPTER_CONTENT_UNSUPPORTED`.

- [ ] **Step 3: Inspect captured request evidence**

Run the new core integration test with output enabled:

```powershell
just test -p codex-core compact_success_resets_then_reestablishes_incremental_baseline -- --nocapture
```

Confirm the captured sequence has these exact invariants:

```text
Qwen initial:                 no previous_response_id, may contain images
DeepSeek compact:             no previous_response_id, no images, no tools
Qwen first after compact:     no previous_response_id, contains summary + fresh context
Qwen second after compact:    previous_response_id = resp-after-compact
```

- [ ] **Step 4: Inspect repository state and diffs**

In both repositories:

```powershell
git status --short --branch
git diff --check origin/main...HEAD
git log --oneline --decorate origin/main..HEAD
```

Expected: no uncommitted implementation files, no whitespace errors, and only the planned commits.
Do not run the complete `codex-rs` test suite without explicit user approval.
