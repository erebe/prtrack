# Repository Guidelines

## Project Structure & Module Organization

`ptrack` is a Rust 2024 binary crate. `src/main.rs` contains the entry point and currently prints `Hello, world!`. `Cargo.toml` defines package metadata and dependencies; no dependencies are declared yet. Keep `Cargo.lock` versioned for reproducible binary builds. Cargo writes build artifacts to `target/`, which `.gitignore` excludes.

Add implementation modules under `src/` as functionality grows. Place unit tests beside the code they exercise and integration tests in `tests/` when needed. No dedicated test or asset directories currently exist.

## Build, Test, and Development Commands

Use a Rust toolchain supporting edition 2024. Run commands from the repository root:

- `cargo build`: compile the debug binary.
- `cargo run`: build and run `ptrack` locally.
- `cargo build --release`: create an optimized binary in `target/release/`.
- `cargo test`: run unit, integration, and applicable documentation tests.
- `cargo fmt --check`: check formatting; use `cargo fmt` to apply it.
- `cargo clippy --all-targets -- -D warnings`: check all targets and treat lint warnings as errors.

## Coding Style & Naming Conventions

Follow standard Rust formatting with four-space indentation and `rustfmt`. Use `snake_case` for modules, functions, and variables; `PascalCase` for types and traits; and `SCREAMING_SNAKE_CASE` for constants. Keep functions focused and add dependencies only when justified. No custom formatting or lint configuration is present.

## Testing Guidelines

Use Rust's built-in test framework: `#[test]` functions within `#[cfg(test)] mod tests` for unit tests. Choose behavior-oriented names such as `rejects_invalid_input`. Cover expected behavior, edge cases, and failures when adding functionality. Run one matching test with `cargo test rejects_invalid_input`. There are currently no tests or configured coverage thresholds.

## Commit & Pull Request Guidelines

The repository has no commits yet, so no historical commit convention is established. Use short, imperative subjects, such as `Add argument parsing`, and keep commits focused.

Pull requests should explain the problem, summarize behavior changes, link relevant issues, and report validation commands and results. Include sample terminal output for CLI behavior changes. Run formatting, lint, and test checks before requesting review.

## Agent-Specific Instructions

Prefer codebase-memory-mcp for code discovery: `search_graph`, `trace_path`, `get_code_snippet`, `query_graph`, then `get_architecture`. Index the repository if needed. Fall back to text search for literals, configuration, non-code files, or insufficient graph results.
