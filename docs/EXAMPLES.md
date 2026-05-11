# Usage Examples

End-to-end scenarios showing how to call ox-codes from the command line. All examples assume the service is running on `localhost:8902`. Replace `/path/to/repo` with an absolute path on the machine running ox-codes.

---

## 1. Find auth-related functions

**Goal**: discover every function in a Go service that touches authentication, across all files — without reading each file manually.

First, do a broad ripgrep search to find files and lines:

```sh
curl -s -X POST http://localhost:8902/search \
  -H 'Content-Type: application/json' \
  -d '{
    "root": "/path/to/repo",
    "pattern": "auth",
    "language": "go",
    "case_sensitive": false,
    "max_results": 100
  }' | jq '.matches[] | .file' | sort -u
```

Then narrow to function definitions using scoped search:

```sh
curl -s -X POST http://localhost:8902/search/scoped \
  -H 'Content-Type: application/json' \
  -d '{
    "root": "/path/to/repo",
    "pattern": "auth",
    "scope": "function",
    "language": "go",
    "case_sensitive": false,
    "expand": "function",
    "max_tokens": 1500
  }' | jq '.matches[] | {file: .file, line: .line, fn: .expanded.symbol_name}'
```

Expected output excerpt:

```json
{ "file": "middleware/auth.go", "line": 14, "fn": "ValidateToken" }
{ "file": "handlers/login.go",  "line": 38, "fn": "HandleLogin" }
{ "file": "store/session.go",   "line": 71, "fn": "CreateAuthSession" }
```

---

## 2. Refactor error-wrapping pattern

**Goal**: replace bare `errors.New(msg)` calls with `fmt.Errorf("%w", err)` across a Go codebase. Preview the diff before applying.

Preview:

```sh
curl -s -X POST http://localhost:8902/rewrite \
  -H 'Content-Type: application/json' \
  -d '{
    "root": "/path/to/repo",
    "pattern": "errors.New($MSG)",
    "rewrite": "fmt.Errorf(\"%w\", $MSG)",
    "language": "go",
    "apply": false
  }' | jq '.files[] | {file: .file, matches: .matches, diff: .diff}'
```

Expected output excerpt:

```json
{
  "file": "pkg/store/db.go",
  "matches": 2,
  "diff": "--- a/pkg/store/db.go\n+++ b/pkg/store/db.go\n@@ -47,1 +47,1 @@\n-\treturn errors.New(\"record not found\")\n+\treturn fmt.Errorf(\"%w\", \"record not found\")\n"
}
```

Apply once the diff looks correct:

```sh
curl -s -X POST http://localhost:8902/rewrite \
  -H 'Content-Type: application/json' \
  -d '{
    "root": "/path/to/repo",
    "pattern": "errors.New($MSG)",
    "rewrite": "fmt.Errorf(\"%w\", $MSG)",
    "language": "go",
    "apply": true
  }' | jq '{total_matches, total_files}'
```

---

## 3. Detect unused variables in Go

**Goal**: run the dataflow analyzer across a Go module to find variables that are assigned but never read, or stores that are immediately overwritten.

```sh
curl -s -X POST http://localhost:8902/dataflow/analyze \
  -H 'Content-Type: application/json' \
  -d '{
    "root": "/path/to/repo",
    "language": "go",
    "max_results": 200
  }' | jq '.findings[] | select(.kind == "unused_variable") | {file: .file, line: .span.start_line, var: .variable, msg: .message}'
```

Expected output excerpt:

```json
{ "file": "pkg/cache/lru.go",   "line": 83, "var": "evicted", "msg": "variable 'evicted' is assigned but never read" }
{ "file": "handlers/search.go", "line": 21, "var": "ctx",     "msg": "variable 'ctx' is assigned but never read" }
```

Scope to a specific subdirectory using `file_glob`:

```sh
curl -s -X POST http://localhost:8902/dataflow/analyze \
  -H 'Content-Type: application/json' \
  -d '{
    "root": "/path/to/repo",
    "language": "go",
    "file_glob": "pkg/store/**",
    "max_results": 50
  }' | jq '.findings | length'
```

---

## 4. Trace user input to SQL sink (taint analysis)

**Goal**: find code paths where HTTP request parameters flow into SQL query execution without sanitization — a SQL-injection risk.

Use the built-in Go rules (no `rules` field needed):

```sh
curl -s -X POST http://localhost:8902/dataflow/taint \
  -H 'Content-Type: application/json' \
  -d '{
    "root": "/path/to/repo",
    "language": "go",
    "max_results": 50
  }' | jq '.findings[] | {rule: .rule_id, file: .file, source_line: .source.span.start_line, sink_line: .sink.span.start_line, cwe: .sink.cwe, msg: .message}'
```

Expected output excerpt:

```json
{
  "rule": "sql-injection",
  "file": "handlers/user.go",
  "source_line": 22,
  "sink_line": 28,
  "cwe": "CWE-89",
  "msg": "Tainted data from `FormValue` flows to `Exec` (sql-injection)"
}
```

Supply a custom rule to detect a project-specific sink:

```sh
curl -s -X POST http://localhost:8902/dataflow/taint \
  -H 'Content-Type: application/json' \
  -d '{
    "root": "/path/to/repo",
    "language": "go",
    "rules": [
      {
        "id": "custom-log-injection",
        "severity": "warning",
        "sources": [{ "pattern": "FormValue", "tag": "user_input" }],
        "sinks":   [{ "pattern": "Infof", "arg_index": -1, "cwe": "CWE-117", "description": "Log injection" }],
        "sanitizers": []
      }
    ]
  }' | jq '.findings[] | {rule: .rule_id, file: .file, msg: .message}'
```

---

## 5. Find all context-aware handler signatures (structural search)

**Goal**: locate every Go HTTP handler function that accepts a `context.Context` parameter, to audit which handlers propagate context correctly.

```sh
curl -s -X POST http://localhost:8902/search/structural \
  -H 'Content-Type: application/json' \
  -d '{
    "root": "/path/to/repo",
    "pattern": "func $NAME($CTX context.Context, $$$) $RET",
    "language": "go",
    "expand": "function",
    "max_tokens": 800,
    "max_results": 100
  }' | jq '.matches[] | {file: .file, line: .line, fn: .expanded.symbol_name}'
```

Expected output excerpt:

```json
{ "file": "handlers/search.go",   "line": 15, "fn": "HandleSearch" }
{ "file": "store/postgres.go",    "line": 42, "fn": "QueryUsers" }
{ "file": "middleware/timeout.go", "line": 8,  "fn": "WithTimeout" }
```
