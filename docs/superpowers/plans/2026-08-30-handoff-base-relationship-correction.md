# Handoff: upstream base relationship correction (2026-08-30)

- Authoritative git evidence:
  - `git merge-base HEAD rust-v0.151.0` =
    `78c290807ce710180111df227df3b7a4fe845452`.
  - `git merge-base --is-ancestor rust-v0.151.0 HEAD` succeeds.
  - `git rev-list --count rust-v0.151.0..HEAD` reports the Catalyst/doc/test
    delta only.
- Therefore the current branch already descends directly from the official
  stable release. The upstream commits previously described as “deferred” (for
  example communication telemetry and websocket liveness) are already present
  in the stable base history; the remaining task is compatibility/delta audit,
  not replaying those commits.
- Earlier deferred handoffs remain historical records of trial analysis, but
  this correction is the authoritative relationship for future agents.
- Completion still requires targeted tests and infrastructure recovery; the
  root filesystem remains nearly full.
