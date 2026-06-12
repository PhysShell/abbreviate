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

# приёмочный бенчмарк (22 кейса, регрессия контракта)
cargo run -p abbrev-cli -- bench data/bench/basic.tsv

# честный бенчмарк: 20k сгенерированных сокращений на лексиконе 48k форм
cargo run --release -p abbrev-cli -- gen --lexicon data/lexicons/ru-50k.tsv \
    --count 20000 --seed 42 -o /tmp/gen.tsv
cargo run --release -p abbrev-cli -- bench /tmp/gen.tsv \
    --lexicon data/lexicons/ru-50k.tsv

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

Ядро с пятью слоями работает; на реальном лексиконе из 48k словоформ и 20 000
сгенерированных сокращений — **top-1 74.9%, top-3 89.3%, p95 ≈ 4 мс,
≈ 35 МБ** (подробности и анализ провалов — в
[docs/BENCHMARKS.md](docs/BENCHMARKS.md); флагманский сценарий «скелет +
окончание» — 93.6% top-1, опечатки закрыты SymSpell delete-индексом:
74.9% top-3).
Боевой лексикон лемматизирован (pymorphy3/OpenCorpora: 18 636 лемм), так что
двухуровневая лента работает на реальных данных:
`abbrev suggest рбте --grouped --lexicon data/lexicons/ru-50k.tsv` →
`работе | hold: работу работа работы…`. FFI/WASM-обвязки собираются.
Следующие шаги — контекстная модель, Android-оболочка. Дорожная карта — в
конце ARCHITECTURE.md.
