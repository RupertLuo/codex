# Handoff: Rendezvous websocket liveness deferred (2026-08-30)

- Reviewed upstream `042e61726d`, which adds a pong watchdog, keepalive
  scheduling, relay harness changes, and roughly 950 changed lines.
- Catalyst's exec-server already has a custom `WebSocketConnector`, rendezvous
  headers, and relay lifecycle handling. The patch touches high-risk transport
  loops and exceeds the repository's 800-line change-size guidance.
- It is intentionally deferred into smaller behavior stages: watchdog module,
  relay integration, remote integration, then dedicated harness tests. Do not
  cherry-pick wholesale.
