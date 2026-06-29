<!-- Boundary: Defines the Rust learning mission for reading and modifying Codex; it does not define product architecture. -->
# Learning Mission: Rust for Codex Runtime Development

## Mission

Learn enough practical Rust to confidently read, debug, and modify the Codex agent turn loop, then implement a maintainable model transport and patch boundary without depending on Moon Bridge.

## Why this matters

The immediate product direction is moving Catalyst runtime responsibilities toward Rust and integrating more directly with Codex. The learner already writes Python, but is new to Rust, so the course should reuse Python intuition while explicitly correcting the places where Rust has a different execution and memory model.

## Success criteria

- Read ordinary Rust functions, structs, enums, `impl` blocks, modules, and macros without translating every line into Python.
- Predict when a value is moved, borrowed immutably, or borrowed mutably.
- Use `Option`, `Result`, `match`, and `?` for normal control flow and error propagation.
- Explain why Codex 0.142.4's RPITIT `HttpTransport` is not directly `dyn` compatible and implement a safe type-erased handle.
- Follow async call chains involving `async fn`, `.await`, futures, streams, Tokio tasks, and channels.
- Locate a stable patch seam in Codex and implement a small transport substitution with tests.

## Current focus

Lesson 2 begins with one disposable edit to the real Codex CLI. The learner first runs a command, changes one documentation comment, rebuilds, observes the result, and restores the change. Rust syntax is explained only after it appears in that loop. The production transport patch starts immediately afterward.
