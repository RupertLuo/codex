# Handoff: remaining stable patch inventory (2026-08-30)

The target remains `rust-v0.151.0` (`78c290807ce710180111df227df3b7a4fe845452`).
After the native-agent lifecycle chain, the fetched upstream history contains
many commits that overlap Catalyst customizations. The next rounds should use
this order rather than a wholesale cherry-pick:

1. Low-risk docs/wording and security-only fixes.
2. Transport/runtime fixes (`cfead68e5d`, `db887d03e1`, `042e61726d`,
   `b35d4b6b9d`, `9acfe8965d`, `641aa1b619`, `6afcf26d5d`) with HTTP transport
   ownership and proxy behavior checked against Catalyst's process-local handle.
3. Multi-agent telemetry and canonical item migrations (`a98a21798c`,
   `129ea2aaf5`, `1bd9d841ca`, `cca16a1087`, `f659eb12bc`, `058d97c5dc`) only
   after current `Op`/event compatibility is mapped.
4. Skills/plugins and app-server API changes (`d206a5d68f`, `9c5be7e1d5`,
   `42156ba007`, `c71895f63b`, `f1affbac5e`) with process-local/global-config
   boundaries preserved.
5. Compaction, rollout, and persistence changes last (`1f710c973b`,
   `2342b2c2a6`, `f17a57b7d5`) with transaction invariants and replay tests.

The first-class reasoning `max` patch (`80f54d1266`) is a separate deferred
round because Catalyst already has custom Ultra fallback and persistent effort
semantics. Every selected commit gets its own safety tag, targeted tests, and
dated handoff before the next selection.

Current validation gate: `cargo fmt --all -- --check` reports pre-existing
workspace drift in unrelated files; native-agent files are individually
formatted. `just test -p codex-core` reaches Cargo dependency unpack but fails
with `No space left on device`. Restore disk capacity and install `dotslash`/`uv`
before attempting broad validation.
