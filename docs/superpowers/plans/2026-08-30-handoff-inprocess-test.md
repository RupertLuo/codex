# 2026-08-30 In-Process RPC Test Handoff

- Added an end-to-end in-process fixture with a trusted `test/echo` extension. The test starts `InProcessClientHandle` with `InProcessStartArgs.rpc_extensions`, calls the public raw request API, and asserts both params and `InProcess` transport context.
- `just test -p codex-app-server` was attempted after the change but stopped before compilation because Cargo could not fetch the pinned crossterm revision with only ~14 MB free (`No space left on device`). The failed crossterm checkout was removed; no test binary ran.
- Direct rustfmt and `git diff --check` passed.
- Next action: provision additional task-owned disk/build space, rerun the targeted app-server suite, then continue the next upstream patch group.
