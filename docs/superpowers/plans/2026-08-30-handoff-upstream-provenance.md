# Handoff: upstream stable tag provenance (2026-08-30)

- Read-only `git ls-remote` against `https://github.com/openai/codex.git`
  confirms annotated tag `rust-v0.151.0` points to peeled commit
  `78c290807ce710180111df227df3b7a4fe845452` (tag object
  `d8673cb68e349c208659b986697773d3145dbb14`).
- The local `upstream` remote matches the official OpenAI repository; `origin`
  remains the Catalyst fork. No fetch, merge, or history rewrite was performed.
- The stable target recorded in the runbook is therefore reproducible and
  externally verified.
