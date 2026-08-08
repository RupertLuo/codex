# DeepSeek V4 Flash text-only compaction

## Problem statement

Qwen Responses conversations can remain small on the wire by continuing from a
`previous_response_id`, but local compaction currently creates a fresh model client session and
sends the whole model-visible history. A conversation containing many inline images can therefore
trigger body-limit compaction only for the compaction request itself to exceed Qwen's 6 MiB request
limit.

Compaction is also a poor place to pay for the active coding model. Its output is a bounded textual
handoff, it intentionally starts a new history window, and the old provider-side response baseline
must not survive a successful history rewrite.

## Required behavior

- Manual `/compact`, pre-turn automatic compaction, and mid-turn automatic compaction use the
  configured compact model.
- Catalyst configures `deepseek/deepseek-v4-flash` as the compact model.
- The compact request contains no image payloads. Every image is replaced by a bounded textual
  placeholder in a cloned compaction input; the live session history is not mutated before compact
  succeeds.
- A DeepSeek compact request is a full text request and does not carry Qwen's
  `previous_response_id`. Response IDs are scoped to the provider/model conversation that created
  them and cannot be used to continue the conversation through DeepSeek.
- A successful compact atomically installs the replacement history and clears the old Qwen
  incremental baseline. A failed compact preserves both the original history and the original
  baseline.
- The thread's selected model does not change. DeepSeek is an internal summarizer only and must not
  be persisted as the previous user-turn model or emit a model-switch context item.

## Configuration

Add an optional `compact_model` product-model ID alongside `model` and `review_model` in the common
Codex configuration surfaces. `None` retains the existing behavior of compacting with the active
model, so generic Codex behavior remains backward compatible.

The Catalyst private configuration layer supplies this product default:

```toml
compact_model = "deepseek/deepseek-v4-flash"
```

The effective config, config TOML type, profile overrides, app-server config representation, and
generated schemas expose the field consistently. Model lookup uses the normal model catalog. In
the Catalyst unified transport, the product model ID resolves to the existing DeepSeek V4 Flash
adapter.

## Compaction input sanitization

Build the compaction request from a clone of the current model-visible history. Traverse all
message and tool-output content containers and remove every image content block, including images
that Qwen relocation stored in adjacent user messages.

For each containing history item, replace all removed image blocks with one text block at the
position of the first removed image:

```text
[N images omitted during compaction]
```

Retain all non-image content in its original order. An image-only message or tool output remains as
a text placeholder rather than disappearing, preserving the fact that visual evidence existed
without retaining its bytes. Placeholder text is fixed apart from the decimal count and therefore
has a hard size bound.

Sanitization affects only the DeepSeek request clone. If compaction fails, the original history,
including its images, remains available to the user and active model.

## Compact model request

Resolve a derived turn context for `compact_model` without updating the session's
`previous_turn_settings`. Build the local compact prompt using the sanitized history, the standard
base instructions, and the configured compact prompt. The compact request advertises no tools,
allows no parallel tool calls, and has no structured output schema, matching current local compact
semantics.

Use a new compact-owned model client session. Do not move the Qwen incremental baseline into that
session and do not put a Qwen `previous_response_id` in the DeepSeek request. Existing context-limit
retry behavior may remove the oldest sanitized history items when the compact model still rejects
the text for length.

Both the standalone `CompactTask` and inline auto-compaction call the same compact-model request
implementation. Trigger phase and analytics continue to distinguish manual, pre-turn, and mid-turn
operations.

## Replacement history and next-turn injection

After DeepSeek completes, extract its final assistant text and prepend the standard
`SUMMARY_PREFIX`. Preserve the current replacement-history contract:

1. retain recent real user text up to the existing 20,000-token bound;
2. append the prefixed DeepSeek summary as a `role: user` history message;
3. atomically replace the old history and persist the compacted rollout item.

Pre-turn and standalone compaction use `InitialContextInjection::DoNotInject`. The next normal Qwen
turn sees that the reference context baseline is absent, rebuilds the full canonical environment
context, and appends the new user input. The resulting model-visible order is:

```text
current Qwen base instructions
retained recent user text
prefixed DeepSeek compaction summary
fresh canonical environment/context items
new user input and turn-scoped injections
```

Mid-turn compaction preserves the existing `BeforeLastUserMessage` behavior: canonical context is
inserted into the replacement history at the trained boundary so tool/model continuation can
resume in the same turn.

On every successful variant, clear the old Qwen HTTP incremental baseline from both session-owned
state and any active turn-owned client session. The first post-compact Qwen request sends the small
replacement history in full and records a new response baseline. Later Qwen requests can again use
`previous_response_id` normally.

## Failure and cancellation semantics

History replacement is the commit point. Before it:

- cancellation, DeepSeek authentication errors, transport errors, and invalid summaries leave the
  session history unchanged;
- manual compaction leaves the session-owned Qwen baseline untouched;
- inline compaction leaves the active turn's Qwen baseline intact so the caller can return it to the
  session or continue the turn.

After replacement succeeds, failure to emit telemetry or a warning must not restore the old
history or baseline. The persisted compacted rollout item and in-memory replacement history remain
the source of truth.

## Test strategy

Implementation follows failure-first tests.

1. Sanitizer tests cover user images, relocated tool images, mixed text/image content, multiple
   images, image-only content, stable ordering, bounded placeholders, and non-mutation of the source
   history.
2. Config tests cover TOML/profile resolution, effective config, app-server round trips, and schema
   generation for `compact_model`.
3. Local compact integration tests assert that manual and automatic compact requests use
   `deepseek/deepseek-v4-flash`, contain no image URLs or base64 payloads, advertise no tools, and do
   not contain `previous_response_id`.
4. Success-state tests start with a Qwen incremental baseline, compact through DeepSeek, verify that
   the next Qwen request sends the replacement history in full, and verify that the following Qwen
   request uses the newly established `previous_response_id`.
5. Failure-state tests make DeepSeek reject or interrupt the compact request and assert deep equality
   of the original history and Qwen baseline before and after the attempt.
6. Context-layout tests cover standalone/pre-turn full reinjection and mid-turn canonical-context
   placement, and assert that no DeepSeek model-switch marker reaches Qwen.
7. Private adapter tests retain the invariant that DeepSeek rejects raw image content; the core
   integration proves sanitization happens before the request crosses that boundary.

## Scope boundaries

- Do not change Qwen's normal incremental request behavior.
- Do not use Qwen response IDs across models or providers.
- Do not delete images from persisted history unless compact succeeds and the whole history is
  replaced.
- Do not add vision support to DeepSeek or teach the compact model to call tools.
- Do not change remote provider-managed compaction endpoints; `compact_model` applies to local
  model-generated compaction.
