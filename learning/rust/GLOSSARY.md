<!-- Boundary: Records Rust terms the learner has demonstrated; it must not claim unverified mastery. -->
# Rust Learning Glossary

Terms are added here only after the learner has explained or used them correctly in an exercise.

## Demonstrated terms

**Clap**:
A Rust command-line parser that converts argument strings into typed Rust data and can generate command help from declarations such as documentation comments.
_Avoid_: Command executor, Rust compiler

**`Result<T, E>`**:
A Rust type that represents either success as `Ok(T)` or failure as `Err(E)`.
_Avoid_: Exception, nullable return value

**Module declaration (`mod`)**:
A Rust declaration that adds a module to the module tree; `mod tests;` makes the compiler load the corresponding test module instead of discovering the file automatically.
_Avoid_: Import, automatic test discovery

**`#[derive(Clone)]`**:
An attribute that asks the Rust compiler to generate the standard `Clone` implementation, making explicit duplication available through `.clone()`.
_Avoid_: Automatic copying, constructor

**`Arc<T>`**:
An atomically reference-counted pointer that gives multiple threads shared ownership of one value; cloning an `Arc` shares the value rather than duplicating it.
_Avoid_: Deep copy, global variable
