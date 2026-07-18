# ox-codes — Rust Code Search Backend

**Port**: 8902 | **Rust** 1.93 edition 2024 | Docker container

Internal search backend for go-code. No MCP — HTTP only.

## Crates

| Crate | Role |
|-------|------|
| `crates/core` | Search engine: grep + scoped + structural + rewrite + expand |
| `crates/langs` | Tree-sitter language scopes (15 languages) |
| `crates/server` | Axum HTTP handlers |

## API

- `POST /search` — grep-like search (ripgrep crates)
- `POST /search/scoped` — regex within AST regions (tree-sitter)
- `POST /search/structural` — pattern matching with $WILDCARDS (ast-grep)
- `POST /rewrite` — structural search + transform with diff output (ast-grep)
- `GET /health` — healthcheck

All search endpoints accept optional `expand` (`"none"`/`"function"`/`"block"`) and `max_tokens` params for returning full enclosing AST blocks instead of single lines.

## Build

```bash
make build    # cargo build --workspace
make test     # cargo test --workspace
make lint     # cargo clippy -- -D warnings
make check    # fmt + lint + test
```

## Deploy

```bash
cd ~/deploy/server-config
docker compose build --no-cache ox-codes && docker compose up -d --no-deps --force-recreate ox-codes
```
