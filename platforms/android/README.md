# Android IME (первая цель)

Тонкая Kotlin-оболочка над `abbrev-ffi`. Здесь живёт Gradle-проект с
`InputMethodService`; движок — готовый `.so` + сгенерированный Kotlin-биндинг.

## Сборка движка

```bash
# 1. Таргеты
rustup target add aarch64-linux-android armv7-linux-androideabi x86_64-linux-android

# 2. .so (удобнее всего через cargo-ndk)
cargo install cargo-ndk
cargo ndk -t arm64-v8a -t armeabi-v7a -t x86_64 \
  -o platforms/android/app/src/main/jniLibs build -p abbrev-ffi --release

# 3. Kotlin-биндинги из библиотеки
cargo run -p uniffi-bindgen-cli --features=uniffi/cli -- \
  generate --library target/release/libabbrev_ffi.so \
  --language kotlin --out-dir platforms/android/app/src/main/kotlin
```

(Команду bindgen можно оформить отдельным bin-крейтом или gradle-таском —
см. документацию UniFFI.)

## Контракт оболочки

```kotlin
class AbbrevImeService : InputMethodService() {
    private lateinit var engine: AbbrevEngine

    override fun onCreate() {
        super.onCreate()
        val lexicon = assets.open("lexicon.tsv").bufferedReader().readText()
        engine = AbbrevEngine.fromLexiconTsv(lexicon)
        historyFile().takeIf { it.exists() }?.let { engine.importHistory(it.readText()) }
    }

    // на каждое изменение композиции:
    fun onComposingChanged(input: String, previousWords: List<String>) {
        val suggestions = engine.suggest(input, previousWords, 5u)
        suggestionStrip.render(suggestions)   // form + score; long-press → formsOfLemma(lemma)
    }

    fun onSuggestionAccepted(input: String, s: Suggestion) {
        currentInputConnection.commitText(s.form, 1)
        engine.accept(input, s.form)
    }

    override fun onFinishInput() {
        historyFile().writeText(engine.exportHistory())
        super.onFinishInput()
    }
}
```

Обязанности оболочки (и только они): раскладка и клавиши, строка подсказок,
вставка текста, чтение лексикона из assets, персистентность блоба истории,
экран настроек (мин. длина, чёрный/белый списки, выключатель обучения).
Никакой лингвистики на стороне Kotlin.

Манифест IME-сервиса объявляется без `INTERNET`-разрешения — офлайн-гарантия
проверяема ревью манифеста.
