// Web demo of the abbreviation engine, on top of the wasm-bindgen surface
// (crates/abbrev-wasm). Everything runs offline in the browser; the only
// persistence is localStorage, and exports are manual — this is the
// privacy-respecting acceptance loop that will replace the synthetic
// generator's guesses with how people actually abbreviate.

import init, { WasmEngine } from "./pkg/abbrev_wasm.js";

const MIN_LEN = 3; // matches EngineConfig::min_input_len
const HISTORY_KEY = "abbrev.history.tsv";
const LOG_KEY = "abbrev.acceptlog.jsonl";

const els = {
  status: document.getElementById("status"),
  strip: document.getElementById("strip"),
  editor: document.getElementById("editor"),
  learn: document.getElementById("opt-learn"),
  context: document.getElementById("opt-context"),
  log: document.getElementById("opt-log"),
  exportHistory: document.getElementById("export-history"),
  exportLog: document.getElementById("export-log"),
  clear: document.getElementById("clear-data"),
};

let engine = null;

async function fetchText(url) {
  const res = await fetch(url);
  if (!res.ok) throw new Error(`${url}: ${res.status}`);
  return res.text();
}

async function boot() {
  try {
    await init();
    els.status.textContent = "Загрузка словаря…";
    const [lexicon, lm] = await Promise.all([
      fetchText("./assets/lexicon.tsv"),
      fetchText("./assets/lm.tsv").catch(() => null),
    ]);
    engine = new WasmEngine(lexicon);
    if (lm) engine.load_language_model(lm);
    const saved = localStorage.getItem(HISTORY_KEY);
    if (saved) engine.import_history(saved);
    els.status.textContent = lm
      ? "Готово — словарь и языковая модель загружены."
      : "Готово — словарь загружен (без LM).";
    els.editor.disabled = false;
    els.editor.focus();
  } catch (e) {
    els.status.textContent = `Ошибка загрузки: ${e.message}. Соберите демо (см. README) и запустите HTTP-сервер.`;
    els.status.classList.add("error");
  }
}

// Plain Russian word (the engine ignores anything else).
const isWordChar = (c) => /[а-яёА-ЯЁ-]/.test(c);

// Current word being typed = run of word chars ending at the caret, plus the
// up-to-two previous words as left context.
function caretWord() {
  const text = els.editor.value;
  const caret = els.editor.selectionStart;
  let start = caret;
  while (start > 0 && isWordChar(text[start - 1])) start--;
  const word = text.slice(start, caret);
  // Extract clean Russian-word tokens for context (drop punctuation), so
  // "с другой, стрны" still feeds "другой" to the LM, not "другой,".
  const before = text.slice(0, start).match(/[а-яёА-ЯЁ-]+/g) || [];
  const context = before.slice(-2).join(" ");
  return { word, start, end: caret, context };
}

function render() {
  els.strip.innerHTML = "";
  if (!engine) return;
  const { word, context } = caretWord();
  if (word.length < MIN_LEN) return;
  const ctx = els.context.checked ? context : "";
  let groups;
  try {
    groups = JSON.parse(engine.suggest_grouped_json(word, ctx, 6));
  } catch {
    return;
  }
  groups.forEach((g, i) => els.strip.appendChild(chip(g, i, word, context)));
}

function chip(group, rank, shorthand, context) {
  const wrap = document.createElement("span");
  wrap.className = "chip";

  const main = document.createElement("button");
  main.className = "chip-main";
  main.textContent = group.best.form;
  main.title = `лемма: ${group.lemma}`;
  main.addEventListener("click", () =>
    choose(group.best.form, shorthand, context, rank, false),
  );
  wrap.appendChild(main);

  if (group.variants.length) {
    const forms = document.createElement("button");
    forms.className = "chip-forms";
    forms.textContent = "▾";
    forms.title = "формы слова";
    forms.addEventListener("click", (ev) => {
      ev.stopPropagation();
      openForms(forms, group, shorthand, context);
    });
    wrap.appendChild(forms);
  }
  return wrap;
}

function openForms(anchor, group, shorthand, context) {
  closePopup();
  const pop = document.createElement("div");
  pop.className = "popup";
  pop.id = "forms-popup";
  for (const form of [group.best.form, ...group.variants]) {
    const b = document.createElement("button");
    b.textContent = form;
    b.addEventListener("click", () => choose(form, shorthand, context, 0, true));
    pop.appendChild(b);
  }
  document.body.appendChild(pop);
  const r = anchor.getBoundingClientRect();
  pop.style.left = `${window.scrollX + r.left}px`;
  pop.style.top = `${window.scrollY + r.bottom + 4}px`;
  setTimeout(() => document.addEventListener("click", closePopup, { once: true }), 0);
}

function closePopup() {
  document.getElementById("forms-popup")?.remove();
}

// Insert the chosen form, teach the engine, and log the acceptance locally.
function choose(form, shorthand, context, rank, fromHold) {
  closePopup();
  const { start, end } = caretWord();
  const text = els.editor.value;
  els.editor.value = text.slice(0, start) + form + " " + text.slice(end);
  const caret = start + form.length + 1;
  els.editor.setSelectionRange(caret, caret);
  els.editor.focus();

  if (els.learn.checked && engine) {
    engine.accept(shorthand, form);
    localStorage.setItem(HISTORY_KEY, engine.export_history());
  }
  // The acceptance log is a separate, explicit opt-in (off by default);
  // disabling learning must never leave logging silently running.
  if (els.log.checked) {
    logAcceptance({ shorthand, form, context, rank, fromHold });
  }
  render();
}

function logAcceptance(event) {
  const line = JSON.stringify({ ts: Date.now(), ...event });
  const prev = localStorage.getItem(LOG_KEY) || "";
  localStorage.setItem(LOG_KEY, prev + line + "\n");
}

function download(name, text, type, { bom = false } = {}) {
  // Declare UTF-8 explicitly, and for human-inspected files prepend a BOM
  // so editors/Excel don't misdetect Cyrillic as a legacy codepage (the
  // ASCII-heavy JSONL log is otherwise guessed as cp1251 → mojibake). The
  // history TSV gets no BOM: it is re-imported by the engine and a leading
  // BOM would corrupt the first field.
  const blob = new Blob([(bom ? "\uFEFF" : "") + text], {
    type: `${type};charset=utf-8`,
  });
  const url = URL.createObjectURL(blob);
  const a = document.createElement("a");
  a.href = url;
  a.download = name;
  a.click();
  URL.revokeObjectURL(url);
}

els.editor.addEventListener("input", render);
els.editor.addEventListener("click", render);
els.editor.addEventListener("keyup", (e) => {
  if (["ArrowLeft", "ArrowRight", "Home", "End"].includes(e.key)) render();
});
els.exportHistory.addEventListener("click", () =>
  download(
    "abbrev-history.tsv",
    localStorage.getItem(HISTORY_KEY) || "",
    "text/tab-separated-values",
  ),
);
els.exportLog.addEventListener("click", () =>
  download(
    "abbrev-acceptlog.jsonl",
    localStorage.getItem(LOG_KEY) || "",
    "application/x-ndjson",
    { bom: true },
  ),
);
els.clear.addEventListener("click", () => {
  if (!confirm("Удалить локальную историю и лог принятий?")) return;
  localStorage.removeItem(HISTORY_KEY);
  localStorage.removeItem(LOG_KEY);
  if (engine) engine.import_history("");
  els.status.textContent = "Локальные данные очищены.";
});

boot();
