# Web-оболочка

Самый дешёвый способ показать движок и собирать фидбек: статическая страница
с полем ввода и строкой подсказок поверх `abbrev-wasm`.

```bash
cargo install wasm-pack
wasm-pack build crates/abbrev-wasm --target web --release
```

```js
import init, { WasmEngine } from "./pkg/abbrev_wasm.js";

await init();
const engine = new WasmEngine(undefined);            // демо-лексикон
// const engine = new WasmEngine(await (await fetch("lexicon.tsv")).text());

const suggestions = JSON.parse(engine.suggest_json("првт", "", 5));
// [{form: "привет", lemma: "привет", score: ...}, ...]

engine.accept("првт", "привет");                     // персонализация
localStorage.setItem("abbrev-history", engine.export_history());
```

История хранится в `localStorage` (или IndexedDB) — движок отдаёт/принимает
непрозрачный блоб, как и на остальных платформах. Тот же код пригоден для
расширения браузера (content script на полях ввода).
