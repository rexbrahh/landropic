# Repository Guidelines

## Project Structure & Module Organization
- `landro-*/`: Rust crates for core modules (e.g., `landro-daemon`, `landro-cli`, `landro-quic`, `landro-cas`, `landro-index`, `landro-chunker`, `landro-crypto`).
- `src/`: Workspace-level glue and helpers.
- `tests/`: Integration tests (`.rs`) and test artifacts.
- `docs/`: User, developer, and architecture docs.
- `scripts/` and `*.sh`: CI/local utilities.
- Nix: `flake.nix`, `flake.lock`, `justfile` for reproducible builds.

## Build, Test, and Development Commands
- `nix develop`: Enter fully provisioned dev shell.
- `just build`: Nix build for all packages; `just build-pkg landropic` for a target.
- `just test`: Run test suite with `cargo nextest`.
- `just ci`: Format check, Clippy, tests, and `nix flake check`.
- `just fmt` / `just fmt-check`: Auto-format / verify formatting.
- `just clippy`: Lint workspace with warnings as errors.
- Cargo direct: `cargo build --release`, `cargo test`.

## Coding Style & Naming Conventions
- Language: Rust 2021, `rustc 1.75+`.
- Formatting: `cargo fmt` (4 spaces; soft 100 cols). Pre-commit enforces formatting.
- Linting: `cargo clippy --workspace --all-targets -D warnings`.
- Naming: modules/functions `snake_case`; types/traits `CamelCase`; consts `SCREAMING_SNAKE_CASE`; crates use `landro-*` prefix.
- Imports & errors: prefer explicit imports; libraries define errors with `thiserror`; binaries use `anyhow`.

## Testing Guidelines
- Frameworks: Rust `#[test]`; property tests via `proptest`; benches via `criterion`.
- Locations: Unit tests inline per crate; integration tests in `/tests`.
- Run: `just test` or `cargo test --profile test-fast`.
- Coverage: `just coverage` (HTML in `target/tarpaulin`).
- Naming: descriptive files (e.g., `integration_file_transfer.rs`); tests like `fn sync_resumes_on_restart()`.

## Commit & Pull Request Guidelines
- Commits: Conventional Commits (`feat:`, `fix:`, `chore:`, `docs:`, `refactor:`).
- Before pushing: `pre-commit install` once, then `just ci`; ensure no Clippy/format failures.
- PRs: clear description, link issue (e.g., `Fixes #123`), add/update tests, attach logs/screenshots for behavior changes, and update docs when needed.

## Security & Configuration Tips
- Never commit secrets/keys; `.envrc` is allowed but avoid secrets.
- Audit regularly with `just audit` (uses `cargo audit`/`deny`).
- Prefer `nix develop` and `just` tasks for reproducible dev.
