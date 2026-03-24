# ox-codes Roadmap

**Текущая версия**: v1.1 | **~2,200 LOC** | **15 языков** | **32 теста**

## Текущие возможности

- **Grep** — ripgrep crates, density ranking, glob/exclude, language filter, context lines
- **Scoped search** — tree-sitter, 5 scope kinds × 15 языков
- **Structural search** — ast-grep-core, `$WILDCARDS`, language-aware preprocessing
- **Rewrite** ✅ — ast-grep structural rewrite с unified diff output (`similar` crate)
- **Expand** ✅ — AST-aware expansion до функций/struct/class (`expand: "function"/"block"`)
- **Token budget** ✅ — `max_tokens` для ограничения expanded body
- **HTTP API** — 4 endpoint'а (`/search`, `/search/scoped`, `/search/structural`, `/rewrite`)

### go-code интеграция (MCP)

| go-code tool | ox-codes фича |
|-------------|--------------|
| `code_search` | grep + scoped + structural + **expand** + **max_tokens** |
| `code_health` | scoped search (TODOs, unhandled errors, magic numbers) |
| `dead_code` | scoped string ref filtering |
| `rewrite` ✅ | structural rewrite с diff preview |

## Конкурентная позиция

| Фича | ox-codes | ast-grep | Semgrep | Probe | Sourcegraph |
|------|----------|----------|---------|-------|-------------|
| Fast grep (ripgrep) | ✅ | — | — | BM25 | Zoekt |
| Scoped search (in-function) | ✅ | — | partial | partial | — |
| Structural ($WILD) | ✅ | ✅ | ✅ | — | Comby |
| Rewrite/transform | ✅ | ✅ | ✅ | — | Batch |
| Token-aware expand | ✅ | — | — | ✅ | — |
| HTTP API | ✅ | CLI only | CLI | MCP | ✅ |
| MCP (via go-code) | ✅ | — | — | ✅ | ✅ |

**Уникальная позиция**: единственный инструмент с ripgrep + scoped + structural + rewrite + expand в одном HTTP API.

---

## ✅ Phase 1 — Rewrite Engine (done)

- [x] `POST /rewrite` — structural search + transform, unified diff output
- [x] `similar` crate для LCS-based diffs
- [x] go-code `rewrite` MCP tool
- [ ] Write mode — применить изменения к файлам (не только preview)

## ✅ Phase 2 — Token-Aware Context (done)

- [x] `expand: "function"` — match → полная функция/метод
- [x] `expand: "block"` — match → struct/class/impl/trait
- [x] `max_tokens` — фильтрация по размеру для LLM-контекста
- [x] Метаданные: `symbol_name`, `symbol_kind`, `line_start`, `line_end`, `body`
- [x] go-code `code_search` интеграция
- [ ] `format: "markdown"` — ` ```lang ` блоки в output

## Phase 3 — Advanced Query Language

**Impact**: средний | **Effort**: средний

Единый query DSL вместо отдельных endpoint'ов.

- [ ] `POST /query` — единый endpoint
- [ ] Boolean: `AND`, `OR`, `NOT`
- [ ] Комбинирование: `scope:function_bodies AND structural:"if $ERR != nil"`
- [ ] Фильтры: `lang:go file:*_test.go -path:vendor`
- [ ] Пагинация: `offset` + `limit`

## Phase 4 — Incremental Index

**Impact**: высокий для scale | **Effort**: высокий

- [ ] Опциональный trigram index (Zoekt/Hound подход)
- [ ] `POST /index` — построить/обновить индекс
- [ ] Инвалидация по git diff
- [ ] Fallback на ripgrep для неиндексированных путей

## Phase 5 — Cross-Reference Engine

**Impact**: средний (go-code частично покрывает) | **Effort**: высокий

- [ ] tree-sitter extraction определений и ссылок
- [ ] `POST /references` — find all usages
- [ ] `POST /definitions` — go-to-definition
- [ ] `POST /symbols` — outline файла/директории

## Phase 6 — Data-Flow (Light)

**Impact**: очень высокий | **Effort**: очень высокий

- [ ] Intra-function data flow: переменная от объявления до использования
- [ ] `POST /trace-variable`
- [ ] Базовый taint: source (user input) → sink (SQL query)

---

## Приоритизация

| Phase | Статус | Impact | Effort |
|-------|--------|--------|--------|
| 1. Rewrite | ✅ done | ★★★★★ | ★★ |
| 2. Token-Aware | ✅ done | ★★★★★ | ★★★ |
| 3. Query DSL | next | ★★★ | ★★★ |
| 4. Index | planned | ★★★★ | ★★★★★ |
| 5. Cross-Ref | planned | ★★★ | ★★★★★ |
| 6. Data-Flow | planned | ★★★★★ | ★★★★★★ |

## Принципы

1. **Каждая фаза — самостоятельная ценность**
2. **HTTP-first** — MCP/CLI/UI через go-code
3. **Zero-index по умолчанию** — индекс опционален
4. **≤200 LOC на файл**
