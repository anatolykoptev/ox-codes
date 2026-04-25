# ox-codes Roadmap

**Текущая версия**: v0.1.0 | **~5,700 LOC** | **15 языков** | **32+ теста**

## Текущие возможности

- **Grep** — ripgrep, density ranking, glob/exclude, language filter, context lines
- **Scoped search** — tree-sitter, 5 scope kinds × 15 языков
- **Structural search** — ast-grep-core, `$WILDCARDS`, language-aware preprocessing
- **Rewrite** ✅ — ast-grep structural rewrite с unified diff output (dry-run)
- **Expand** ✅ — AST-aware expansion до функций/struct/class
- **Token budget** ✅ — `max_tokens` для ограничения expanded body
- **Dataflow analyze** ✅ — dead stores, unused vars (Go, Python)
- **Dataflow taint** ✅ — source→sink taint tracking с custom rules (Go, Python)

### HTTP API endpoints

| Endpoint | Статус |
|---------|--------|
| `POST /search` | ✅ |
| `POST /search/scoped` | ✅ |
| `POST /search/structural` | ✅ |
| `POST /rewrite` | ✅ dry-run |
| `POST /dataflow/analyze` | ✅ Go, Python |
| `POST /dataflow/taint` | ✅ Go, Python |

---

## ✅ Phase 1 — Rewrite Engine (done)

- [x] `POST /rewrite` — structural search + transform, unified diff output
- [x] `similar` crate для LCS-based diffs
- [x] go-code `rewrite` MCP tool
- [ ] **Write mode** — `apply: true` записывает изменения в файлы (не только preview)

## ✅ Phase 2 — Token-Aware Context (done)

- [x] `expand: "function"` / `expand: "block"`
- [x] `max_tokens` — фильтрация по размеру для LLM-контекста
- [x] Метаданные: `symbol_name`, `symbol_kind`, `line_start`, `line_end`, `body`
- [ ] **`format: "markdown"`** — ` ```lang ` блоки в output (trivial)

## ✅ Phase 3 — Data-Flow Engine (done)

Полный intraprocedural dataflow engine реализован:

- [x] IL (Intermediate Language) builder — `crates/dataflow/src/il_builder.rs`
- [x] CFG (Control Flow Graph) builder — `crates/dataflow/src/cfg_builder.rs`
- [x] Def-use chains — `crates/dataflow/src/def_use.rs`
- [x] Reaching definitions — `crates/dataflow/src/reaching_defs.rs`
- [x] Dead store / unused var detection — `crates/dataflow/src/analysis.rs`
- [x] Taint tracking (source→sink) — `crates/dataflow/src/taint.rs`
- [x] Custom taint rules — `crates/dataflow/src/taint_rules.rs`
- [x] Language queries: **Go, Python** только
- [ ] **TypeScript/JavaScript queries** — нет `LangQueries` impl
- [ ] **Rust queries** — нет `LangQueries` impl
- [ ] **Java queries** — нет `LangQueries` impl

## 🎯 Phase 4 — Dataflow Language Expansion (next)

**Impact**: высокий | **Effort**: средний | **Блокер**: taint только для Go/Python

Добавить `LangQueries` trait impl для TypeScript, JavaScript, Rust.
Каждый язык — tree-sitter queries для деклараций, присваиваний, параметров, ref, вызовов.

- [ ] TypeScript + JavaScript queries (один impl, общий грамматик)
- [ ] Rust queries
- [ ] Java queries (опционально)
- [ ] Обновить `get_queries()` в `crates/dataflow/src/queries/mod.rs`
- [ ] Тесты для каждого языка

## Phase 5 — Rewrite Write Mode (easy win)

**Impact**: средний | **Effort**: низкий ★ | **Разблокирует**: go-code `rewrite` tool применяет изменения

- [ ] Добавить `apply: bool` в `RewriteInput` (core/src/types.rs)
- [ ] После `apply_edits()` — записать файл если `apply == true`
- [ ] Atomic: записывать через tmpfile + rename
- [ ] Обновить go-code клиент + MCP tool

## Phase 6 — Markdown Format (trivial)

- [ ] `format: "markdown"` параметр в `/search` expand output
- [ ] Оборачивать `body` в ` ```lang\n...\n``` `

## Backlog

- [ ] Trigram index для репо >100k файлов (Phase 4 из старого роадмапа)
- [ ] `POST /references`, `/definitions` (покрывается go-code `symbol_search`)

---

## Конкурентная позиция

| Фича | ox-codes | ast-grep | Semgrep | Probe |
|------|----------|----------|---------|-------|
| Fast grep (ripgrep) | ✅ | — | — | BM25 |
| Scoped search | ✅ | — | partial | partial |
| Structural ($WILD) | ✅ | ✅ | ✅ | — |
| Rewrite/transform | ✅ dry-run | ✅ | ✅ | — |
| Token-aware expand | ✅ | — | — | ✅ |
| Dataflow (dead store) | ✅ Go/Py | — | ✅ | — |
| Taint tracking | ✅ Go/Py | — | ✅ enterprise | — |
| HTTP API | ✅ | CLI only | CLI | MCP |
| MCP (via go-code) | ✅ | — | — | ✅ |

## Принципы

1. **Каждая фаза — самостоятельная ценность**
2. **HTTP-first** — MCP/CLI/UI через go-code
3. **≤200 LOC на файл**
