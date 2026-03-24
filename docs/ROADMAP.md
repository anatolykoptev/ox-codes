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

## Phase 3 — Data-Flow (Light) 🎯 next

**Impact**: очень высокий | **Effort**: высокий | **Уникальный дифференциатор**

Ни один lightweight tool не делает taint tracking. Semgrep делает, но он тяжёлый и enterprise-only. Даже intra-function версия — киллер-фича.

- [ ] Intra-function data flow: переменная от объявления до использования
- [ ] `POST /trace-variable` — path данных внутри функции
- [ ] Pattern: `$SOURCE -> ... -> $SINK` в structural queries
- [ ] Детекция: неиспользуемые присваивания, shadowed variables
- [ ] Базовый taint: пометить source (user input) → найти sink (SQL query, exec)
- [ ] go-code интеграция: новый MCP tool `trace_variable`

## Phase 4 — Incremental Index

**Impact**: высокий для scale | **Effort**: высокий

Пока наши репо <100k файлов — ripgrep справляется. Нужен когда/если будем индексировать Linux kernel или monorepo.

- [ ] Опциональный trigram index (Zoekt/Hound подход)
- [ ] `POST /index` — построить/обновить индекс
- [ ] Инвалидация по git diff
- [ ] Fallback на ripgrep для неиндексированных путей

## Phase 5 — Rewrite Write Mode

**Impact**: средний | **Effort**: низкий

Сейчас rewrite — dry-run only. Добавить возможность применить изменения.

- [ ] `POST /rewrite` + `apply: true` — записать изменения в файлы
- [ ] Atomic: все файлы или ни одного (через tmpfile + rename)
- [ ] Backup оригиналов (опционально)

## Backlog (низкий приоритет)

**Query DSL** — go-code уже маршрутизирует запросы, AI-агентам DSL не нужен.
- [ ] `POST /query` с boolean + комбинированием scope/structural

**Cross-Reference** — go-code `symbol_search` + `call_trace` покрывают.
- [ ] `POST /references`, `/definitions`, `/symbols`

**Markdown format** — expand output как ` ```lang ` блоки.
- [ ] `format: "markdown"` параметр

---

## Приоритизация

| Phase | Статус | Impact | Effort | Почему |
|-------|--------|--------|--------|--------|
| 1. Rewrite | ✅ done | ★★★★★ | ★★ | — |
| 2. Token-Aware | ✅ done | ★★★★★ | ★★★ | — |
| 3. Data-Flow | 🎯 next | ★★★★★ | ★★★★★ | Уникальный, ни у кого нет в lightweight |
| 4. Index | planned | ★★★★ | ★★★★★ | Нужен для scale >100k файлов |
| 5. Write Mode | planned | ★★★ | ★ | Простое расширение Phase 1 |
| Backlog | — | ★★ | ★★★ | go-code уже покрывает |

## Принципы

1. **Каждая фаза — самостоятельная ценность**
2. **HTTP-first** — MCP/CLI/UI через go-code
3. **Zero-index по умолчанию** — индекс опционален
4. **≤200 LOC на файл**
