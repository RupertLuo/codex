# Thread source listing audit (2026-08-30)

Catalyst commit `604be1c5b9` was audited against the current stable target.
Its intended behavior is already present in the working tree: `rollout::ThreadItem`
and `HeadTailSummary` carry `ThreadSource`, the first session metadata line is
parsed into that field, and recorder/state conversion preserves it. App-server
thread summary conversion also already maps the value.

No duplicate patch was applied. The remaining upstream test fixture that edits
a rollout file can be reintroduced only if the current app-server thread-list
suite lacks equivalent coverage. Full test execution remains pending the disk
capacity gate.
