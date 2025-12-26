# Repository Guidelines

## Project Structure & Module Organization
- `backend/`: Axum API service and markdown handling (see `backend/src/`).
- `frontend/`: Yew WASM UI, pages and components in `frontend/src/`.
- `shared/`: shared Rust types for API contracts.
- `content/`: sample Markdown posts and images (`content/images/`).
- `docs/` and `deployment-examples/`: reference material and deployment notes.
- Root tooling: `Cargo.toml`, `Makefile`, `rustfmt.toml`.

## Build, Test, and Development Commands
- `make install`: install Rust target, Trunk, and frontend npm deps.
- `make dev`: start backend (`:3000`) and frontend (`:8080`) together.
- `make dev-backend` / `make dev-frontend`: run each service separately.
- `cargo build --workspace`: build all crates; `trunk build --release` for frontend prod build.
- `cargo test --workspace`: run workspace tests (currently minimal).
- `cargo fmt --all` and `cargo clippy --workspace -- -D warnings`: format and lint.
- `make ci`: run fmt, lint, test, and check in one pass.

## Coding Style & Naming Conventions
- Rust 2021 edition; format with `cargo fmt` (see `rustfmt.toml`).
- Indentation: 4 spaces, max line width 100, LF line endings.
- Imports are grouped std → external crates → local modules.
- Use Rust conventions: `snake_case` for functions/modules, `CamelCase` for types.

## Testing Guidelines
- No dedicated test suite yet; `cargo test --workspace` should stay green.
- When adding tests, prefer `#[cfg(test)] mod tests` near the code under test.
- Name tests descriptively (e.g., `parses_markdown_frontmatter`).

## Commit & Pull Request Guidelines
- Follow Conventional Commits as seen in history: `feat:`, `fix:`, `docs:`, etc.
- Keep subjects short and imperative (e.g., `feat: add tag filtering`).
- PRs should include a brief summary, testing notes, and screenshots for UI changes.
- Link related issues when available.

## Automation & Agent Notes
- This project favors “build skills, not agents.”
- If using Codex/Claude workflows, follow `CLAUDE.md` for project-specific guidance.
