# Stable provider package handles — audit handoff (2026-08-30)

## Scope

Audited Catalyst commits `5793b0dad6` (`feat(skills): support stable provider
package handles`) and `eaf21f5a50` (`fix(skills): enforce safe provider routing
bounds`) against the current `rust-v0.151.0`-based synchronization branch.

## Findings

- The target already contains the newer, budget-aware skills tool pipeline
  (`SkillToolAuthoritySelector`, pagination cursors, response byte budgets,
  invocation-scoped context, and executor read snapshots). The old patch's
  `external_json_output` list/read shape and single-turn authority lookup are
  therefore not mechanically applicable.
- `5793b0dad6` introduces `SkillPackageDependency` metadata, bounded dependency
  projection, and coded provider errors. The current catalog has no dependency
  field and `SkillProviderError` has no public/internal message split; adding
  only one side would either expose incomplete metadata or preserve unsafe
  provider diagnostics. Its 320-line integration fixture also needs adaptation
  to the current `ToolCall<'call>` and pagination APIs.
- `eaf21f5a50` is a security boundary, not a cosmetic follow-up. Its
  orchestrator authorization cache must be keyed by the stable
  `McpResourceClientCacheKey`, expire dead connection generations, and authorize
  both package and resource before transport access. Applying only
  `is_alive()` or only the read-side check would leave an incomplete guarantee.
- The current branch already has `McpResourceClient::cache_key()` and
  invocation-scoped skill state, but does not have the authorization table from
  `eaf21f5a50`. That table must be reconciled with current state/cache lifetime
  semantics and tested with a real discovery/read sequence.
- Later upstream commits (`dbbd0d59ec`, `6b61d98fe2`) supersede parts of the
  old patch: unsafe dependencies are omitted as a whole entry and selected
  resource access handles are injected into skill instructions. These commits
  are useful references, but are not ancestors of this sync branch and should
  not be cherry-picked without checking their current API prerequisites.

## Decision

Defer both Catalyst commits as a single medium-risk skills round. No production
code was changed in this audit. A safe migration is larger than the requested
small patch boundary once the current pagination, response-budget, and
invocation-lifetime APIs plus integration coverage are included.

## Exact next action

When capacity permits Rust compilation, implement one coherent skills round:

1. Add package dependency metadata and provider error public/internal fields.
2. Project dependencies through the current budgeted list response, omitting
   the parent when any dependency is not tool-addressable and bounding both
   per-entry and total dependency counts.
3. Sanitize and bound coded provider errors before returning them to the model.
4. Add orchestrator authorization keyed by `McpResourceClientCacheKey`, remove
   dead generations, and test unlisted package rejection before transport.
5. Add/adapt integration tests for cross-turn list→read, invalid dependencies,
   truncation, and provider error sanitization; then run
   `just test -p codex-skills-extension`.

Do not expose package handles through a global mutable cache, and do not route a
handle to a different authority merely because the package string matches.
