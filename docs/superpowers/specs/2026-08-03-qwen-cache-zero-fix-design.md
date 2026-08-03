# Qwen previous-response cache-zero fix

## Problem statement

One local request-shape mismatch deterministically breaks Qwen requests that use
`previous_response_id`; a separate provider-side multimodal cache limitation can
still produce cache misses after the local mismatch is fixed.

1. Tool output images are rewritten from one `function_call_output` item into a
   `function_call_output` followed by a user image message. The old Qwen policy
   replaces an image-only output array with an empty string and performs the
   one-to-two-item rewrite after the incremental request baseline has been
   captured. The provider therefore receives an unstable empty output shape and
   the local baseline describes one item while the provider stores two.
Production logs provide an A/B for the image path. In the failing process the
incremental tracker recorded one item while the Qwen wire request contained two,
and every image-only tool result was serialized as `"output":""`. Six of six
matched image requests reported zero cached tokens. After the image rewrite was
moved before baseline capture, both tracker and wire request contained
`["function_call_output", "message"]`, the tool output remained the array
`"output":[]`, and nine of ten matched image requests reported cached tokens.

Live follow-up testing found a residual Qwen behavior that this structural fix does not solve:

- The Responses schema documents `function_call_output.output` as a string. Qwen sometimes returns
  HTTP 200 for an OpenAI-style image array in that field, but reports zero `image_tokens` and answers
  a solid-color probe incorrectly. The same image in an adjacent user message reports 534 image
  tokens and the correct RGB value. Other attempts reject the tool-output array with HTTP 400.
- With session cache enabled, a request containing an image can create a large ephemeral cache block
  and still leave its response id unable to read that block on the next request. In one controlled
  branch, the post-image id returned zero cached tokens for a text-only follow-up, while the original
  pre-image id still hit 96,646 cached tokens. Waiting 15 seconds did not change the result.
- The affected production rollout contains one Codex turn and one Skills catalog insertion. Repeated
  Skills instructions are therefore not the cause of the continuous post-image misses, and no Skills
  deduplication change belongs in this fix.

The local fix remains necessary because it makes the baseline identical to the wire request. It
cannot guarantee that Qwen's provider-side multimodal session cache will hit.

## Required behavior

- Every provider-visible structural rewrite must happen before the request is
  diffed against, or stored as, an incremental baseline.
- A tool output image must remain visible to Qwen as an adjacent user image
  message because Qwen does not consume the base64 image nested in the tool
  output reliably.
- Removing images from a function output must retain the output's array shape,
  including an empty array for an image-only result; it must not synthesize an
  empty string.
- No request history may be rewritten and no client session may be reset solely
  for this fix.

## Design

### Image normalization and incremental baseline

Add a model capability flag for providers that require images to be relocated out
of tool outputs. In the core Responses client, apply the relocation to the full
request before cloning the request used by the incremental tracker and before
calculating the incremental suffix. The Qwen model catalog enables the flag.

The relocation is idempotent: it removes image content from the tool output,
preserves the remaining content array exactly, and inserts one adjacent user
message containing the images. An image-only output remains an empty array. Qwen
policy normalization remains as a defensive boundary, filters images from the
array in place, and must not produce another image message when core normalization
already ran.

This keeps three representations identical:

1. the request used to calculate the incremental suffix;
2. the request saved as the next local baseline;
3. the item sequence sent to and stored by Qwen.

## Failure-first test strategy

No production implementation is changed until the corresponding regression test
has been observed failing for the expected reason.

### RED 1: image request shape

Run the new regression tests against a clean worktree at the current committed
baseline, not against the dirty primary worktree that already contains a proposed
image fix. A Qwen policy test asserts that removing the only image leaves
`"output":[]`; it must fail on the pre-fix policy with actual value
`"output":""`. A core test drives a Responses request through the real
incremental request path and a test transport that applies the same late Qwen
rewrite. It asserts that the provider-visible two-item input is identical to the
input retained by the incremental baseline. On the pre-fix baseline, this must
fail because the tracker retains one item while the wire request contains two.

After recording both expected failures, filter policy images from the output
array in place, then apply the minimal capability flag and pre-baseline relocation.
Re-run the same tests and require them to pass. Also test that text output is
preserved, multiple images keep their order, and relocation is idempotent.

## Verification

The implementation is acceptable only when all of the following evidence exists:

1. Both new regression tests were captured failing on the pre-fix implementation
   for the intended behavioral mismatch, not because of compilation or setup
   errors.
2. The same tests pass after the minimal fixes.
3. The affected core, protocol, model-manager, and private Qwen crate tests pass
   using repository test commands.
4. Formatting and scoped lint/fix checks complete without new warnings.
5. Captured request bodies show that a Qwen image suffix has the same item kinds in
   the tracker and on the wire.
6. The paid live semantic probe remains ignored by default and records that a tool-output image has
   zero image tokens while the adjacent user-image control is actually processed.

Existing production logs prove the provider cache behavior before and after the
image request-shape correction. A new paid live Qwen call is optional and requires
separate authorization; deterministic request-shape tests are mandatory regardless.

## Scope boundaries

- Do not remove image support or send base64 images as ordinary text.
- Do not refactor unrelated request, history, or Skills discovery code.
