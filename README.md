# ox-codes

[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Rust 1.97+](https://img.shields.io/badge/rust-1.97%2B-orange.svg)](https://www.rust-lang.org)
[![Docker ready](https://img.shields.io/badge/docker-ready-2496ED.svg)](Dockerfile)

Code search and structural rewrite as an HTTP service. Wraps [ripgrep](https://github.com/BurntSushi/ripgrep), [tree-sitter](https://tree-sitter.github.io), and [ast-grep](https://ast-grep.github.io) behind one JSON API, plus an intraprocedural dataflow engine on top.

## Why use it

- **Language-aware search** — scope regex matches to function bodies, class blocks, or any named AST region across 15 languages.
- **Shape-based queries** — match by AST structure (`func $N($$$) error`) instead of text, with wildcard captures.
- **Structural rewrite** — search-and-replace at the AST level; preview as a unified diff before applying.
- **Dataflow analysis** — detect dead stores, unused variables, and tainted data flows (source→sink) without spinning up a compiler.
- **Zero state, zero auth** — stateless HTTP service; mount the directories you want analyzed and call the API.
- **Warm-cache speed** — scoped-search and dataflow results are cached per repo, keyed on each file's mtime+size and invalidated on change, so repeat queries on an unchanged tree return from cache (sub-millisecond) instead of re-parsing.

## Quick start

```sh
git clone https://github.com/anatolykoptev/ox-codes
cd ox-codes
make build   # cargo build --workspace  (requires Rust 1.97+)
make test    # cargo test --workspace
cargo run -p ox-codes -- --port 8902   # start the HTTP service
```

Then send a search request:

```sh
curl -s -X POST http://127.0.0.1:8902/search \
  -H 'Content-Type: application/json' \
  -d '{"root":"/path/to/repo","pattern":"TODO","language":"go"}' | jq .
```

Response:

```json
{
  "matches": [
    { "file": "cmd/main.go", "line": 42, "text": "// TODO: handle error" }
  ],
  "total_matches": 1,
  "truncated": false,
  "duration_ms": 8
}
```

## Endpoints

| Endpoint | Engine | Use case |
|---|---|---|
| `POST /search` | ripgrep | Grep-like text/regex search across a directory tree |
| `POST /search/scoped` | tree-sitter + regex | Regex restricted to named AST regions (functions, classes, …) |
| `POST /search/structural` | ast-grep | Pattern match by AST shape with `$WILDCARD` captures |
| `POST /rewrite` | ast-grep | Structural search-and-replace; returns unified diff per file |
| `POST /dataflow/analyze` | custom IL/CFG | Dead stores, unused variables (Go, Python) |
| `POST /dataflow/taint` | custom IL/CFG | Taint tracking source→sink with built-in or custom rules |
| `GET /health` | — | Liveness probe |
| `GET /cache/stats` | — | Hit/miss counters + entry counts for the scoped and dataflow result caches |

All search endpoints support `expand` (`"none"` / `"function"` / `"block"`) and `max_tokens` for returning full enclosing AST blocks instead of single matched lines.

## Documentation

- **[`docs/API.md`](docs/API.md)** — full request/response schemas with examples for every endpoint.
- **[`docs/EXAMPLES.md`](docs/EXAMPLES.md)** — end-to-end usage scenarios: auth-function discovery, error-pattern refactor, unused-variable sweep, taint trace.
- **[`docs/INTEGRATION.md`](docs/INTEGRATION.md)** — consumer contract: filesystem mount conventions, path pitfalls, Docker volume wiring.
- **[`docs/ROADMAP.md`](docs/ROADMAP.md)** — phase status; what is done and what is next.

## Languages supported

Scoped and structural search: Go, Python, TypeScript, JavaScript, Rust, Java, C, C++, Ruby, C#, PHP, Svelte, Astro, Bash, Lua.

Dataflow (analyze + taint): Go, Python. Additional languages tracked in [`docs/ROADMAP.md`](docs/ROADMAP.md).

## Crate layout

| Crate | Role |
|---|---|
| [`crates/core`](crates/core) | Search engine: grep, scoped, structural, rewrite, expand |
| [`crates/langs`](crates/langs) | Tree-sitter language scopes (15 languages) |
| [`crates/dataflow`](crates/dataflow) | IL builder, CFG, def-use chains, taint engine |
| [`crates/server`](crates/server) | axum HTTP handlers |
| [`src/`](src) | Binary entrypoint (`ox-codes` CLI) |

## Acknowledgements

Built on top of excellent open-source work:

- [ripgrep](https://github.com/BurntSushi/ripgrep) — fast regex search across file trees
- [tree-sitter](https://tree-sitter.github.io) — incremental, error-tolerant parsing for 15+ languages
- [ast-grep](https://ast-grep.github.io) — AST pattern matching and rewriting

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md). Please follow the [Code of Conduct](CODE_OF_CONDUCT.md).

## License

MIT — see [LICENSE](LICENSE).
