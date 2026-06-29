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
