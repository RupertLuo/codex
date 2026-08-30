# Handoff: websocket trace removal review (2026-08-30)

- Reviewed upstream `db887d03e1`, which removes full request text from
  websocket tracing.
- Catalyst already omits that trace in
  `codex-api/src/endpoint/responses_websocket.rs`; the patch is a verified
  no-op and no source/index changes remain.
- This preserves the existing privacy/log-volume invariant without another
  migration commit. Continue by checking the next transport patch for overlap.
