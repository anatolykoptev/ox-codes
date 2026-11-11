# ox-codes — Rust Code Search Backend

**Port**: 8902 | **Rust** 1.97 edition 2024 | Docker container

Internal search backend for go-code. No MCP — HTTP only. Public repository: no operator-specific data (hosts, addresses, home paths) in tracked files. All documents: [docs/INDEX.md](docs/INDEX.md).

## Crates

| Crate | Role |
|-------|------|
| `crates/core` | Search engine: grep + scoped + structural + rewrite + expand |
| `crates/langs` | Tree-sitter language scopes (15 languages) |
| `crates/dataflow` | IL builder, CFG, def-use chains, taint engine |
| `crates/server` | Axum HTTP handlers |

## API

- `POST /search` — grep-like search (ripgrep crates)
- `POST /search/scoped` — regex within AST regions (tree-sitter)
- `POST /search/structural` — pattern matching with $WILDCARDS (ast-grep)
- `POST /rewrite` — structural search + transform with diff output (ast-grep)
- `POST /dataflow/analyze` — dead stores and unused variables (Go, Python)
- `POST /dataflow/taint` — source-to-sink taint tracking, built-in or custom rules
- `GET /health` — healthcheck
- `GET /cache/stats` — hit/miss counters of the scoped and dataflow caches

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

## Gate (CI)

`.github/workflows/ci.yml` on GitHub-hosted ARM runners, Rust 1.97: `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets --locked -- -D warnings`, a `--locked` build, `cargo nextest run --workspace --locked` plus `cargo test --workspace --locked --doc`, and `cargo deny check` (`deny.toml`). Releases are cut by release-please from Conventional Commits; never tag by hand. The workflow has no path filter, so a documentation-only commit ends its subject with `[skip ci]`.

## Invariants

- The service is stateless and unauthenticated by design. The only writer is `/rewrite` with `apply: true`, which writes atomically and refuses any file whose re-parse gains new tree-sitter ERROR nodes; keep that check.
- Cache entries are keyed on file mtime, size and content hash; a change to the key must keep same-length in-place edits invalidating.
- No `unwrap` / `expect` in request handlers (startup excepted).
