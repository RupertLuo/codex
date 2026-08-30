# Automatic compaction enable switch handoff (2026-08-30)

## Result

Catalyst commit `b7b541398e` was migrated onto the stable
`rust-v0.151.0` API as commit `feat(core): add automatic compaction enable
switch`. The migration was low-risk but required manual conflict resolution
because the current branch has newer context-window/token-budget logic.

The new `model_auto_compact_enabled` setting is optional in `ConfigToml`,
defaults to `true` in the effective core `Config`, is exposed by app-server v2
config read/write payloads, and participates in the guardian review-session
reuse key. When false, proactive auto-compaction thresholds are disabled while
the model's full context-window safety boundary remains active. The previous
branch's buffered token-budget and body-after-prefix calculations were kept.

The upstream session tests were not copied verbatim: they reference the older
`tokens_until_compaction` status field and obsolete `TurnContext` helpers.
Equivalent config default/explicit-false tests and schema coverage were added;
behavioral session coverage should be added after a compile-capable environment
is available.

## Verification and blocker

Rust formatting was run with the task Python 3.11 environment. The Rust
formatter completed; repository `just fmt` still reports unavailable optional
`uv`/`dotslash` tools. `git diff --check` passes. No Rust test or full compile
was run because the host filesystem remains capacity-constrained.

## Exact next action

With sufficient build capacity, run targeted config/core and app-server config
tests, then add a session-level assertion that disabled proactive compaction
still trips the full context-window boundary. Regenerate schemas with the
repository recipes if generated output differs.
