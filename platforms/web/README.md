# Web-демо

Рабочая демонстрация движка в браузере поверх `abbrev-wasm`. Полностью офлайн:
словарь и языковая модель грузятся из assets, ничего не уходит на сервер.
Это же — самый дешёвый способ собрать **реальные данные** о том, как люди
сокращают слова (opt-in лог принятий), чтобы заменить гипотезу генератора.

## Сборка и запуск

```bash
cargo install wasm-pack          # один раз
./platforms/web/build.sh         # wasm + копии lexicon/lm в assets/
python3 -m http.server -d platforms/web 8000
# открыть http://localhost:8000/
```

`build.sh` кладёт wasm-модуль в `platforms/web/pkg/` и копирует
`data/lexicons/ru-50k.tsv` → `assets/lexicon.tsv`, `data/lm/ru-lm.tsv` →
`assets/lm.tsv`. Обе папки в `.gitignore` — это артефакты сборки.

Примечание: `wasm-opt` (binaryen) тянется с GitHub при сборке; он отключён в
метаданных крейта (`wasm-opt = false`), поэтому модуль собирается офлайн —
неоптимизированный, но рабочий (~210 КБ). Для релиза włącz wasm-opt.

## Что показывает демо

- лента подсказок по мере ввода (порог ≥ 3 символов, как в ядре);
- тап по чипу — вставить лучшую форму; `▾` — попап форм слова (hold-список);
- контекст: предыдущие 1–2 слова идут в LM (`в првт` ≠ `ну првт`);
- обучение на выборе: `accept()` + история в `localStorage`,
  восстанавливается при загрузке;
- настройки: обучение on/off, контекст (LM) on/off;
- экспорт: история (TSV, формат `import_history`) и **лог принятий** (JSONL:
  сокращение, контекст, показанные варианты, выбор, ранг, из hold ли).

## Контракт движка (тот же, что у нативных оболочек)

```js
import init, { WasmEngine } from "./pkg/abbrev_wasm.js";
await init();
const engine = new WasmEngine(lexiconTsv);   // или new WasmEngine(undefined) — демо-словарь
engine.load_language_model(lmTsv);           // опционально
JSON.parse(engine.suggest_grouped_json("рбте", "по", 6)); // [{lemma, best, variants}]
engine.accept("рбте", "работе");
localStorage.setItem("abbrev.history", engine.export_history());
```

Проверено в Node на боевых данных (тот же биндинг, что в браузере):
`првт → привет`, `с другой стрны → стороны`,
`рбте → работе | hold: работу работа работы`.
