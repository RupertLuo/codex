# 2026-08-30 In-Process Raw RPC Handoff

- Added `InProcessClientSender::raw_request` and `InProcessClientHandle::raw_request` with a public `PendingClientRequestResponse` alias.
- Raw requests use the same pending-response routing and bounded queue behavior as typed requests, then enter `MessageProcessor::process_request` with an explicit `InProcess` transport context.
- This enables registered process-local RPC extensions without fabricating a `ClientRequest` enum variant. Typed request behavior is unchanged.
- A dedicated end-to-end in-process extension test is still required; current tracing tests cover the raw processor path, while Cargo execution remains blocked by disk exhaustion before compilation.
- Next action: add the in-process extension fixture/test, then run targeted app-server tests after reclaiming task-owned build space or provisioning a larger filesystem.
