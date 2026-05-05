# ox-codes

Code search and structural rewrite as an HTTP service. Rust, axum, no MCP.

Wraps three primitives — [ripgrep](https://github.com/BurntSushi/ripgrep), [tree-sitter](https://tree-sitter.github.io), and [ast-grep](https://ast-grep.github.io) — behind one JSON API, plus a small intraprocedural dataflow engine on top.

## What it does

| Endpoint | Backed by | Use case |
|---|---|---|
| `POST /search` | ripgrep | "find every line matching this pattern" |
| `POST /search/scoped` | tree-sitter + regex | "match only inside function bodies" |
| `POST /search/structural` | ast-grep | "match by AST shape, e.g. `func $N($$$) error`" |
| `POST /rewrite` | ast-grep | structural search-and-replace, returns unified diff |
| `POST /dataflow/analyze` | custom IL/CFG | dead stores, unused variables (Go, Python) |
| `POST /dataflow/taint` | custom IL/CFG | taint tracking source→sink (Go, Python) |
| `GET /health` | — | liveness probe |

15 languages parsed for scoped/structural: Go, Python, TypeScript/JavaScript, Rust, Java, C, C++, Ruby, C#, PHP, Svelte, Astro, Bash, Lua. Dataflow currently Go and Python only ([roadmap](docs/ROADMAP.md#-phase-4--dataflow-language-expansion-next)).

## Quick start

```sh
make build   # cargo build --workspace
make test    # cargo test --workspace
make run     # cargo run -p ox-server -- --port 8902
```

Then:

```sh
curl -X POST http://127.0.0.1:8902/search \
  -H 'Content-Type: application/json' \
  -d '{"root":"/path/to/repo","pattern":"TODO","language":"go"}'
```

## Architecture

| Crate | Role |
|---|---|
| [`crates/core`](crates/core) | search engine: grep + scoped + structural + rewrite + expand |
| [`crates/langs`](crates/langs) | tree-sitter language scopes (15 languages) |
| [`crates/dataflow`](crates/dataflow) | IL builder, CFG, def-use chains, taint engine |
| [`crates/server`](crates/server) | axum HTTP handlers |
| [`src/`](src) | binary entrypoint (`ox-codes` CLI) |

No persistent state. No authentication. Internal infra — every consumer is privileged.

## Documentation

- **[`docs/INTEGRATION.md`](docs/INTEGRATION.md)** — consumer contract: mount conventions, endpoint reference, pitfalls. Read this first if you're wiring a new caller.
- **[`docs/ROADMAP.md`](docs/ROADMAP.md)** — phase status, what's done and what's next.
- **[`CLAUDE.md`](CLAUDE.md)** — short orientation for AI agents and humans dropping in cold.

## Deploy (krolik server)

```sh
cd ~/deploy/krolik-server
docker compose build --no-cache ox-codes \
  && docker compose up -d --no-deps --force-recreate ox-codes
```

Auto-deploy is wired through dozor on push to `master`. Smoke probe at `http://127.0.0.1:8904/health` (host) → `http://ox-codes:8902/health` (container).

## Versions

Workspace `0.1.0`, Rust 2024 edition. See `Cargo.toml`.

## License

Internal project. Not published.
