# ox-codes Roadmap

**Текущая версия**: v1.1 | **~2,200 LOC** | **15 языков** | **32 теста**

## Текущие возможности (baseline)

- **Grep** — ripgrep crates, density ranking, glob/exclude, language filter, context lines
- **Scoped search** — tree-sitter, 5 scope kinds × 15 языков
- **Structural search** — ast-grep-core, `$WILDCARDS`, language-aware preprocessing
- **Rewrite** — ast-grep structural rewrite с unified diff output
- **Expand** — AST-aware expansion matches до функций/struct/class
- **HTTP API** — 4 endpoint'а (`/search`, `/search/scoped`, `/search/structural`, `/rewrite`)

## Конкурентный контекст

| Инструмент | Уникальная сила | Чего нет у ox-codes |
|-----------|----------------|-------------------|
| **ast-grep** (13k★) | Rewrite engine, YAML lint rules, playground | Трансформация кода |
| **Semgrep** (11k★) | Taint tracking, data-flow, 30+ языков | Data-flow анализ |
| **Sourcegraph/Zoekt** | Trigram index, multi-repo, Batch Changes | Персистентный индекс |
| **Probe** (511★) | Native MCP, token-aware context для LLM | Семантические блоки для AI |
| **Comby** | Language-agnostic rewrite с holes | Rewrite templates |
| **GitHub Blackbird** | Ngram index, 200M+ repos | Scale-архитектура |

**Уникальная позиция ox-codes**: единственный инструмент, объединяющий ripgrep + AST-scoped + structural search в одном HTTP API.

---

## Phase 1 — Rewrite Engine

**Impact**: высокий | **Effort**: низкий (ast-grep-core уже поддерживает)

ast-grep-core имеет встроенную поддержку rewrite — нужно прокинуть через API.

- [x] `POST /rewrite` — применить трансформацию к файлам, возвращать unified diff
- [x] Формат результата: unified diff (via `similar` crate) + summary (files changed, matches replaced)
- [ ] Dry-run режим — показать что изменится без записи (TODO: write mode)

**Примеры использования:**
```json
// Поиск + превью трансформации
{"pattern": "log.Printf($MSG)", "rewrite": "slog.Info($MSG)", "language": "go"}

// Рефакторинг error handling
{"pattern": "if $ERR != nil { return $ERR }", "rewrite": "if $ERR != nil { return fmt.Errorf(\"...: %w\", $ERR) }"}
```

## Phase 2 — Token-Aware Context

**Impact**: высокий (киллер-фича для AI) | **Effort**: средний

AI-агентам нужны не строки, а полные семантические блоки.

- [x] `expand: "function"` — расширить match до родительской AST-ноды (функция/method)
- [x] `expand: "block"` — расширить до ближайшего блока (struct, class, impl, trait)
- [x] `max_tokens` лимит — отсекать результаты по размеру для LLM-контекста
- [x] Возврат метаданных: `{symbol_name, symbol_kind, line_start, line_end, body}`
- [ ] `format: "markdown"` — результаты с ` ```lang ` блоками

**Пример:**
```json
// Запрос: найти TODO в функциях, вернуть полные функции
{"pattern": "TODO", "scope": "function_bodies", "expand": "function", "max_tokens": 4000}

// Результат: полное тело функции, а не одна строка
```

## Phase 3 — Advanced Query Language

**Impact**: средний | **Effort**: средний

Единый query DSL вместо отдельных endpoint'ов.

- [ ] `POST /query` — единый endpoint
- [ ] Boolean операторы: `AND`, `OR`, `NOT`
- [ ] Комбинирование mode'ов: `scope:function_bodies AND structural:"if $ERR != nil"`
- [ ] Фильтры: `lang:go file:*_test.go -path:vendor`
- [ ] Сортировка: `sort:density`, `sort:relevance`, `sort:path`
- [ ] Пагинация: `offset` + `limit`

**Синтаксис:**
```
# Найти error handling в тестах
scope:function_bodies "err != nil" lang:go file:*_test.go

# Structural + grep в одном запросе
structural:"if $ERR != nil { $BODY }" AND "database"
```

## Phase 4 — Incremental Index

**Impact**: высокий для scale | **Effort**: высокий

Для кодовых баз >100k файлов grep без индекса слишком медленный.

- [ ] Опциональный trigram index (подход Zoekt/Hound)
- [ ] `POST /index` — построить/обновить индекс для директории
- [ ] `DELETE /index` — удалить индекс
- [ ] Инвалидация по git diff (только изменённые файлы)
- [ ] File watcher для автообновления
- [ ] Fallback на ripgrep для неиндексированных путей
- [ ] Бенчмарки: latency с индексом vs без на реальных репо (Linux kernel, Chromium)

## Phase 5 — Cross-Reference Engine

**Impact**: средний (go-code частично покрывает) | **Effort**: высокий

Навигация по коду — главная фича OpenGrok/Sourcegraph.

- [ ] tree-sitter extraction определений и ссылок
- [ ] `POST /references` — find all usages of symbol
- [ ] `POST /definitions` — go-to-definition по позиции
- [ ] `POST /symbols` — список символов в файле/директории (outline)
- [ ] Интеграция с go-code `symbol_search` / `call_trace`

## Phase 6 — Data-Flow (Light)

**Impact**: очень высокий | **Effort**: очень высокий

Даже light-версия taint tracking — серьёзный дифференциатор.

- [ ] Intra-function data flow: отследить переменную от объявления до использования
- [ ] `POST /trace-variable` — path данных внутри функции
- [ ] Pattern: `$SOURCE -> ... -> $SINK` в structural queries
- [ ] Детекция: неиспользуемые переменные, unreachable assignments
- [ ] Базовый taint: пометить source (user input) → найти sink (SQL query)

---

## Приоритизация

| Phase | Impact | Effort | Ориентир |
|-------|--------|--------|----------|
| 1. Rewrite | ★★★★★ | ★★ | 1-2 дня |
| 2. Token-Aware | ★★★★★ | ★★★ | 2-3 дня |
| 3. Query DSL | ★★★ | ★★★ | 3-5 дней |
| 4. Index | ★★★★ | ★★★★★ | 1-2 недели |
| 5. Cross-Ref | ★★★ | ★★★★★ | 1-2 недели |
| 6. Data-Flow | ★★★★★ | ★★★★★★ | 2-4 недели |

## Принципы развития

1. **Каждая фаза — самостоятельная ценность**. Не блокировать Phase 2 на Phase 1.
2. **HTTP-first**. ox-codes — бэкенд; MCP, CLI, UI — отдельные слои (go-code = MCP).
3. **Zero-index по умолчанию**. Индекс — опция для scale, не требование.
4. **≤200 LOC на файл**. Новые фичи = новые модули, не раздувание существующих.
