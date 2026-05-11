# Contributing to ox-codes

Thanks for your interest! Quick guide:

## Development setup

```sh
git clone https://github.com/anatolykoptev/ox-codes
cd ox-codes
make build   # cargo build --workspace
make test    # cargo nextest run --workspace
```

Requires Rust 1.93+ and `cargo-nextest` (install: `cargo install cargo-nextest`).

## Submitting changes

1. Fork the repo + create feature branch (`feat/X` or `fix/X`)
2. Make changes + add tests
3. Verify locally: `make test && cargo deny check`
4. Commit with [Conventional Commits](https://www.conventionalcommits.org) format (`feat:`, `fix:`, `chore:`, etc.)
5. Open PR against `master`

## Code style

- Rust 2024 edition
- `cargo fmt` before commit
- `cargo clippy --all-targets -- -D warnings` clean
- No `unwrap` / `expect` in request handlers (except startup)

## License

By contributing, you agree your contributions are licensed under MIT.
