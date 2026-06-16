# Android shell

Тонкая Kotlin-оболочка над `abbrev-ffi`. Вся лингвистика — в Rust-движке;
Kotlin только читает текст, показывает подсказки и вставляет выбор.

Здесь живут **две оболочки над одним швом**:

- **scratchpad** (`ScratchpadActivity`) — приложение для проверки «начинки»
  в своём `EditText`;
- **IME** (`ime/AbbrevImeService`) — аббревиатурная клавиатура: печатаешь в
  любом приложении, подсказки в полосе над клавишами.

Обе используют один и тот же `SuggestionController` + движок; различается только
`TextHost` (EditText против `InputConnection`) — ровно то, ради чего шов и
сделан.

## Архитектура (шов host ↔ движок)

```text
ScratchpadActivity ──implements── TextHost            (shell-specific, ~70 строк)
        │                            ▲
        │ refresh / accept           │ replaceTokenAtCursor / textBeforeCursor
        ▼                            │
SuggestionController ─────────► SuggestionPort ◄── UniffiSuggestionPort ─► AbbrevEngine (UniFFI)
   (host-agnostic, чистый,        (интерфейс,                                   .so + Kotlin-биндинг
    юнит-тестируемый)              фейк в тестах)
```

- **`controller/SuggestionController`** — без Android и без UniFFI: вырезает
  токен у курсора и левый контекст, дёргает порт, держит выбор (стрелки/цифры),
  коммитит через `TextHost`. Тестируется на чистой JVM (`SuggestionControllerTest`).
- **`engine/SuggestionPort`** — шов к движку; `UniffiSuggestionPort` — боевая
  реализация поверх сгенерированного биндинга (единственное место, где Kotlin
  касается `uniffi.abbrev_ffi`).
- **`host/TextHost`** — единственная часть, которую переписывают при смене
  оболочки (scratchpad `EditText` → IME `InputConnection` → accessibility
  `ACTION_SET_TEXT`). Контроллер и порт переезжают без изменений.

## Сборка

Движок (биндинг + `.so`) — это артефакты, они не коммитятся, а генерируются:

```bash
# 1. таргеты + инструменты (один раз)
rustup target add aarch64-linux-android armv7-linux-androideabi x86_64-linux-android
cargo install cargo-ndk            # нужен Android NDK (ANDROID_NDK_HOME)

# 2. сгенерировать Kotlin-биндинг + .so (по умолчанию arm64; ABIS="arm64-v8a x86_64" для эмулятора)
./platforms/android/gen-bindings.sh

# 3. собрать приложение
cd platforms/android && ./gradlew assembleDebug   # APK в app/build/outputs/apk/debug/
./gradlew testDebugUnitTest                        # юнит-тесты контроллера (без .so)
```

CI-джоб `android-app` делает ровно эти шаги (генерит биндинг, собирает arm64
`.so`, гоняет юнит-тесты + `assembleDebug`) и публикует APK артефактом — так
сборку Android-приложения видно прямо в CI.

Debug-сборки подписываются **закоммиченным** `app/debug.keystore` (обычный
debug-ключ, пароль `android`). Иначе каждый CI-прогон подписывал бы APK новым
ключом, и переустановка поверх прежней давала бы `INSTALL_FAILED_UPDATE_INCOMPATIBLE`
(приходилось бы удалять приложение). С фиксированным ключом `adb install -r app.apk`
встаёт поверх без удаления.

Бинды/`.so` генерируются через standalone `uniffi-bindgen` (`tools/uniffi-bindgen`):

```bash
cargo run -p uniffi-bindgen -- generate \
  --library target/release/libabbrev_ffi.so --language kotlin --no-format \
  --out-dir platforms/android/app/src/uniffi/kotlin
```

## Лексикон

Scratchpad грузит **реальные данные** из `/data` — единый источник для всех
платформ. Gradle-таск `bundleEngineData` копирует их в `app/src/main/assets/`
при сборке (как `platforms/web/build.sh` для веба; копии в `.gitignore`):

| из `/data`                | → asset         | роль                                   |
|---------------------------|-----------------|----------------------------------------|
| `lexicons/ru-50k.tsv`     | `lexicon.tsv`   | словарь (обязательно)                  |
| `lm/ru-lm.tsv`            | `lm.tsv`        | биграммная LM — контекст-ранжирование  |
| `shortcuts/ru.tsv`        | `shortcuts.tsv` | договорные сокращения (мб, ща)         |

`UniffiSuggestionPort.fromData(lexicon, lm, shortcuts)` поднимает движок; в
`ScratchpadActivity` это делается **в фоновом потоке** со статус-строкой
(≈11 МБ TSV на главном потоке = ANR). Если ассетов нет — фолбэк на
`UniffiSuggestionPort.demo()` (встроенный мини-словарь), чтобы приложение всегда
запускалось.

Данные раздувают debug-APK на несколько МБ (TSV хорошо жмётся в aapt) — для
спайки ок; для реального продукта стоит подумать про bin-формат/стрим-загрузку.

## IME-клавиатура

`ime/AbbrevImeService` — `InputMethodService`, реализующий тот же `TextHost`
поверх `InputConnection` (`deleteSurroundingText` + `commitText` — точечная
замена токена, без перезаписи всего поля). `SuggestionController` и движок —
как в scratchpad'е (загрузка через общий `EngineLoader` в фоновом потоке).
Манифест без `INTERNET` — офлайн-гарантия проверяема ревью манифеста.

Раскладка и поведение:

- ЙЦУКЕН; `⌫` — в правом конце нижнего буквенного ряда (подальше от системной
  «свернуть клавиатуру» в левом нижнем углу);
- **пробел** при непустой полосе подсказок вставляет верхнюю форму; сразу `⌫`
  после этого — откат к исходному сокращению (дальше `⌫` удаляет как обычно);
- **`EN`/`РУ`** — тоггл на латинскую QWERTY (печатать латиницу без смены
  системной раскладки);
- **`тр`** — транслит выделения кириллица→латиница (тупой побуквенный, регистр
  сохраняется): выдели текст → тап.

Включить на устройстве:

```text
Настройки → Система → Языки и ввод → Экранная клавиатура → Управление
  → включить «Abbrev» → сменить клавиатуру (значок в навбаре) на Abbrev
```

(или `adb shell ime enable com.physshell.abbreviate/.ime.AbbrevImeService`
и `ime set ...`). Затем печатай в любом поле: подсказки появятся в полосе над
клавишами, тап вставляет форму.

## Тест набора

`TestActivity` (кнопка «→ Тест набора» в scratchpad'е) — замер скорости:
целевой текст, отсчёт 3-2-1, таймер с первого символа, авто-стоп при совпадении.
Метрики: время, зн/мин, сл/мин, число правок (прокси нажатий) и метка активной
клавиатуры. Сравнение «обычная vs наша» — это два прогона с переключением
системной клавиатуры (двух IME одновременно не бывает). Тапы/выбор подсказок для
*системной* клавиатуры из приложения не видны — точные эти метрики есть в
веб-тестере (`platforms/web/test.html`).

## Дальше: accessibility

Третья оболочка — `AccessibilityService` поверх чужой клавиатуры (подсказки
оверлеем в любом приложении, вставка через `ACTION_SET_TEXT`). Тот же
`SuggestionController`; `TextHost` читает `AccessibilityNodeInfo` и пишет весь
текст узла. Пока не сделано.
