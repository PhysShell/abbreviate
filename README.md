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

# слой конвенциональных сокращений (сленг): спс→спасибо, мб→может быть
cargo run -p abbrev-cli -- bench data/bench/slang.tsv \
    --lexicon data/lexicons/ru-50k.tsv --shortcuts data/shortcuts/ru.tsv

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
| `platforms/web` | рабочее браузерное демо (WASM) + opt-in лог принятий |
| `platforms/{android,ios,desktop-tauri}` | оболочки-контракты (Android — первая нативная цель) |
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
Контекстная биграмная LM (субтитры OPUS, PMI) даёт **+13пп top-1** на
контекстных кейсах. Живой пример с committed-LM (`--lm data/lm/ru-lm.tsv`):
`стрны → страны`, но `с другой стрны → стороны`; `длми → для`, но
`за длми → делами`. Сценарий `ну првт → привет` / `в првт → приват`
закреплён юнит-тестом на синтетической LM — в субтитрах биграммы «в приват»
нет (см. ограничение домена в BENCHMARKS.md). Работает web-демо поверх WASM (`platforms/web`): лента подсказок,
hold-формы, обучение на выборе и opt-in лог принятий — самый дешёвый
способ начать собирать реальные данные вместо гипотезы генератора.
Следующие шаги — Android-оболочка, морфология. Дорожная карта — в конце ARCHITECTURE.md.
