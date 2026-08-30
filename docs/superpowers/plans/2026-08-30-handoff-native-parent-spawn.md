# Handoff: native agents through parent control (2026-08-30)

- Adapted upstream `a92f3b7e61` to the current Catalyst agent-control API.
- Native requests now return a lightweight `NativeAgentSpawn` thread ID and
  route spawning through the parent session's `AgentControl`.
- `Op::UserInput` is converted to the current `spawn_agent_with_metadata`
  input shape; non-user operations are rejected explicitly. Existing parent
  depth, role, nickname, and bounded fork mode metadata are preserved.
- Upstream tests that depended on obsolete model-catalog/skill-provider runtime
  options were excluded; the parent-control behavior test remains.
- Formatting and conflict-marker checks pass. Cargo validation remains blocked
  by the full filesystem during pinned dependency checkout.
- Next: adapt `8167a15f90` to propagate `ExtensionDataInit` through current
  spawn/fork paths, retaining all Catalyst-specific spawn options.
