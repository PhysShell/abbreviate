# iOS Keyboard Extension

Тонкая Swift-оболочка над `abbrev-ffi` (staticlib + Swift-биндинги UniFFI).

```bash
rustup target add aarch64-apple-ios aarch64-apple-ios-sim
cargo build -p abbrev-ffi --release --target aarch64-apple-ios
# Swift-биндинги: uniffi-bindgen generate --library ... --language swift
```

Особенности платформы, влияющие на план:

* Keyboard Extension живёт в жёстком лимите памяти (~60 МБ) — это главный
  аргумент за компактный бинарный формат лексикона (ADR-0004 v1) перед
  релизом iOS-оболочки.
* «Full Access» не требуется: движок офлайн, история хранится в app group
  контейнере.
* Контракт тот же, что у Android: `suggest` на изменение композиции,
  `accept` по принятию, `exportHistory` при уходе в фон.

Оболочка ставится в очередь после Android MVP.
