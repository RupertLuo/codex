# Thread source listing audit (2026-08-30)

Catalyst commit `604be1c5b9` was manually migrated as `569f4418bf`.
`rollout::ThreadItem` and `HeadTailSummary` now carry `ThreadSource`, the first
session metadata line is parsed into that field, and recorder/local-store
conversion preserves it. App-server thread summary conversion already mapped
the value and was retained.

The fixture-level app-server thread-list assertion remains pending until the
targeted suite can compile. Full test execution remains pending the disk gate.
