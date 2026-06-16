# Android shell

Тонкая Kotlin-оболочка над `abbrev-ffi`. Вся лингвистика — в Rust-движке;
Kotlin только читает текст, показывает подсказки и вставляет выбор.

Сейчас здесь живёт **scratchpad** — минимальное приложение для проверки
«начинки» на устройстве (биндинг грузится, движок ранжирует, вставка работает),
без обязательства строить полноценную клавиатуру. Тот же контроллер потом
поедет в IME/accessibility-оболочку — меняется только тонкий `TextHost`.

## Архитектура (шов host ↔ движок)

```
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

Бинды/`.so` генерируются через standalone `uniffi-bindgen` (`tools/uniffi-bindgen`):

```bash
cargo run -p uniffi-bindgen -- generate \
  --library target/release/libabbrev_ffi.so --language kotlin --no-format \
  --out-dir platforms/android/app/src/uniffi/kotlin
```

## Лексикон

Scratchpad стартует на встроенном demo-лексиконе (`UniffiSuggestionPort.demo()`)
— достаточно проверить весь цикл без ассетов. Чтобы гонять реальный словарь,
положи `ru-50k.tsv` в `app/src/main/assets/` и собери движок из него:

```kotlin
val tsv = assets.open("ru-50k.tsv").bufferedReader().readText()
UniffiSuggestionPort(AbbrevEngine.fromLexiconTsv(tsv))
```

## Дальше: IME / accessibility

Когда «начинка» устраивает — оболочка-IME реализует тот же `TextHost` поверх
`InputConnection` (точечно: `deleteSurroundingText` + `commitText`), а
`SuggestionController` остаётся как есть. Манифест IME-сервиса объявляется без
`INTERNET` — офлайн-гарантия проверяема ревью манифеста.

```kotlin
class AbbrevImeService : InputMethodService(), TextHost {
    private val controller = SuggestionController(UniffiSuggestionPort(/* engine */))

    override fun textBeforeCursor() =
        currentInputConnection.getTextBeforeCursor(64, 0)?.toString().orEmpty()

    override fun replaceTokenAtCursor(token: String, replacement: String) {
        currentInputConnection.deleteSurroundingText(token.length, 0)
        currentInputConnection.commitText(replacement, 1)
    }
}
```
