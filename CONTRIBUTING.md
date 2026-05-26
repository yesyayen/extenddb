# Contributing to ExtendDB

Thank you for your interest in contributing to ExtendDB.

## Getting Started

### Prerequisites

- Rust 1.85+ (stable)
- PostgreSQL 14+
- Python 3.10+ (for integration tests)

### Build

```bash
cargo build --workspace
```

### Run Tests

```bash
# Rust unit tests
cargo test --workspace

# Integration tests (requires a running extenddb server)
pytest tests/
```

### Code Style

All contributions must pass:

```bash
cargo fmt --check
cargo clippy -- -W clippy::pedantic -W clippy::unwrap_used -W clippy::expect_used
```

- Use `cargo fmt` before committing.
- Address all clippy warnings or add `#[allow(...)]` with a justification comment.
- Avoid `.unwrap()` and `.expect()` in library crates — use `?` or explicit error handling.

### Submitting Changes

1. Fork the repository and create a branch from `main`.
2. Make your changes with clear, focused commits.
3. Ensure all tests pass and code is formatted.
4. Open a pull request with a description of what changed and why.

## When you need an ADR or an RFC

ExtendDB uses two lightweight processes to keep core decisions reviewable:

- **ADR** ([`docs/adr/`](docs/adr/)) — records a decision *after* it has been
  made, in four sections: Context, Options Considered, Decision, Consequences.
  Use for narrower, internal calls (for example, "we chose `ring` over
  `openssl`" or "ADRs live in `docs/adr/`").
- **RFC** ([`docs/rfcs/`](docs/rfcs/)) — proposes a change *before* it is made and
  invites a comment period. Use for changes to the wire protocol, the
  `Storage` trait, the auth model, on-disk format, or the public CLI surface.
  See [`docs/rfcs/README.md`](docs/rfcs/README.md) for the lifecycle.

If you are not sure which to write, open an issue describing the change. A
maintainer will tell you which path fits.

Some areas are protected by [`.github/CODEOWNERS`](.github/CODEOWNERS) — PRs
touching them require approval from the listed owners.

## License

By contributing, you agree that your contributions will be licensed under the
[Apache License 2.0](LICENSE).
