# Mandatory host instructions handoff (2026-08-30)

Catalyst's required base-instructions behavior was migrated as commit
`f953666576`. `ThreadManagerRuntimeOptions` now accepts a required instruction
string; every newly spawned thread prefixes it once to requested base
instructions. Guardian/review task prompts compose on top without dropping the
host requirement. `TestCodexBuilder` can configure the same runtime option.

Focused tests cover option visibility and idempotent prefix composition. Direct
rustfmt and `git diff --check` pass. Full compilation/tests remain pending a
larger build filesystem; no API schema or wire payload changed.

Next action: run the core integration suite with a real runtime option and
verify resumed/child sessions preserve the required instruction in their
effective base configuration.
