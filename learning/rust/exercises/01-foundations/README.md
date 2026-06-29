<!-- Boundary: Defines hands-on tasks for Lesson 1; it does not provide production implementation guidance. -->
# Exercise 01: Ownership and `Result`

After installing Rust, run:

```bash
cargo run --manifest-path learning/rust/exercises/01-foundations/Cargo.toml
cargo test --manifest-path learning/rust/exercises/01-foundations/Cargo.toml
```

Then complete these experiments one at a time:

1. After `let response = send_request(request)?;`, add `println!("{:?}", request);`. Predict the compiler error before running it.
2. Change `send_request(request: ModelRequest)` to `send_request(request: &ModelRequest)`, update the call site, and make the program compile without cloning.
3. Add an optional `temperature: Option<f32>` field. Render `"default"` for `None` and the number for `Some(value)` using `match`.
4. Replace the string error with `enum RequestError { EmptyPrompt }`, then implement `std::fmt::Display` for it.

The goal is not merely to get green tests. For each compiler error, explain which scope owns the affected value and whether the function needs ownership or only a borrow.
