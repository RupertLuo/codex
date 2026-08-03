# Qwen previous-response cache-zero fix

## Problem statement

Two independent request-shape changes are causing Qwen requests that use
`previous_response_id` to lose their cached prefix.

1. Tool output images are rewritten from one `function_call_output` item into a
   `function_call_output` followed by a user image message. The rewrite currently
   happens after the incremental request baseline has been captured. The local
   baseline therefore describes one item while the provider stores two.
2. The Skills extension emits the complete turn-level skills catalog as a new
   developer message on every user turn, even when the catalog is unchanged.
   Qwen treats this repeated developer item as a changed continuation and the
   first request of the new turn reports zero cached input tokens.

Production logs provide an A/B for the image path. In the failing process the
incremental tracker recorded one item while the Qwen wire request contained two;
all later requests in that image sequence missed the cache. After the image
rewrite was moved before baseline capture, both tracker and wire request contained
`["function_call_output", "message"]`, and image requests plus their successors
continued to report cached input tokens. Other threads also cache image requests,
so the user image role itself is not the root cause.

## Required behavior

- Every provider-visible structural rewrite must happen before the request is
  diffed against, or stored as, an incremental baseline.
- A tool output image must remain visible to Qwen as an adjacent user image
  message because Qwen does not consume the base64 image nested in the tool
  output reliably.
- An unchanged turn-level skills catalog must not be appended again on a later
  user turn in the same thread.
- A changed turn-level skills catalog must be emitted once, then suppressed again
  until it changes.
- Explicitly selected skill instructions remain per-turn input and must never be
  suppressed by catalog deduplication.
- No request history may be rewritten and no client session may be reset solely
  for either fix.

## Design

### Image normalization and incremental baseline

Add a model capability flag for providers that require images to be relocated out
of tool outputs. In the core Responses client, apply the relocation to the full
request before cloning the request used by the incremental tracker and before
calculating the incremental suffix. The Qwen model catalog enables the flag.

The relocation is idempotent: it empties image content from the tool output,
preserves any text output, and inserts one adjacent user message containing the
images. Qwen policy normalization may still run as a defensive boundary, but it
must not produce another image message when core normalization already ran.

This keeps three representations identical:

1. the request used to calculate the incremental suffix;
2. the request saved as the next local baseline;
3. the item sequence sent to and stored by Qwen.

### Skills catalog deduplication

Extend `SkillsThreadState` with a mutex-protected snapshot of the last rendered
turn-level catalog. The TurnInput contributor renders the bounded catalog exactly
as it does today, then performs one atomic compare-and-update operation:

- no previous snapshot or different rendered content: save it and emit the
  developer fragment;
- identical rendered content: emit no catalog fragment.

Only the turn-level catalog participates in this state. The one-time thread
context catalog has different provider coverage, so it does not seed or overwrite
the turn snapshot. Explicit skill instructions are generated after catalog
deduplication and remain unaffected.

Comparing the rendered content rather than only a hash avoids collision behavior
and makes changes in descriptions, locators, ordering, budget truncation, or usage
instructions observable. The catalog is already bounded, so retaining one copy is
bounded per thread.

## Failure-first test strategy

No production implementation is changed until the corresponding regression test
has been observed failing for the expected reason.

### RED 1: image request shape

Run the new regression test against a clean worktree at the current committed
baseline, not against the dirty primary worktree that already contains a proposed
image fix. The test drives consecutive Responses requests through the real
incremental request path and a Qwen-style tool-output image rewrite. It asserts
that the provider-visible two-item suffix is also the suffix retained by the
incremental baseline. On the pre-fix baseline, the assertion must fail because the
tracker retains one item while the wire request contains two.

After recording the expected failure, apply the minimal capability flag and
pre-baseline relocation. Re-run the same test and require it to pass. Also test
that text output is preserved, multiple images keep their order, and relocation is
idempotent.

### RED 2: repeated skills catalog

Use the Skills extension through its contributor interfaces for two consecutive
turns with the same catalog. Assert that the first turn emits one developer catalog
and the second emits none. On the current implementation this must fail because
both turns emit the complete catalog.

After recording the expected failure, add the thread-level compare-and-update
state and re-run. Add separate tests proving that a changed host catalog is emitted
once and that explicit skill instructions are still emitted when the catalog is
suppressed.

## Verification

The implementation is acceptable only when all of the following evidence exists:

1. Both new regression tests were captured failing on the pre-fix implementation
   for the intended behavioral mismatch, not because of compilation or setup
   errors.
2. The same tests pass after the minimal fixes.
3. The affected core, protocol, model-manager, Skills extension, and private Qwen
   crate tests pass using repository `just test` commands.
4. Formatting and scoped lint/fix checks complete without new warnings.
5. Captured request bodies show that a Qwen image suffix has the same item kinds in
   the tracker and on the wire.
6. A two-turn request capture shows no repeated full `<skills_instructions>` item
   when the catalog is unchanged, while a changed catalog appears exactly once.

Existing production logs prove the provider cache behavior before and after the
image request-shape correction. A new paid live Qwen call is optional and requires
separate authorization; deterministic request-shape tests are mandatory regardless.

## Scope boundaries

- Do not remove image support or send base64 images as ordinary text.
- Do not make the deduplication Qwen-specific; repeated stable context is a core
  model-context defect regardless of provider.
- Do not suppress selected skill bodies or executor world-state updates.
- Do not refactor unrelated request, history, or Skills discovery code.
