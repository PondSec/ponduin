# Developing Ponduin

Ponduin is maintained by PondSec. Changes are accepted through the internal
review and release process.

## Development workflow

1. Create a focused branch from `dev`.
2. Keep changes limited to one clear purpose.
3. Add or update tests for changed behavior.
4. Run formatting, linting, builds and relevant test suites.
5. Open a review against `dev`.
6. Address review feedback before integration.

`main` is reserved for tested stable releases.

## Local setup

Install the toolchain and activate the repository environment:

```bash
source bin/activate-hermit
```

Build the Rust workspace:

```bash
cargo build
```

Run the Rust test suite:

```bash
cargo test
```

Validate formatting and lints:

```bash
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
```

Install UI dependencies and validate TypeScript:

```bash
cd ui
pnpm install
cd desktop
pnpm run typecheck
pnpm test
```

## Engineering standards

- Preserve existing behavior unless a change explicitly requires otherwise.
- Keep platform behavior consistent across macOS, Linux and Windows.
- Avoid logging secrets, user prompts or private file contents.
- Maintain clear permission boundaries for tools and extensions.
- Update user-facing documentation with product changes.
- Keep package, CLI and UI terminology consistently branded as Ponduin.

## Legal

Do not remove or alter applicable copyright, attribution or third-party license
notices. New dependencies require a license and security review.
