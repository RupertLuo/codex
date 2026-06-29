<!-- Boundary: Curates authoritative Rust learning references for this mission; it does not mirror external documentation. -->
# Rust Learning Resources

## Core references

- [The Rust Programming Language](https://doc.rust-lang.org/book/) — the canonical book. Read selectively alongside the local lessons rather than front-to-back.
- [Understanding Ownership](https://doc.rust-lang.org/book/ch04-00-understanding-ownership.html) — the most important chapter for moving from Python to Rust.
- [Enums and Pattern Matching](https://doc.rust-lang.org/book/ch06-00-enums.html) — foundation for `Option`, `Result`, protocol events, and state machines.
- [Defining Shared Behavior with Traits](https://doc.rust-lang.org/book/ch10-02-traits.html) — foundation for Codex extension seams such as an HTTP transport abstraction.
- [Async and Await](https://doc.rust-lang.org/book/ch17-00-async-await.html) — foundation for following Codex model streams and the agent loop.
- [Impl trait type](https://doc.rust-lang.org/reference/types/impl-trait.html) — authoritative reference for the RPITIT syntax used by Codex 0.142.4's `HttpTransport`.
- [Dyn compatibility](https://doc.rust-lang.org/reference/items/traits.html#dyn-compatibility) — explains why a trait returning `impl Future` cannot be used directly as `dyn HttpTransport`.
- [Rust By Example](https://doc.rust-lang.org/stable/rust-by-example/) — concise executable examples when syntax needs a quick lookup.
- [Rustlings](https://github.com/rust-lang/rustlings) — small compiler-guided exercises after the first local lesson.
- [Codex 0.142.4 `HttpTransport`](https://github.com/openai/codex/blob/rust-v0.142.4/codex-rs/codex-client/src/transport.rs) — the exact upstream trait and default Reqwest implementation used in the transport lab.
- [Codex build and tracing guide](https://github.com/openai/codex/blob/rust-v0.142.4/docs/install.md) — official source-build, `just`, nextest, and `RUST_LOG` commands.

## How to use these resources

Use the local explainer as the learning path. Open the official book when a concept needs a second explanation, and use Rust By Example as a syntax reference. The compiler is part of the learning loop: after installing Rust, run each exercise and read the entire diagnostic before editing.
