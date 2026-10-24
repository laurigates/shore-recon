# CLAUDE.md

Rust workspace for shoreline-recon. Design source of truth: `docs/kickoff.md`.

- TDD: add or update unit tests with every change; keep tests next to code (`#[cfg(test)]`).
- Integration tests needing external tools or a GPU go in modules named `integration` so the default nextest profile excludes them.
- Before committing: `cargo fmt --all`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo nextest run` (or `cargo test`).
- Shared dependencies are declared in `[workspace.dependencies]`; crates use `dep.workspace = true`.
- External tools run only through the `ToolRunner` in `core` (`shoreline_core::tools`, ADR-0003); stages do not spawn processes directly.
- Backend and format decisions get an ADR in `docs/adr/`.
