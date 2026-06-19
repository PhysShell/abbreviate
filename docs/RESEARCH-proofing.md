# Корректоры текста: что переносимо в офлайн-IME (research-дайджест)

Конспект многоисточникового разбора: как устроена коррекция в MS Word и
открытых аналогах, и что из этого реально переносимо в **офлайн, on-device**
русскую «аббревиатурную» клавиатуру (символьный генератор кандидатов + лёгкий
линейный ранкер + биграм-LM; без INTERNET; без нейросети в hot-path; LLM только
офлайн-учитель; precision-first — подсказка, не молчаливая автозамена).

Источники приведены ссылками по месту; где данные тонкие/спорные — помечено.

## TL;DR

- «Магия» Word — четыре слоя за 30 лет, и лучший (Editor) **облачный**.
- Твой текущий рецепт — **именно то, чем выиграли русский шаред-таск
  SpellRuEval** (edit-distance + фонетика + n-gram реранк ≈75% F1).
- **F0.5 / precision-first** — норма литературы, а не локальная причуда.
- Пунктуация — единственное место, где честно нужна **крошечная нейронка**
  (~30 МБ); чистыми правилами русскую запятую не закрыть.

---

## Фронт 1. Как устроен Word

1. **Красная волна (орфография)** — словарь + Дамерау-Левенштейн. Текстбук.
   ([MS blog 2006](https://learn.microsoft.com/en-us/archive/blogs/correcteurorthographiqueoffice/contextual-spelling-in-the-2007-microsoft-office-system))
2. **Синяя волна (контекстная, their/there)** — Word 2007: курируемые
   confusion-sets + статистическая контекст-модель, тюнинг на высокую
   precision/низкий recall (~96%/40%).
   ([MS naturallanguage](https://learn.microsoft.com/en-us/archive/blogs/naturallanguage/an-academic-evaluation-of-the-office-2007-contextual-spelling-checker))
   - ⚠️ Распространённое заблуждение (которое я сам сперва повторил): что фичу
     питали «веб-n-граммы». Microsoft Web N-gram Service был **поисковым** (Bing),
     связь с внутрифичей Word **чисто не документирована** — вероятно, конфляция
     двух разных проектов. Механизм = confusion-sets + большая LM на корпусе.
3. **Зелёная волна (грамматика) = NLPWin** — рукописный широкоохватный парсер
   MSR, старт 1991 (Йенсен, Хайдорн), **десятки лет лингвистов**, 7 языков, в
   Word с 1997. Ядро проприетарное, *подход* (PLNLP, Logical Form) опубликован.
   ([NLPWin](https://www.microsoft.com/en-us/research/project/nlpwin/))
4. **Microsoft Editor (сегодня)** — трансформерные seq2seq, **в облаке**
   (Aggressive Decoding). Rewrite/clarity/style — только сервер, по подписке.
   ([Zero-COGS](https://www.microsoft.com/en-us/research/blog/achieving-zero-cogs-with-microsoft-editor-neural-grammar-checker/))

**Контрпример к «нейронку нельзя на устройство»:** MSR **EdgeFormer** —
on-device seq2seq, **~9M параметров int8, <50 МБ RAM, ~100 мс**, тестирован на
GEC, публичный чекпойнт (EMNLP 2022). ([arXiv:2202.07959](https://arxiv.org/abs/2202.07959))

> **Переносимо:** красная волна целиком; срез синей (confusion-set + малая
> локальная LM); сам факт, что компактная нейронка влезает в 50 МБ/100 мс.
> **Не переносимо:** NLPWin, облачный Editor, веб-масштабная статистика Bing.

## Фронт 2. Hunspell (LibreOffice/Firefox/Chrome)

- `.dic` (основы+флаги) + `.aff` (аффиксы). Порядок саджестов: **REP →
  edit-distance (вкл. KEY = соседи по клавиатуре) → ngram → PHONE**.
  ([hunspell.5](https://www.mankier.com/5/hunspell),
  [Spylls](https://spylls.readthedocs.io/en/latest/hunspell/algo_suggest.html))
- **REP-таблица — статичный confusion-set** (`REP что замена`, якоря `^/$`),
  приоритетные кандидаты. MAP = классы похожих букв. Чистые данные, on-device.
- Hunspell — спелл-чекер, **не анализатор**: лемм/граммем не отдаёт (для этого и
  нужен pymorphy3/OpenCorpora). Рус. словарь ~1.1 МБ.
  ([Debian hunspell-ru](https://packages.debian.org/sid/hunspell-ru))

> **Переносимо:** дизайн **REP-таблицы** (русские опечатки е/ё, и/й, удвоения как
> приоритетные кандидаты); порядок «confusion → edit → ngram»; аффиксное сжатие.
> **Не нужно:** сам движок (pymorphy3-лексикон богаче), PHONE (русский фонемный).

## Фронт 3. LanguageTool — открытый офлайн-референс

- XML-паттерны (токен/лемма/POS/regex + antipatterns + `skip`), плюс Java-правила.
  ([dev overview](https://dev.languagetool.org/development-overview))
- N-gram детекция реально-словных ошибок: данные Google Books, **~8 ГБ, только
  EN/DE/FR/ES, нужен SSD, в комплект не входит**; пары — в `confusion_sets.txt`.
  ([n-gram docs](https://dev.languagetool.org/finding-errors-using-n-gram-data.html))
- Русский: **892 XML + 20 Java правил, но всего 2 confusion-пары**, активность
  низкая. Рантайм **JVM, 1+ ГБ RAM**. Лицензия **LGPL 2.1** → копировать
  правила/`confusion_sets.txt` = тянуть share-alike; нужен **clean-room**.

> **Переносимо:** *концепция* декларативных паттерн-правил (лёгкий матчер на Rust);
> формат списка confusion-пар.
> **Не переносимо:** JVM, 8-ГБ n-граммы (русского нет), полный POS-пайплайн;
> сами правила нельзя копировать (LGPL).

## Фронт 4. Пунктуация (русская запятая) — конфликт с принципом «no neural»

- SOTA = трансформер-теггер. **Запятая — самый трудный знак**: англ. comma
  F1 ≈40 против period ≈79.
  ([W-NUT 2020](https://aclanthology.org/2020.wnut-1.18.pdf),
  [survey](https://arxiv.org/pdf/2111.10746))
- **Чистыми правилами не закрыть.** LORuGEC (Dialogue 2025): даже 48
  формализованных правил — лучший открытый 7B LLM даёт лишь **50% F0.5**; многие
  правила «требуют семантики» (обороты, границы придаточных).
  ([LORuGEC](https://dialogue-conf.org/wp-content/uploads/2025/04/SorokinANasyrovaR.052.pdf))
- **Но крошечная модель влезает:** RUPunct_small / Silero — rubert-tiny2 класс,
  **~29M, ~30 МБ int8, реалтайм на CPU телефона**.
  ([RUPunct](https://huggingface.co/RUPunct/RUPunct_small),
  [Silero](https://github.com/snakers4/silero-models))

> **Переносимо:** гибрид — FST/правила для обязательных запятых + **маленький
> дистиллированный теггер** для неоднозначных; учитель-LLM генерит и правила, и
> студента, но в hot-path студент.
> **Не переносимо:** «чистые правила без модели» (посредственное качество). Это
> осознанное **исключение** из «no neural in hot-path».

## Фронт 5. Контекстная коррекция + confusion-sets — ты уже на этом пути

- Канон: **noisy-channel** (Kernighan/Church-Gale 1990); **тригам-реранк
  реально-словных ошибок** (Mays-Damerau-Mercer 1991) — флаг только если замена
  из confusion-set повышает n-gram вероятность предложения. Переоценка 2008:
  простой тригам-реранк бьёт WordNet-методы.
  ([MDM reconsidered](http://ftp.cs.toronto.edu/pub/gh/WilcoxOHearn-etal-2008.pdf))
- Confusion-set из: edit-distance + **клавиатурной близости** + фонетики/омофонов
  + частоты. (У нас `alphabet::keyboard_adjacent` уже есть.)
- On-device LM: мобильные клавиатурные LM **≤10 МБ**, ≤1.5M n-грамм, <20 мс;
  малые LM хуже на невиданном контексте.
  ([Federated n-gram LM](https://arxiv.org/pdf/1910.03432))

> **Переносимо:** почти всё. Это ровно наш биграм-LM (`w_ctx`) + конфьюжн-слой;
> рецепт MDM = confusion-swap с порогом по приросту LM.

## Фронт 6. Русский GEC — данные, метрики, валидация подхода

- **SpellRuEval/RUSpellRU** (Dialogue 2016, LiveJournal). **Победитель:
  edit-distance + фонетика + n-gram реранк, ≈75% F1** — прямая валидация нашей
  архитектуры.
  ([SpellRuEval](https://www.semanticscholar.org/paper/SpellRueval-:-the-FiRSt-Competition-on-automatiC-Shavrina-%D0%9C%D0%BE%D1%81%D0%BA%D0%B2%D0%B0/d3d9ba1161ecbe2d691a1d7385266e0806c3124d))
- **RULEC-GEC** (TACL 2019, L2-русский, M2-формат), **RU-Lang8**,
  **MultiGEC-2025**, **SAGE** (4 рус. набора, метрика **F0.5**).
  ([RULEC-GEC](https://github.com/arozovskaya/RULEC-GEC),
  [SAGE/EACL2024](https://aclanthology.org/2024.findings-eacl.10.pdf))
- **F0.5 — норма именно чтобы штрафовать overcorrection** (precision×2). LLM
  склонны переправлять даже под minimal-edit → консервативный suggestion-only
  дизайн (наш ROADMAP п.7).
- Глубокая грамматика (падеж/согласование/вид) — вне досягаемости малого n-gram
  реранка; орфография/опечатки — реалистичная офлайн-цель.
- 🔎 «GERA»/«MELABNLP» — **не подтверждены** (вероятно, перепутанные названия).
  RuCoLA — классификация приемлемости (детекция, не коррекция), полезна как гейт
  против overcorrection.

> **Переносимо:** весь оценочный аппарат — **F0.5 + раздельные detection/correction
> P/R** на RUSpellRU/MultidomainGold через M2-scorer; делает `harm_rate`
> (DIAGNOSTICS) измеримым стандартной метрикой.

---

## Что брать первым (рычаги с наибольшим выхлопом)

1. **Confusion-set слой (Hunspell-REP-стиль).** Статичная русская таблица
   типичных опечаток; кандидаты из edit-distance + `keyboard_adjacent` + фонетики;
   реранк существующим биграм-LM с порогом по приросту. On-device, дёшево, и это
   буквально рецепт-победитель SpellRuEval. **Самый высокий выхлоп.**
2. **Precision-first оценка как первоклассный артефакт.** Бенч на
   RUSpellRU/MultidomainGold, метрика **F0.5 + раздельно detection/correction**
   (M2-scorer). Превращает «не навреди» в число; стыкуется с `harm_rate`.
3. **Реально-словная коррекция по MDM** поверх п.1: флаг только когда confusion-swap
   поднимает вероятность предложения; консервативный порог; suggestion-only.
4. **Пунктуация — отдельный опциональный тир, честно с крошечной нейронкой.** Не
   чистые правила. Либо гибрид (FST на обязательные + теггер ~30 МБ), либо
   отложить. Осознанное исключение из «no neural in hot-path» (ROADMAP п.7
   правильно держит это как «другой продукт»).

**Стратегически:** не догонять облачный Editor. Преимущество проекта — задача,
которую Word не решает (аббревиатурный ввод). Заимствовать *технику* (confusion +
n-gram реранк, precision-дисциплина) и *данные через офлайн-учителя*.
