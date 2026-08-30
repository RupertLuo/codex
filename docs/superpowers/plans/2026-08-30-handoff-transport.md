# Transport patch migration handoff (2026-08-30)

## Scope

Evaluated Catalyst commits `002e7d3643` and `31e648951a` against the
`sync/20260830-rust-v0.151.0` tip (`26d79fc0ca`). No large test suite was run.

## Result

The type-erased transport layer has now been ported as an isolated semantic
step into `codex-http-client`:

* `HttpTransportHandle` lives beside the stable `HttpTransport` trait and is
  re-exported from `codex-http-client`.
* It supports cloneable callback-based construction and
  `from_transport` erasure, with focused execute/stream delegation tests.
* No `ModelClient`, session, or thread-manager plumbing was added yet; those
  APIs still require a coordinated migration that preserves route-aware
  client construction and websocket fallback behavior.

The new module was formatted directly. The repository-wide `just fmt` command
ran Rust formatting but also reported pre-existing parse failures in
`core/src/session/turn_context.rs` and `models-manager/src/manager.rs`; the
optional Python/Bazel formatters are unavailable (`uv`/`dotslash`).

The two commits were not cherry-picked. A `git cherry-pick --no-commit
002e7d3643` probe was aborted after a conflict in `codex-rs/codex-client/src/lib.rs`.
The conflict is structural, not a local typo:

* The Catalyst patch assumes `codex-client` owns `transport.rs`,
  `HttpTransport`, `ReqwestTransport`, `StreamResponse`, and `HttpTransportHandle`.
* The stable target has already extracted those types into the separate
  `codex-http-client` crate (`codex-rs/http-client/src/transport.rs`).
  `codex-client` now re-exports `codex_http_client::*` and no longer has a
  transport module.
* Consequently, the second patch's `codex_api::HttpTransportHandle` import,
  `ModelClient` override, and `Session`/`ThreadManager` plumbing cannot be
  applied mechanically either. Its constructor also targets an older
  `Session::new`/`ThreadManager` shape.

The probe left no changes in the worktree. The current branch already contains
the old fork's `client_tests.rs` transport-injection test references, but the
corresponding handle implementation is absent; this is an existing source/test
baseline mismatch and should not be papered over by cherry-picking these old
commits.

## Recommended migration shape

Port the feature as a fresh semantic group after the transport API is reviewed:

1. Add the type-erased handle beside the stable `HttpTransport` trait in
   `codex-rs/http-client`, with tests in that crate. Re-export it from
   `codex-api` only if the public core boundary needs it.
2. Thread an optional handle through the *current* `ModelClient` and session
   construction APIs, preserving route-aware `ReqwestTransport` creation and
   websocket/fallback behavior. Do not copy the old `build_reqwest_client`
   assumptions.
3. Update the current test support and integration test to exercise the public
   current session/thread startup path. Compile the smallest affected crates
   before proceeding.

This should be treated as a medium-risk transport/runtime migration, not a
low-risk cherry-pick. It is deferred until the repository has enough disk for
Cargo compilation and the current compaction boundary has been validated.

## Verification

`git status --short` is clean after aborting the probe. No source or dependency
files were modified by this evaluation; only this handoff document records the
result.

## Exact next action

When capacity is available, inspect `codex-rs/http-client`'s public trait and
the current `ModelClient` construction path, then implement the handle there
as a new, reviewable commit. Run `just fmt` and the targeted `codex-http-client`
/`codex-core` tests before taking the next transport patch group.
