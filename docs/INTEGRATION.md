# Integration Guide

Consumer-facing documentation for ox-codes — what callers need to know to use the service correctly. Companion to [`CLAUDE.md`](../CLAUDE.md) (architecture overview) and [`ROADMAP.md`](./ROADMAP.md) (capability status).

## What ox-codes is

A Rust HTTP service that exposes ripgrep, tree-sitter, and ast-grep as a unified search-and-rewrite API. **Internal infra — no MCP exposure.** Currently has one consumer ([go-code](https://github.com/anatolykoptev/go-code)), but the contract below applies to any future caller.

The service has no authentication, no persistent state, and no access control beyond the network and filesystem mounts the operator gives it. Treat it as a privileged backend.

## The integration contract

ox-codes operates on **filesystem paths the consumer passes via `root`**. It does no cloning, fetching, or path translation of its own. This makes the contract trivial in form but unforgiving in practice:

> **Every path the consumer sends must resolve to a real, readable directory inside the ox-codes container.**

This sounds obvious. It is the source of every integration failure we have seen.

### Required: shared filesystem visibility

If the consumer writes to `/X/repo` and asks ox-codes to grep `/X/repo`, **both containers must mount `/X` at the same path**. Examples from the current go-code deployment:

```yaml
# compose/search.yml
go-code:
  read_only: true
  tmpfs:
    - /tmp
  volumes:
    - /home/krolik:/host:ro                            # local repos
    - go_code_workspace:/tmp/go-code-workspace          # remote clones (RW)

ox-codes:
  volumes:
    - /home/krolik:/host:ro                            # same path on both sides
    - go_code_workspace:/tmp/go-code-workspace:ro      # same path, RO
```

Two non-negotiable rules from this:

1. **Identical mount path on both sides.** ox-codes does not rewrite paths. If go-code clones to `/tmp/go-code-workspace/<slug>`, ox-codes must see that exact path.
2. **The consumer is the only writer.** ox-codes mounts shared workspaces read-only so concurrent writes from another caller cannot race.

### The visibility check

Before debugging stale data or zero matches, **always confirm the path exists from inside the ox-codes container**:

```sh
docker exec ox-codes ls -la <path-the-consumer-sent>
```

If this returns "No such file or directory", no amount of fixing the consumer's clone logic will help — the mount is wrong.

This caught us in 2026-05: `code_search` against any GitHub slug returned `matches=0` for a long time. We diagnosed it as a clone-staleness race (and fixed a real race in go-code's `CloneRepo` cache-hit path along the way) before realizing ox-codes physically could not see go-code's tmpfs. The mount visibility check is the first thing to run.

## API quick reference

All endpoints accept `application/json`, return `application/json` (or `text/plain` for `/health`). All search endpoints support `expand` (`"none"` | `"function"` | `"block"`) and `max_tokens` for AST-aware context.

Authoritative request/response types live in [`crates/core/src/types.rs`](../crates/core/src/types.rs); this is a summary.

### `POST /search` — ripgrep grep

```json
{
  "root": "/host/src/myrepo",
  "pattern": "TODO",
  "is_regex": false,
  "language": "go",
  "context_lines": 2,
  "max_results": 50,
  "expand": "function"
}
```

Returns `SearchResponse { matches, total_matches, truncated, duration_ms }`. Each `matches[]` item: `{ file, line, text, context[], expanded? }`.

### `POST /search/scoped` — regex within AST regions

Run the regex only inside the bodies of named AST scopes. Required: `scope` and `language`.

```json
{
  "root": "/host/src/myrepo",
  "pattern": "panic\\(",
  "scope": "function",
  "language": "go"
}
```

Valid scopes per language live in `crates/langs/`. Common: `function`, `class`, `block`.

### `POST /search/structural` — ast-grep patterns

Shape-based queries with `$WILDCARDS`. Required: `language`.

```json
{
  "root": "/host/src/myrepo",
  "pattern": "func $N($CTX context.Context, $$$) error",
  "language": "go",
  "expand": "function"
}
```

`$N` matches an identifier, `$$$` matches zero-or-more siblings. Method-receiver patterns (`$RECV.Method($$$)`) work after fix in `353dfb2`. See `crates/core/tests/` for accepted forms.

### `POST /rewrite` — structural rewrite (preview-by-default)

Same pattern grammar as `/search/structural`, plus a `rewrite` template using the same wildcards. By default returns a unified diff per file without writing.

```json
{
  "root": "/host/src/myrepo",
  "pattern": "errors.New($MSG)",
  "rewrite": "fmt.Errorf($MSG)",
  "language": "go",
  "apply": false
}
```

Phase 5 (per ROADMAP) will add `apply: true` to write changes atomically. Until then, callers must apply diffs themselves.

### `POST /dataflow/analyze` — quality findings

Dead stores, unused variables. Currently Go and Python only.

```json
{
  "root": "/host/src/myrepo",
  "language": "go",
  "max_results": 200
}
```

Returns `DataflowResponse { findings[], total_findings, files_analyzed, ... }`. Each finding: `{ kind, severity, message, file, span, variable }`.

### `POST /dataflow/taint` — taint tracking

Source→sink data-flow. Built-in rules per language; consumer can pass `rules[]` with custom source/sink/sanitizer patterns. Currently Go and Python only.

```json
{
  "root": "/host/src/myrepo",
  "language": "go",
  "max_results": 100
}
```

Returns `TaintResponse { findings[], total_findings, ... }`. Each finding: `{ rule_id, source, sink, severity, message, file }`.

### `GET /health`

Returns `200 ok`. Use for orchestration smoke probes.

## Path conventions in the current deployment

Maps consumer-side concepts to filesystem paths. Helpful when reading go-code source or debugging.

| Consumer concept | go-code path | ox-codes path | Volume |
|---|---|---|---|
| Local repo on host | `/home/krolik/src/foo` → translated to `/host/src/foo` via `PATH_MAPPINGS` | `/host/src/foo` | bind mount `/home/krolik:/host:ro` |
| Remote clone (`owner/repo` slug) | `/tmp/go-code-workspace/<slug>` | `/tmp/go-code-workspace/<slug>` | docker volume `go_code_workspace` |

Both are read-only from ox-codes' perspective — see `docker inspect ox-codes` to confirm.

## Adding a new consumer

Checklist:

1. **Mount the same paths.** If your service writes to `/X/something`, declare a volume that mounts `/X/something` at the same path in ox-codes' container.
2. **Mount read-only on the ox-codes side.** Writes should originate from one source.
3. **Match `host:container` on the consumer side.** ox-codes does not rewrite paths; whatever you send is what it greps.
4. **Confirm visibility before integrating.** `docker exec ox-codes ls <path>` from a fresh deploy. If it fails, no API call will succeed.
5. **Handle `truncated: true`** in `SearchResponse` — set `max_results` deliberately and surface truncation upstream rather than silently capping.
6. **Don't share a consumer's tmpfs.** ox-codes cannot see another container's tmpfs, by design. Use a named docker volume instead.
7. **Set timeouts on your client.** ox-codes has no per-request budget; runaway patterns on huge repos can burn CPU. The go-code client uses 10s default.

## Operational notes

**Where the service runs:** docker container `ox-codes`, host port `127.0.0.1:8904` → container `:8902`. Restart with `docker compose up -d --no-deps --force-recreate ox-codes` from `~/deploy/krolik-server`.

**Auto-deploy:** dozor watches `anatolykoptev/ox-codes` repo (config in `~/.dozor/deploy-repos.yaml`). Push to `master` → docker rebuild → smoke probe `/health`. Roughly 1–3 min depending on cargo cache.

**Logs:** `docker logs ox-codes` for stdout/stderr, `journalctl --user -u dozor -f` for deploy lifecycle.

**Concurrency model:** axum-based, requests handled on a tokio thread pool. No internal locking on the search engine — concurrent calls on the same `root` are safe as long as the directory isn't being mutated. Since shared workspaces are read-only on the ox-codes side, this is automatically the case.

## Pitfalls captured from past sessions

**Mount visibility (2026-05).** Diagnosed as clone-staleness for an hour before realizing ox-codes simply had no mount for the path go-code was sending. Always run the visibility check first.

**Stale on-disk clone.** go-code's `CloneRepo` cache used to return on-disk state without `git fetch`, so even when paths were visible the data could be hours old. Fixed go-code-side; if a future consumer maintains its own cache, document the freshness contract.

**Large `expand: function` on huge functions.** A 5000-line function returned as `body` will swamp the consumer. Always pair `expand` with `max_tokens`.

**`structural` requires language.** ast-grep patterns are per-language; sending `pattern` without `language` returns 400. The error message is clear, but easy to miss when bridging from `/search` (which accepts language as optional).

**Method-call structural patterns.** Pattern `$RECV.Method($$$)` works only after commit `353dfb2`. Older deployments will silently miss matches. Check the version label in `/health` (added in Phase 6 per ROADMAP).
