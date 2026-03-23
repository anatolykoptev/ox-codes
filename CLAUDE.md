# ox-codes — Rust Code Search Backend

**Port**: 8902 | **Rust** 1.93 edition 2024 | Docker container

Internal search backend for go-code. No MCP — HTTP only.

## Crates

| Crate | Role |
|-------|------|
| `crates/core` | Search engine: grep + scoped + structural |
| `crates/langs` | Tree-sitter language scopes (Go, Rust, Python, TS, Java) |
| `crates/server` | Axum HTTP handlers |

## API

- `POST /search` — grep-like search (ripgrep crates)
- `POST /search/scoped` — regex within AST regions (tree-sitter)
- `POST /search/structural` — pattern matching with $WILDCARDS (ast-grep)
- `GET /health` — healthcheck

## Build

```bash
make build    # cargo build --workspace
make test     # cargo test --workspace
make lint     # cargo clippy -- -D warnings
make check    # fmt + lint + test
```

## Deploy

```bash
cd ~/deploy/krolik-server
docker compose build --no-cache ox-codes && docker compose up -d --no-deps --force-recreate ox-codes
```
