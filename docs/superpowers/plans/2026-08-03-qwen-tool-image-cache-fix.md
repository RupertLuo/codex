# Qwen Tool-Image Cache Fix Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Keep Qwen tool-image continuations cacheable by preserving the function-output array and making core's incremental baseline identical to provider-visible input.

**Architecture:** Qwen policy filters image parts from the output array in place and emits them as one adjacent user message. A Qwen model capability applies the same idempotent relocation in core before incremental diffing and baseline capture; policy normalization remains a defensive wire boundary.

**Tech Stack:** Rust, Tokio, serde_json, codex-core Responses transport, Catalyst Direct Responses policy, cargo-nextest.

## Global Constraints

- Preserve both dirty primary repositories; run RED and implementation in paired sibling worktrees so private path dependencies resolve to the matching Codex checkout.
- A valid RED is an assertion failure showing `"output":""` versus `"output":[]`, or one baseline item versus two wire items. Compilation/setup failures do not count.
- Do not rewrite history, reset the client session, or include unrelated incremental logging and turn instrumentation.
- Run Codex tests through `just test`; use the private repository's documented `cargo test` commands.

---

### Task 1: Reproduce the empty-output request shape

**Files:**
- Modify/Test: `catalyst-codex-private/crates/catalyst-codex/src/qwen/policy.rs:875`

**Interfaces:**
- Consumes: `normalize_request(Value, &ModelSpec) -> CatalystResult<Value>`.
- Produces: image relocation that leaves `function_call_output.output` as an array.

- [ ] **Step 1: Add the failure-first policy test**

Add beside `moves_function_output_images_into_an_adjacent_user_message`:

```rust
#[test]
fn image_only_function_output_stays_an_array_when_image_is_relocated() {
    let normalized = normalize_request(
        json!({
            "model": "qwen/qwen3.7-max",
            "stream": true,
            "input": [{
                "type": "function_call_output",
                "call_id": "call-1",
                "output": [{
                    "type": "input_image",
                    "image_url": "data:image/jpeg;base64,aW1hZ2U="
                }]
            }]
        }),
        &model(),
    )
    .unwrap();

    assert_eq!(
        normalized["input"],
        json!([
            {"type":"function_call_output","call_id":"call-1","output":[]},
            {"type":"message","role":"user","content":[{
                "type":"input_image",
                "image_url":"data:image/jpeg;base64,aW1hZ2U="
            }]}
        ])
    );
}
```

- [ ] **Step 2: Run RED**

```powershell
cargo test -p catalyst-codex --lib qwen::policy::tests::image_only_function_output_stays_an_array_when_image_is_relocated -- --exact
```

Expected: FAIL because actual output is `String("")`, while expected output is `Array []`.

- [ ] **Step 3: Preserve the output array in production code**

Replace the text-joining logic in `normalize_function_output_images` with:

```rust
let Some(parts) = output.as_array_mut() else {
    return Ok(Vec::new());
};
let mut images = Vec::new();
for (index, part) in parts.iter().enumerate() {
    let part_path = format!("{path}.output[{index}]");
    let object = part
        .as_object()
        .ok_or_else(|| unsupported(&part_path, "must be an object"))?;
    if object.get("type").and_then(Value::as_str) == Some("input_image") {
        images.push(part.clone());
    }
}
parts.retain(|part| part.get("type").and_then(Value::as_str) != Some("input_image"));
Ok(images)
```

Change the existing mixed text/image expectation to:

```rust
assert_eq!(
    normalized["input"][0]["output"],
    json!([{"type":"input_text","text":"result"}])
);
```

- [ ] **Step 4: Run GREEN**

```powershell
cargo test -p catalyst-codex --lib qwen::policy::tests::image_only_function_output_stays_an_array_when_image_is_relocated -- --exact
cargo test -p catalyst-codex --lib qwen::policy::tests::moves_function_output_images_into_an_adjacent_user_message -- --exact
```

Expected: both tests PASS.

- [ ] **Step 5: Pin ordering and idempotence**

Add `relocates_multiple_images_once_in_order`. Normalize an output array containing
`input_text`, image URL `...YQ==`, and image URL `...Yg==`; normalize the already
normalized result a second time. Deep-compare both full JSON values and assert the
adjacent user message content is exactly:

```rust
assert_eq!(
    once["input"][1]["content"],
    json!([
        {"type":"input_image","image_url":"data:image/png;base64,YQ=="},
        {"type":"input_image","image_url":"data:image/png;base64,Yg=="}
    ])
);
assert_eq!(twice, once);
```

Run the test and commit the complete policy change:

```powershell
cargo test -p catalyst-codex --lib qwen::policy::tests::relocates_multiple_images_once_in_order -- --exact
git add crates/catalyst-codex/src/qwen/policy.rs
git commit -m "fix(qwen): preserve tool output arrays around images"
```

Expected: PASS.

### Task 2: Reproduce baseline versus wire divergence

**Files:**
- Modify/Test: `codex/codex-rs/core/src/client_tests.rs`

**Interfaces:**
- Consumes: `ModelClientSession::stream_responses_api` and `take_incremental_baseline`.
- Produces: a transport-level test comparing captured Qwen-style wire input with `HttpIncrementalSession.last_request.input`.

- [ ] **Step 1: Add a core behavioral test without adding production symbols**

Import `codex_api::RequestBody`, `FunctionCallOutputContentItem`, and `FunctionCallOutputPayload`. The test must:

1. Create an injected streaming transport like `model_client_uses_injected_http_transport`.
2. In the transport closure, deserialize the JSON body, remove `input_image` parts from each function output array, append one user image message, and capture the resulting `body["input"]`. This emulates the current late Qwen policy and is idempotent.
3. Serialize `test_model_info()`, insert JSON fields `supports_incremental_requests=true` and `relocates_tool_output_images=true`, then deserialize it. On the pre-fix protocol the unknown relocation field is ignored, so the test still compiles.
4. Stream a `Prompt` containing one image-only `FunctionCallOutput`, fully drain the SSE stream, take the incremental baseline, serialize `baseline.last_request.input`, and deep-compare it to the captured wire input.

Use this exact assertion:

```rust
let baseline = session.take_incremental_baseline();
let baseline_input = serde_json::to_value(
    &baseline
        .last_request
        .expect("incremental request baseline")
        .input,
)?;
assert_eq!(
    baseline_input,
    wire_input
        .lock()
        .expect("wire input lock")
        .clone()
        .expect("captured wire input")
);
```

- [ ] **Step 2: Run RED**

```powershell
just test -p codex-core qwen_tool_image_wire_shape_matches_incremental_baseline
```

Expected: FAIL. Baseline has one function output with a nested image; wire has an empty output array followed by a user image message.

### Task 3: Normalize before incremental baseline capture

**Files:**
- Modify: `codex/codex-rs/protocol/src/openai_models.rs:391`
- Modify: `codex/codex-rs/models-manager/src/model_info.rs:93`
- Modify: `codex/codex-rs/core/src/client.rs:1501`
- Modify/Test: `codex/codex-rs/core/src/client_tests.rs`
- Modify/Test: `catalyst-codex-private/crates/catalyst-codex/src/model_catalog.rs:85`

**Interfaces:**
- Produces: `ModelInfo::relocates_tool_output_images: bool`, serde default false.
- Produces: `relocate_tool_output_images(&mut Vec<ResponseItem>)`, invoked before `full_request_for_baseline` is cloned.

- [ ] **Step 1: Add the capability and fallback default**

In `ModelInfo`:

```rust
#[serde(default)]
pub relocates_tool_output_images: bool,
```

In models-manager's fallback initializer:

```rust
relocates_tool_output_images: false,
```

- [ ] **Step 2: Add idempotent pre-baseline relocation**

Import `FunctionCallOutputContentItem`. Implement `relocate_tool_output_images` by taking the request input, retaining non-image function-output content, converting removed images to `ContentItem::InputImage`, and appending one user `ResponseItem::Message` only when images were removed. Never convert the retained content array to a string.

Immediately after `build_responses_request` and before cloning `full_request_for_baseline`:

```rust
if model_info.relocates_tool_output_images {
    relocate_tool_output_images(&mut request.input);
}
let full_request_for_baseline = request.clone();
```

- [ ] **Step 3: Enable Qwen catalog metadata and test it**

In private `model_catalog.rs` add:

```rust
"relocates_tool_output_images": provider.id.as_str() == crate::qwen::POLICY_ID
    || model.id.as_str().starts_with("qwen/"),
```

Add `qwen_catalog_relocates_tool_output_images`, building a registry from `fixed_provider_specs()`, `qwen_model_specs()`, no adapters, and `qwen_direct_registration(true)`. Serialize the returned Qwen model and assert:

```rust
assert_eq!(serialized["relocates_tool_output_images"], json!(true));
```

Add `tool_output_image_relocation_is_ordered_and_idempotent` in
`client_tests.rs`. Construct a function output containing one text part followed
by two images, call `relocate_tool_output_images` twice, and deep-compare the full
`Vec<ResponseItem>` after each call:

```rust
relocate_tool_output_images(&mut input);
let after_first = input.clone();
relocate_tool_output_images(&mut input);
assert_eq!(input, after_first);
assert!(matches!(
    &input[1],
    ResponseItem::Message { content, .. }
        if matches!(content.as_slice(), [
            ContentItem::InputImage { image_url: first, .. },
            ContentItem::InputImage { image_url: second, .. }
        ] if first.ends_with("YQ==") && second.ends_with("Yg=="))
));
```

- [ ] **Step 4: Run GREEN**

```powershell
just test -p codex-core qwen_tool_image_wire_shape_matches_incremental_baseline
just test -p codex-core an_image_returned_by_a_tool_becomes_its_own_user_message
cargo test -p catalyst-codex --lib qwen::policy::tests::image_only_function_output_stays_an_array_when_image_is_relocated -- --exact
cargo test -p catalyst-codex --lib model_catalog::tests::qwen_catalog_relocates_tool_output_images -- --exact
```

Expected: all PASS.

- [ ] **Step 5: Commit core and private catalog changes**

```powershell
git add codex-rs/protocol/src/openai_models.rs codex-rs/models-manager/src/model_info.rs codex-rs/core/src/client.rs codex-rs/core/src/client_tests.rs
git commit -m "fix(core): normalize tool images before incremental baseline"
```

```powershell
git add crates/catalyst-codex/src/model_catalog.rs
git commit -m "fix(qwen): advertise tool image relocation"
```

### Task 4: Verify the image fix

**Files:** none beyond Tasks 1-3.

- [ ] **Step 1: Run scoped suites before formatting/lint**

```powershell
just test -p codex-core
just test -p codex-protocol
just test -p codex-models-manager
cargo test -p catalyst-codex --lib
```

- [ ] **Step 2: Format and lint**

```powershell
just fmt
just fix -p codex-core
cargo fmt --check
cargo clippy -p catalyst-codex --lib -- -D warnings
```

- [ ] **Step 3: Audit diffs**

Run `git diff --check`; confirm the policy leaves arrays, core relocation precedes baseline cloning, and no unrelated instrumentation was introduced.
