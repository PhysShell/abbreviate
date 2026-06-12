# abbreviate — русская аббревиатурная IME

Кросс-платформенная клавиатура/IME, восстанавливающая полные русские слова из
сокращённого ввода: `првт → привет`, `тстрние → тестирование`. Полностью
офлайн, приватно, с обучением на выборе пользователя.

**Архитектура:** одно Rust-ядро (`abbrev-core`, sans-IO, ноль зависимостей) +
тонкие платформенные оболочки (Android IME, iOS, десктоп, web) через UniFFI и
WASM. Подробно — в [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md), решения — в
[docs/adr/](docs/adr/), исследовательская база — в
[docs/RESEARCH.md](docs/RESEARCH.md).

## Быстрый старт

```bash
# подсказки для сокращения (встроенный демо-лексикон)
cargo run -p abbrev-cli -- suggest првт

# интерактивный REPL; `!1` принимает первый вариант (движок учится)
cargo run -p abbrev-cli -- repl

# офлайн-бенчмарк: top-1, top-3, латентность
cargo run -p abbrev-cli -- bench data/bench/basic.tsv

# тесты и линт
cargo test --workspace
cargo clippy --workspace --all-targets
```

## Карта репозитория

| Путь | Что это |
|---|---|
| `crates/abbrev-core` | движок: лексикон, индексы (скелетный/префиксный/суффиксный), взвешенный ред. дистанс, ранжирование, персонализация |
| `crates/abbrev-ffi` | UniFFI-биндинги → Kotlin (Android), Swift (iOS), Python |
| `crates/abbrev-wasm` | wasm-bindgen → web и webview-десктоп |
| `crates/abbrev-cli` | dev-CLI: `suggest`, `repl`, `bench` |
| `tools/lexicon-builder` | офлайн-конвейер сборки лексикона (OpenCorpora/НКРЯ → TSV) |
| `platforms/*` | платформенные оболочки (Android — первая цель) |
| `data/bench` | бенчмарк-наборы `вход → ожидание` |

## Состояние

Каркас: ядро с пятью слоями работает (22+ тестов, бенчмарк на сид-наборе —
top-3 100%, p95 ≈ 1 мс на демо-лексиконе), FFI/WASM-обвязки собираются.
Следующие шаги — импортёр OpenCorpora, тюнинг весов на большом лексиконе,
Android-оболочка. Дорожная карта — в конце ARCHITECTURE.md.
