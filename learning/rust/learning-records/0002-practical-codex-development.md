<!-- Boundary: Records the learner's requested learning method; it does not claim mastery of the planned Rust concepts. -->
# Learning Record 0002: Practical Codex development

- Date: 2026-06-29
- Evidence: The learner requested to proceed by writing real Codex code step by step, including debugging and command-line execution.
- Teaching consequence: Each lesson will use a tight red/green feedback loop, explain only the Rust syntax required by the current patch, and include exact build, test, tracing, and debugger commands.
- Current challenge: Understand RPITIT, boxed futures, `Arc`, and type erasure well enough to inject an HTTP transport without replacing the native Codex agent loop.
- Mission link: This is the first public-fork patch required before implementing the private multi-provider protocol runtime.
- Updated evidence: The learner found an architecture-first explanation too obscure without prior Rust syntax knowledge.
- Updated teaching consequence: Begin with a visible edit in the real Codex CLI. Introduce at most one small cluster of syntax per action, and keep advanced architecture in optional reference sections.
