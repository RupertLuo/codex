# Handoff: Rendezvous TCP_NODELAY review (2026-08-30)

- Reviewed upstream `cfead68e5d`, which enables `disable_nagle` for Rendezvous
  websocket connections.
- Catalyst already routes both connection paths through `WebSocketConnector`
  with `.with_tcp_nodelay()`, while also preserving fork-specific headers and
  TLS factory injection. The upstream change is therefore a verified no-op.
- Conflicts were resolved by retaining the stronger existing connector path;
  no source or index changes remain from the trial.
