# Диагностика и жизненный цикл коррекции

Два контура, которые легко спутать с косметикой, но они — фундамент:

1. **Жизненный цикл коррекции** — как сигнал «пользователь принял подсказку»
   превращается в обучающую улику. Тап ≠ доволен.
2. **Приватный логгинг и диагностика** — как пользователь (и разработчик)
   видит, отлаживает и отправляет проблему, не сливая при этом набранный текст.

Почему это закладывается **рано**, а не «когда-нибудь потом»: семантика сигнала
необратима. Логи и счётчики, накопленные с неверной семантикой («тапнул» = «успех»),
бесполезны для последующего обучения ранжирования — их придётся выбросить. А для
клавиатуры цена плохого UX отказа особенно высока: если она начала тупо
автозаменять, пользователь не пишет багрепорт, он удаляет приложение.

## Базовые принципы (несущие, не нарушать)

* **Тап ≠ доволен.** Сырое «принято» — это испорченный сигнал: пользователь мог
  тапнуть, увидеть результат, разочароваться и стереть. Считаем раздельно
  `confirmed` и `reverted`, обучаемся на `confirmed`.
* **Только суммируемые счётчики (CRDT-friendly).** История — сумма счётчиков на
  пару, поэтому два устройства сводятся сложением, без разрешения конфликтов.
  Несуммируемое состояние («последний выбор» без счётчиков) ломает это свойство
  и **не добавляется**. Это же ограничивает ядро: временны́х меток в `PairStats`
  нет — логика «исключение после N откатов за 7 дней» живёт в оболочке, не в ядре.
* **Приватность по умолчанию.** Сырой набранный текст в логи **не попадает**
  никогда, кроме явного debug-opt-in с предупреждением. Это клавиатура — она
  видит слишком много. Диагностика, требующая сырого текста, — уже не
  диагностика.
* **Sans-IO ядро ничего не логирует и не знает времени.** Ядро (`abbrev-core`,
  ADR-0002) не трогает файлы, сеть и часы. Всё, что про время, persistence,
  `CorrectionSession`, `InputConnection` и debug-bundle, — обязанность
  платформенной оболочки. Ядро выдаёт лишь `confirm`/`reject` и суммируемый блоб.

---

## Часть A. Жизненный цикл коррекции

### A.1 Состояния

Принятие — **pending**, а не **final**:

```
Shown
  └─ AcceptedByTap | Autoreplaced
        └─ PendingCommit
              ├─ Confirmed            (пережило окно подтверждения)
              ├─ RevertedByUndo       (нажат undo-чип)
              ├─ DeletedAfterAccept   (backspace сразу после вставки)
              ├─ EditedAfterAccept    (курсор вернулся, replacement изменён)
              └─ Expired              (нет данных — best-effort, не считаем за успех)
```

Статистика различает: `shown`, `tapped`, `confirmed`, `reverted`,
`editedAfterAccept`, `ignored`. Один счётчик `accepted++` — это аналитика
уровня «считать визит к стоматологу показателем любви к сверлению».

### A.2 Когда принятие считать подтверждённым

Не сразу. `Confirmed`, если **всё** выполнено:

* пользователь ввёл пробел/пунктуацию после replacement;
* не откатил замену в пределах pending-окна;
* не вернулся курсором редактировать это слово;
* replacement всё ещё находится рядом с ожидаемой позицией.

Pending-окно: **5–15 секунд** или **до следующих 2–3 слов**. Только после
подтверждения `confirmed_accept_count += 1`. При откате —
`rejected_after_accept_count += 1`, и (важно) **не** повышать user-prior, **не**
считать за успешное обучение, **снизить** score пары.

### A.3 Сигналы отмены (best-effort на Android)

В порядке убывания чистоты сигнала:

1. **Undo-чип** — `RevertedByUndo`. Самый чистый сигнал, точнее любого гадания
   по backspace. Чип показывается после каждой автозамены/tap-commit.
2. **Backspace сразу после вставки** — `DeletedAfterAccept`. Удалил только что
   вставленный replacement → почти точно негатив.
3. **Возврат курсора и правка слова** — `EditedAfterAccept`. Если оболочка
   читает surrounding text и видит, что replacement изменён
   (`GitHub → гитхаб`, `тестирование → тестирования`).
4. **Replacement исчез** — `DeletedAfterAccept` / `UnknownEdit`.

На Android это работает не везде идеально: `InputConnection` в части приложений
формально существует, фактически бесполезен. Поэтому **best-effort**: при
отсутствии данных — `Expired`, не «успех».

### A.4 `CorrectionSession` (модель данных оболочки)

Принадлежит **платформенной оболочке** (Android), не sans-IO ядру. Ядро видит
только итог — `confirm`/`reject`.

```kotlin
data class CorrectionSession(
    val id: String,
    val original: String,
    val replacement: String,
    val inputTokenHash: String,
    val replacementHash: String,
    val startCursor: Int?,
    val endCursor: Int?,
    val timestamp: Long,
    val source: CandidateSource,
    val candidateScore: Float,
    val status: CorrectionStatus,
)

enum class CorrectionStatus {
    Pending, Confirmed,
    RevertedByUndo, EditedAfterAccept, DeletedAfterAccept,
    CursorReturnedToEdit, Expired,
}
```

### A.5 Влияние на обучение

Для `UserHistory` хранятся **раздельные** счётчики, а не `skeleton → count`:

```
UserPairStats { shown, tapped, confirmed, reverted, editedAfterAccept,
                lastAcceptedAt?, lastRejectedAt? }
```

Score считается по `confirmed`, не по `tapped`. Метрики качества пары:

```
accept_quality = confirmed / max(tapped, 1)
rejection_rate = reverted / max(tapped, 1)
```

Пример: `тя → тебя` (shown 100, tapped 50, confirmed 48, reverted 2) — хороший
кандидат. `гитхаб → GitHub` (shown 100, tapped 30, confirmed 12, reverted 18) —
пользователь часто пробует и откатывает, агрессивно поднимать не надо.

> **Текущая реализация в ядре** (`history.rs`) — минимальный, но достаточный
> срез этой модели: на пару хранятся `confirmed`/`reverted` (без `shown`,
> `tapped`, временны́х меток — они требуют не-суммируемого состояния и/или часов,
> т.е. оболочки). Приор — *чистая* улика
> `2·(ln(1+confirmed) − ln(1+reverted)) + 0.5·ln(1+form_popularity)`: уходит в
> минус, когда откаты перевешивают, поэтому многократно отклонённая пара тонет.

### A.6 Заслуженные исключения после повторных откатов

При высоком `rejection_rate` — понизить кандидата, **предложить исключение**,
для автозамены — отключить её для этой пары.

```
Вы часто отменяете замену «хрюндель» → «хрендель».
Не исправлять «хрюндель»?
[Не исправлять] [Только здесь] [Нет]
```

Триггер: `reverted_count(pair) >= 3` за последние 7 дней, **или**
`rejection_rate(pair) > 0.5` при `tapped >= 5`. Scope: `global` /
`per-language` / `per-app` / `per-context` (для клавиатуры особенно полезно
«Не исправлять в Telegram»). Система может переспросить позже (затухание).

> Требует временны́х меток и persistence → логика оболочки. В ядре пока нет.

### A.7 Undo/original всегда часть сессии коррекции

После любой автозамены или tap-commit — чип возврата:

```
гитхаб → GitHub
[↶ гитхаб] [Не исправлять]
```

Нажатие → `RevertedByUndo`. Это **лучший обучающий сигнал**, точнее догадок по
backspace. В web-демо уже реализовано: tap-commit ставит `pendingUndo`, набор
дальше = `confirmed`, нажатие чипа = `reverted`.

### A.8 Метрики

Для каждого provider: `shown_count`, `tap_accept_count`,
`confirmed_accept_count`, `reverted_count`, `edited_after_accept_count`,
`ignored_count`.

Качество: `confirmation_rate = confirmed/tapped`, `revert_rate = reverted/tapped`,
`edit_after_accept_rate = edited_after_accept/tapped`.

Для автозамены отдельно: `auto_replace_count`, `auto_replace_confirmed`,
`auto_replace_reverted`, `auto_replace_disabled_by_user`.

Главная метрика:

```
harm_rate = reverted_or_edited / accepted_or_autoreplaced
```

Высокий `harm_rate` означает, что фича **вредная** — не «спорная», не «требует
улучшений», а вредная. Честность здесь дешевле поддержки.

---

## Часть B. Приватный логгинг и диагностика (оболочка)

Для клавиатуры просмотр/отправка лога — must-have:

```
Settings → Diagnostics
    View latest logs
    Export debug bundle
    Send crash report
    Clear logs
```

### B.1 Что логировать (privacy-preserving)

app version · device model · Android version · keyboard layout · current
language profile · enabled features · exception stacktrace · last engine stage ·
latency metrics · candidate counts · artifact versions · memory usage.

Событие движка — хеши/длины/метрики, **не** сырой текст:

```json
{
  "event": "suggest_failed",
  "input_len": 7,
  "input_hash": "sha256:…",
  "language": "ru",
  "providers": ["word", "short", "lm"],
  "candidate_count": 12,
  "top_score": 0.91,
  "duration_ms": 8,
  "exception": "IndexOutOfBoundsException"
}
```

### B.2 Что НИКОГДА не логировать

Сырой набранный текст / контекст / введённые слова:

```json
{ "typed_text": "мой пароль от банка…" }   // нет.
```

### B.3 Два режима

* **Обычный (по умолчанию):** без сырого текста, без контекста, без введённых
  слов — только хеши/длины/метрики/stacktrace.
* **Debug opt-in:** пользователь явно включает, с предупреждением.

```
Advanced diagnostics
[ ] Include typed snippets in debug logs
    Может содержать текст, который вы вводили. Включайте только для отладки.
```

Raw traces — допустимы только в dev-сборке; в продакшене — лишь по opt-in.

### B.4 Debug bundle

Экспорт одним файлом, **с обязательным preview** перед отправкой
(«Посмотреть, что будет отправлено» — это бьётся с privacy-позиционированием):

```
debug-bundle.zip
    crash.log
    app-info.json
    engine-config.json
    artifact-versions.json
    recent-events.jsonl
    performance.json
```

---

## Статус реализации

| Пункт | Где живёт | Статус |
|---|---|---|
| Раздельные `confirmed`/`reverted` на пару | `abbrev-core/history.rs` | ✅ ядро |
| Net-prior (уходит в минус на откатах) | `history.rs::prior` | ✅ ядро |
| Суммируемость / `merge` (база синка) | `history.rs::merge` | ✅ ядро + тесты |
| `confirm`/`reject` в API/FFI/WASM | `engine.rs`, `abbrev-ffi`, `abbrev-wasm` | ✅ |
| Undo-чип, confirm-по-набору-дальше | `platforms/web/app.js` | ✅ web-демо |
| Pending-окно подтверждения (тайминг) | `app.js` | ✅ web-демо |
| `CorrectionSession` / детекция отмены через `InputConnection` | Android-оболочка | ⏳ отложено (с Android) |
| Полный `UserPairStats` (`shown`/`tapped`/метки времени) | оболочка | ⏳ отложено |
| Заслуженные per-pair исключения (N откатов / 7 дней) | оболочка | ⏳ отложено |
| Per-provider метрики, `harm_rate` | оболочка/телеметрия (локальная) | ⏳ отложено |
| Приватный логгинг, debug-bundle, Diagnostics UI | Android-оболочка | ⏳ зафиксирован принцип, не реализовано |

Отложенное привязано к Android-оболочке сознательно: без реального
`InputConnection`, часов и persistence этим вещам негде жить, а sans-IO ядро их
по дизайну не несёт (ADR-0002).

## Связанные документы

* [ROADMAP.md](ROADMAP.md) — приоритеты (контур обратной связи — п. 1, 2, 4).
* [ARCHITECTURE.md](ARCHITECTURE.md) — §4.4 (ранжирование/история), §6 (Android-оболочка).
* `crates/abbrev-core/src/history.rs` — реализация и инварианты в doc-комментариях.
