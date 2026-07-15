# API Reference

All endpoints accept `Content-Type: application/json` and return `application/json` (or `text/plain` for `/health`).

Authoritative request/response types are in [`crates/core/src/types.rs`](../crates/core/src/types.rs) and [`crates/dataflow/src/types.rs`](../crates/dataflow/src/types.rs).

---

## POST /search

Grep-like search using ripgrep. Scans every file under `root` that matches the optional `language` and glob filters.

### Request

```json
{
  "root": "/path/to/repo",
  "pattern": "TODO",
  "is_regex": false,
  "language": "go",
  "case_sensitive": true,
  "context_lines": 2,
  "max_results": 50,
  "file_glob": "cmd/**",
  "exclude_glob": "vendor/**",
  "expand": "none",
  "max_tokens": null,
  "format": "plain"
}
```

| Field | Type | Default | Description |
|---|---|---|---|
| `root` | string | **required** | Absolute path to the directory to search |
| `pattern` | string | **required** | Search term or regex |
| `is_regex` | bool | `false` | Treat `pattern` as a regex |
| `language` | string | `null` | Filter to files of this language (`"go"`, `"python"`, …) |
| `case_sensitive` | bool | `true` | Case-sensitive matching |
| `context_lines` | int | `2` | Lines of context before/after each match |
| `max_results` | int | `50` | Cap on returned matches |
| `file_glob` | string | `null` | Include-only glob (e.g. `"src/**"`) |
| `exclude_glob` | string | `null` | Exclude glob (e.g. `"**/testdata/**"`) |
| `expand` | string | `"none"` | `"none"` / `"function"` / `"block"` — expand match to enclosing AST node |
| `max_tokens` | int | `null` | Truncate `expanded.body` to this many characters |
| `format` | string | `"plain"` | `"plain"` / `"markdown"` — format for `expanded.body` |

### Response

```json
{
  "matches": [
    {
      "file": "cmd/main.go",
      "line": 42,
      "text": "// TODO: handle error",
      "context": ["", "func main() {"],
      "expanded": {
        "symbol_name": "main",
        "symbol_kind": "function",
        "line_start": 40,
        "line_end": 55,
        "body": "func main() {\n    // TODO: handle error\n}"
      }
    }
  ],
  "total_matches": 1,
  "truncated": false,
  "duration_ms": 12
}
```

`expanded` is omitted when `expand` is `"none"`. `context` is omitted when empty.

### Errors

| Status | Condition |
|---|---|
| `400` | Invalid regex, unreadable root, unknown language |
| `500` | Internal spawn failure |

---

## POST /search/scoped

Runs the regex only inside the bodies of named AST regions (functions, classes, blocks). Requires `scope` and `language`.

### Request

```json
{
  "root": "/path/to/repo",
  "pattern": "panic\\(",
  "scope": "function",
  "language": "go",
  "is_regex": true,
  "case_sensitive": true,
  "max_results": 50,
  "file_glob": null,
  "exclude_glob": null,
  "expand": "none",
  "max_tokens": null,
  "format": "plain"
}
```

| Field | Type | Default | Description |
|---|---|---|---|
| `root` | string | **required** | Directory to search |
| `pattern` | string | **required** | Regex or literal to match inside scope bodies |
| `scope` | string | **required** | AST scope kind: `"function"`, `"class"`, `"block"` (language-dependent) |
| `language` | string | **required** | Language to parse (`"go"`, `"python"`, `"typescript"`, …) |
| `is_regex` | bool | `false` | Treat `pattern` as a regex |
| `case_sensitive` | bool | `true` | Case-sensitive matching |
| `max_results` | int | `50` | Cap on returned matches |
| `file_glob` | string | `null` | Include-only glob |
| `exclude_glob` | string | `null` | Exclude glob |
| `expand` | string | `"none"` | Expand match to enclosing AST node |
| `max_tokens` | int | `null` | Truncate `expanded.body` |
| `format` | string | `"plain"` | `"plain"` / `"markdown"` |

Valid scope names per language are defined in `crates/langs/src/<lang>.rs`. Common values: `function`, `class`, `block`.

### Response

Same shape as `/search` (`ExpandedSearchResponse`).

### Errors

| Status | Condition |
|---|---|
| `400` | Unknown language, unsupported scope, invalid regex |
| `500` | Internal spawn failure |

---

## POST /search/structural

AST pattern matching with `$WILDCARD` captures via ast-grep. Matches by tree shape rather than text. Requires `language`.

### Request

```json
{
  "root": "/path/to/repo",
  "pattern": "func $N($CTX context.Context, $$$) error",
  "language": "go",
  "max_results": 50,
  "file_glob": null,
  "exclude_glob": null,
  "expand": "function",
  "max_tokens": 2000,
  "format": "plain"
}
```

| Field | Type | Default | Description |
|---|---|---|---|
| `root` | string | **required** | Directory to search |
| `pattern` | string | **required** | ast-grep pattern with `$NAME` (single node) or `$$$` (zero-or-more siblings) wildcards |
| `language` | string | **required** | Language for AST parsing |
| `max_results` | int | `50` | Cap on matches |
| `file_glob` | string | `null` | Include-only glob |
| `exclude_glob` | string | `null` | Exclude glob |
| `expand` | string | `"none"` | Expand match to enclosing AST node |
| `max_tokens` | int | `null` | Truncate `expanded.body` |
| `format` | string | `"plain"` | `"plain"` / `"markdown"` |

Pattern notes:
- `$NAME` — matches a single AST node (identifier, expression, …)
- `$$$` — matches zero or more sibling nodes (variadic arguments, parameter list tail)
- Method-call patterns (`$RECV.Method($$$)`) are supported.

### Response

Same shape as `/search` (`ExpandedSearchResponse`).

### Errors

| Status | Condition |
|---|---|
| `400` | Missing `language`, invalid pattern, unsupported language |
| `500` | Internal spawn failure |

---

## POST /rewrite

Structural search-and-replace using the same pattern grammar as `/search/structural`. Returns a unified diff per modified file. By default, changes are **not** written to disk (`apply: false`).

### Request

```json
{
  "root": "/path/to/repo",
  "pattern": "errors.New($MSG)",
  "rewrite": "fmt.Errorf(\"%w\", $MSG)",
  "language": "go",
  "apply": false,
  "max_results": 50,
  "file_glob": null,
  "exclude_glob": null
}
```

| Field | Type | Default | Description |
|---|---|---|---|
| `root` | string | **required** | Directory to rewrite |
| `pattern` | string | **required** | ast-grep match pattern |
| `rewrite` | string | **required** | Replacement template (uses same `$WILDCARD` captures) |
| `language` | string | **required** | Language for AST parsing |
| `apply` | bool | `false` | When `true`, write changes atomically to disk |
| `max_results` | int | `50` | Cap on total matches |
| `file_glob` | string | `null` | Include-only glob |
| `exclude_glob` | string | `null` | Exclude glob |

### Response

```json
{
  "files": [
    {
      "file": "pkg/util/errors.go",
      "matches": 3,
      "skipped": 1,
      "diff": "--- a/pkg/util/errors.go\n+++ b/pkg/util/errors.go\n@@ -10,1 +10,1 @@\n-errors.New(msg)\n+fmt.Errorf(\"%w\", msg)\n"
    }
  ],
  "total_matches": 3,
  "total_skipped": 1,
  "total_files": 1,
  "duration_ms": 45,
  "rejected": [
    {
      "file": "pkg/util/broken.go",
      "reason": "re-parse introduced 2 new ERROR/MISSING node(s) (original=0, modified=2); the rewrite would corrupt the file"
    }
  ]
}
```

#### Counting semantics

`matches`/`total_matches` count only edits **ACTUALLY applied** (post-overlap-resolution), not raw ast-grep matches. When `foo($$$ARGS)` matches `foo(foo(x))` at two nesting depths, only the outer (widest) edit is applied; the inner one is counted in `skipped`/`total_skipped`.

| Field | Type | Description |
|---|---|---|
| `files` | array | One entry per file with at least one applied edit |
| `files[].matches` | int | Edits applied to this file (post-overlap-resolution) |
| `files[].skipped` | int | Edits skipped for this file due to overlapping/nested match ranges (omitted when 0) |
| `files[].diff` | string | Unified diff of the changes |
| `total_matches` | int | Total edits applied across all files |
| `total_skipped` | int | Total edits skipped due to overlapping/nested ranges (omitted when 0) |
| `total_files` | int | Number of files with at least one applied edit |
| `duration_ms` | int | Wall-clock duration |
| `rejected` | array | Files whose post-edit re-parse invariant failed and were **NOT** persisted (omitted when empty). The batch continues past these so valid files still land. |
| `rejected[].file` | string | File path (relative to root) |
| `rejected[].reason` | string | Why the file was rejected (new ERROR/MISSING nodes introduced) |

When `apply: true`, each file's modified buffer is re-parsed with the same grammar used for matching. If the re-parse introduces new `ERROR` or `MISSING` tree-sitter nodes that were not in the original tree, the file is **not** persisted and is reported in `rejected` instead. The batch does not abort — valid files are still written.

### Errors

| Status | Condition |
|---|---|
| `400` | Missing `language`, invalid pattern |
| `500` | Internal spawn failure |

Note: a per-file re-parse invariant failure (when `apply: true`) is **not** a `400` — the file is reported in `rejected` and the batch continues. A `400` is returned only for request-level errors (bad pattern, unknown language).

---

## POST /dataflow/analyze

Static dataflow analysis: dead stores, unused variables, constant-value propagation. Currently supports **Go** and **Python**.

### Request

```json
{
  "root": "/path/to/repo",
  "language": "go",
  "max_results": 100,
  "max_files": null,
  "file_glob": null,
  "exclude_glob": null
}
```

| Field | Type | Default | Description |
|---|---|---|---|
| `root` | string | **required** | Directory to analyze |
| `language` | string | **required** | `"go"` or `"python"` |
| `max_results` | int | `100` | Cap on findings returned (server-side max `1000`) |
| `max_files` | int | `null` | Cap on files walked; `null` → server-side max `10000` |
| `file_glob` | string | `null` | Include-only glob |
| `exclude_glob` | string | `null` | Exclude glob |

**Server-side clamping**: `max_results` is clamped to `1000` and `max_files` is clamped to `10000` (an explicit `null` is treated as `10000`). Without this, a caller can request unbounded findings or force a full-repo walk on arbitrarily large repos.

### Response

```json
{
  "findings": [
    {
      "kind": "unused_variable",
      "severity": "warning",
      "message": "variable 'err' is assigned but never read",
      "file": "pkg/store/db.go",
      "span": {
        "start_byte": 1024,
        "end_byte": 1031,
        "start_line": 47,
        "end_line": 47
      },
      "variable": "err"
    }
  ],
  "total_findings": 1,
  "files_analyzed": 12,
  "truncated": false,
  "files_truncated": false,
  "duration_ms": 230
}
```

Finding `kind` values: `dead_store`, `unused_variable`, `constant_value`, `uninitialized_var`, `unreachable_code`.

Finding `severity` values: `error`, `warning`, `info`.

| Field | Type | Description |
|---|---|---|
| `findings` | array | Findings (capped at `max_results`) |
| `total_findings` | int | Total findings before truncation |
| `files_analyzed` | int | Number of files actually analyzed |
| `truncated` | bool | `true` if `total_findings > max_results` (findings budget hit) |
| `files_truncated` | bool | `true` if the walk was cut short by `max_files` (treat as a sample, not exhaustive) |
| `duration_ms` | int | Wall-clock duration |

### Errors

| Status | Condition |
|---|---|
| `400` | Unsupported language, bad request |
| `500` | Internal spawn failure |
| `503` | Walk pool saturated (all 8 permits held by in-flight walks); retry later |
| `504` | Analysis exceeded the 25s hard deadline |

### Cache

Results are cached by a byte-weighed LRU (`moka`) keyed on repo root + language + globs + `max_results` + an aggregate file fingerprint (mtime + size per file). The default ceiling is **64 MB** of estimated response bytes (override via `OX_CODES_DATAFLOW_CACHE_BYTES`; `0` = disable). TTL defaults to 300s (override via `OX_CODES_DATAFLOW_CACHE_TTL_SECS`; `0` = no expiry). The legacy `OX_CODES_DATAFLOW_CACHE_ENTRIES` (entry-count cap) was renamed to `OX_CODES_DATAFLOW_CACHE_BYTES` (byte ceiling) — if only the old name is set, a warning is emitted and the byte default is used (the old numeric value is NOT reinterpreted as bytes).

---

## POST /dataflow/taint

Intraprocedural taint tracking: finds data flows from configurable sources to sinks. Built-in rules cover SQL injection and command injection for Go and Python. Custom rules can be supplied per request.

### Request

```json
{
  "root": "/path/to/repo",
  "language": "go",
  "max_results": 100,
  "file_glob": null,
  "exclude_glob": null,
  "rules": null
}
```

| Field | Type | Default | Description |
|---|---|---|---|
| `root` | string | **required** | Directory to analyze |
| `language` | string | **required** | `"go"` or `"python"` |
| `max_results` | int | `100` | Cap on findings returned |
| `file_glob` | string | `null` | Include-only glob |
| `exclude_glob` | string | `null` | Exclude glob |
| `rules` | array | `null` | Custom taint rules (see below); `null` uses built-in rules for the language |

Custom rule schema:

```json
{
  "id": "my-rule",
  "severity": "error",
  "sources": [
    { "pattern": "UserInput", "tag": "user_input" }
  ],
  "sinks": [
    { "pattern": "ExecQuery", "arg_index": 0, "cwe": "CWE-89", "description": "SQL injection" }
  ],
  "sanitizers": [
    { "pattern": "Escape" }
  ]
}
```

`arg_index`: 0-based index of the argument to check; `-1` means any argument.

### Response

```json
{
  "findings": [
    {
      "rule_id": "sql-injection",
      "source": {
        "function": "FormValue",
        "span": { "start_byte": 512, "end_byte": 530, "start_line": 22, "end_line": 22 }
      },
      "sink": {
        "function": "Exec",
        "span": { "start_byte": 640, "end_byte": 660, "start_line": 28, "end_line": 28 },
        "arg_index": 0,
        "cwe": "CWE-89"
      },
      "severity": "error",
      "message": "Tainted data from `FormValue` flows to `Exec` (sql-injection)",
      "file": "handlers/user.go"
    }
  ],
  "total_findings": 1,
  "files_analyzed": 8,
  "truncated": false,
  "duration_ms": 185
}
```

### Errors

| Status | Condition |
|---|---|
| `400` | Unsupported language, malformed rule |
| `500` | Internal spawn failure |

---

## GET /cache/stats

Per-cache effectiveness counters and walk-pool stats.

### Response

```json
{
  "scope": {
    "hits": 42,
    "misses": 7,
    "entry_count": 5
  },
  "dataflow": {
    "hits": 3,
    "misses": 1,
    "entry_count": 1
  },
  "walks": {
    "in_flight": 1,
    "oldest_start_ms": 1718467200000
  }
}
```

| Field | Type | Description |
|---|---|---|
| `scope.hits` / `scope.misses` | int | Scope cache counters |
| `scope.entry_count` | int | Entries in the scope cache |
| `dataflow.hits` / `dataflow.misses` | int | Dataflow result cache counters |
| `dataflow.entry_count` | int | Entries in the dataflow cache |
| `walks.in_flight` | int | Currently-active directory walks (permits held, max 8) |
| `walks.oldest_start_ms` | int | UNIX-epoch ms of the oldest in-flight walk, or `0` if none. A walk whose age exceeds 25s is "stuck" (its permit will never be returned because `spawn_blocking` cannot be cancelled) — the operator should investigate before the pool exhausts and every request starts getting `503`. |

---

## GET /health

Liveness probe.

### Response

```
200 OK
ok
```

Returns `text/plain` `"ok"`. Use for orchestration smoke probes and readiness checks.
