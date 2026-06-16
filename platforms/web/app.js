// Web demo of the abbreviation engine, on top of the wasm-bindgen surface
// (crates/abbrev-wasm). Everything runs offline in the browser; the only
// persistence is localStorage, and exports are manual — this is the
// privacy-respecting acceptance loop that will replace the synthetic
// generator's guesses with how people actually abbreviate.

import init, { WasmEngine } from "./pkg/abbrev_wasm.js";

// 2, not 3: conventional shortcuts (мб, ща) are shorter than the fuzzy
// minimum; the engine returns only exact shortcut matches below length 3.
const MIN_LEN = 2;
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
// Last choice that can still be reverted with one tap. It is deliberately
// not written to history until the user types on/leaves the page: a tap is
// only confirmed once it is kept.
let pendingUndo = null;
// Snapshot of what the strip currently offers, so the keyboard handlers can
// act (digit = pick, arrows = navigate) without re-deriving it.
let current = { word: "", context: "", groups: [] };

async function fetchText(url) {
  const res = await fetch(url);
  if (!res.ok) throw new Error(`${url}: ${res.status}`);
  return res.text();
}

async function boot() {
  try {
    await init();
    els.status.textContent = "Загрузка словаря…";
    const [lexicon, lm, shortcuts, paradigms] = await Promise.all([
      fetchText("./assets/lexicon.tsv"),
      fetchText("./assets/lm.tsv").catch(() => null),
      fetchText("./assets/shortcuts.tsv").catch(() => null),
      fetchText("./assets/hold-groups.tsv").catch(() => null),
    ]);
    engine = new WasmEngine(lexicon);
    if (lm) engine.load_language_model(lm);
    if (shortcuts) engine.load_shortcuts(shortcuts);
    if (paradigms) engine.load_paradigms(paradigms);
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
  closePopup();
  els.strip.innerHTML = "";
  current = { word: "", context: "", groups: [] };
  if (!engine) return;
  const { word, context } = caretWord();
  // Right after a choice the word is empty (caret sits past the inserted
  // space): offer a one-tap undo. Tapping it is the reverted signal —
  // confirmed != tapped, the whole point of the lifecycle.
  if (word.length === 0 && pendingUndo) {
    els.strip.appendChild(undoChip());
    return;
  }
  if (word.length < MIN_LEN) return;
  const ctx = els.context.checked ? context : "";
  let groups;
  try {
    groups = JSON.parse(engine.suggest_grouped_json(word, ctx, 6));
  } catch {
    return;
  }
  current = { word, context, groups };
  groups.forEach((g, i) => els.strip.appendChild(chip(g, i, word, context)));
}

function chip(group, rank, shorthand, context) {
  const wrap = document.createElement("span");
  wrap.className = "chip";

  const main = document.createElement("button");
  main.className = "chip-main";
  main.type = "button";
  // Roving tabindex: only the first chip is in the tab order; arrows move
  // focus among the rest (managed in the strip keydown handler).
  main.tabIndex = rank === 0 ? 0 : -1;
  main._group = group;
  main._rank = rank;
  if (rank < 9) {
    const badge = document.createElement("span");
    badge.className = "chip-badge";
    badge.textContent = rank + 1;
    badge.setAttribute("aria-hidden", "true");
    main.appendChild(badge);
  }
  main.appendChild(document.createTextNode(group.best.form));
  main.title = `лемма: ${group.lemma} — клавиша ${rank + 1}`;
  main.addEventListener("click", () =>
    choose(group.best.form, shorthand, context, rank, false),
  );
  wrap.appendChild(main);

  if (group.variants.length) {
    const forms = document.createElement("button");
    forms.className = "chip-forms";
    forms.type = "button";
    forms.tabIndex = -1;
    forms.textContent = "▾";
    forms.title = "формы слова (стрелка вниз)";
    forms.addEventListener("click", (ev) => {
      ev.stopPropagation();
      openForms(main, group, shorthand, context, false);
    });
    wrap.appendChild(forms);
  }
  return wrap;
}

// Insert the best form of the rank-th group (the digit / Enter fast path).
function chooseGroup(rank, fromHold) {
  const g = current.groups[rank];
  if (g) choose(g.best.form, current.word, current.context, rank, fromHold);
}

const stripMains = () => [...els.strip.querySelectorAll("button.chip-main")];

function openForms(anchor, group, shorthand, context, focusFirst) {
  closePopup();
  const pop = document.createElement("div");
  pop.className = "popup";
  pop.id = "forms-popup";

  // Every selectable form, in DOM order, so the keyboard can walk a flat list
  // even when the popup is a 2-D declension grid.
  const items = [];
  const formButton = (form, label) => {
    const b = document.createElement("button");
    b.type = "button";
    b.tabIndex = -1;
    const n = items.length + 1;
    if (n <= 9) {
      const badge = document.createElement("span");
      badge.className = "form-badge";
      badge.textContent = n;
      badge.setAttribute("aria-hidden", "true");
      b.appendChild(badge);
    }
    if (label) {
      const tag = document.createElement("span");
      tag.className = "case-tag";
      tag.textContent = label;
      b.appendChild(tag);
    }
    b.appendChild(document.createTextNode(form));
    b.addEventListener("click", () => choose(form, shorthand, context, 0, true));
    items.push(b);
    return b;
  };

  // Prefer the generated declension grid (ед./мн. × cases); fall back to the
  // flat frequency list when the lemma has no paradigm (non-nouns).
  let paradigm = [];
  try {
    paradigm = JSON.parse(engine.paradigm_of_lemma_json(group.lemma));
  } catch {
    paradigm = [];
  }
  if (paradigm.length) {
    pop.classList.add("grouped");
    for (const grp of paradigm) {
      const section = document.createElement("div");
      section.className = "popup-group";
      const head = document.createElement("div");
      head.className = "popup-group-head";
      // Adjective singulars carry a gender label ("ед." + "м. р."); nouns and
      // plurals show the number alone.
      head.textContent = grp.gender ? `${grp.number} ${grp.gender}` : grp.number;
      section.appendChild(head);
      for (const cell of grp.forms) section.appendChild(formButton(cell.form, cell.case));
      pop.appendChild(section);
    }
  } else {
    for (const form of [group.best.form, ...group.variants]) pop.appendChild(formButton(form));
  }

  // Keyboard: arrows walk the flat item list, digits jump, Enter selects,
  // Esc closes back to the chip that opened it.
  pop.addEventListener("keydown", (e) => {
    const i = items.indexOf(document.activeElement);
    const focusAt = (j) => items[(j + items.length) % items.length]?.focus();
    if (e.key === "Escape") {
      e.preventDefault();
      closePopup();
      anchor.focus();
    } else if (e.key === "ArrowDown" || e.key === "ArrowRight") {
      e.preventDefault();
      focusAt((i < 0 ? -1 : i) + 1);
    } else if (e.key === "ArrowUp" || e.key === "ArrowLeft") {
      e.preventDefault();
      focusAt((i < 0 ? 0 : i) - 1);
    } else if (e.key === "Home") {
      e.preventDefault();
      focusAt(0);
    } else if (e.key === "End") {
      e.preventDefault();
      focusAt(items.length - 1);
    } else if (e.key === "Enter" || e.key === " ") {
      e.preventDefault();
      (items[i] || items[0])?.click();
    } else if (e.key >= "1" && e.key <= "9" && !e.ctrlKey && !e.altKey && !e.metaKey) {
      const j = +e.key - 1;
      if (items[j]) {
        e.preventDefault();
        items[j].click();
      }
    }
  });

  document.body.appendChild(pop);
  const r = anchor.getBoundingClientRect();
  pop.style.left = `${window.scrollX + r.left}px`;
  pop.style.top = `${window.scrollY + r.bottom + 4}px`;
  setTimeout(() => document.addEventListener("click", closePopup, { once: true }), 0);
  if (focusFirst) items[0]?.focus();
}

function closePopup() {
  document.getElementById("forms-popup")?.remove();
}

// A pending choice becomes confirmed only when it survives past the immediate
// undo window. This keeps the core signal semantics honest: undoing a just-made
// choice records `reverted`, not `confirmed + reverted`.
function confirmPendingUndo() {
  if (!pendingUndo) return;
  const { shorthand, form, context, rank, fromHold } = pendingUndo;
  if (els.learn.checked && engine) {
    engine.accept(shorthand, form);
    localStorage.setItem(HISTORY_KEY, engine.export_history());
  }
  if (els.log.checked) {
    logEvent({ status: "confirmed", shorthand, form, context, rank, fromHold });
  }
  pendingUndo = null;
}

// Insert the chosen form and arm a one-tap undo; learning/logging are deferred
// until the choice is actually kept.
function choose(form, shorthand, context, rank, fromHold) {
  confirmPendingUndo();
  closePopup();
  const { start, end } = caretWord();
  const text = els.editor.value;
  els.editor.value = text.slice(0, start) + form + " " + text.slice(end);
  const caret = start + form.length + 1;
  els.editor.setSelectionRange(caret, caret);
  els.editor.focus();

  pendingUndo = {
    shorthand,
    form,
    context,
    rank,
    fromHold,
    start,
    span: form.length + 1,
  };
  render();
}

function undoChip() {
  const b = document.createElement("button");
  b.className = "chip-main undo";
  b.type = "button";
  b.textContent = `↶ отменить «${pendingUndo.shorthand}»`;
  b.title = "вернуть сокращение — это негативный сигнал (reverted)";
  b.addEventListener("click", revertLast);
  return b;
}

// Reverting a just-made choice: restore the shorthand and tell the engine
// this pair was rejected (negative signal — its prior can go negative).
function revertLast() {
  if (!pendingUndo) return;
  const { shorthand, form, start, span } = pendingUndo;
  const text = els.editor.value;
  els.editor.value = text.slice(0, start) + shorthand + text.slice(start + span);
  const caret = start + shorthand.length;
  els.editor.setSelectionRange(caret, caret);
  els.editor.focus();

  if (els.learn.checked && engine) {
    engine.reject(shorthand, form); // reverted (negative signal)
    localStorage.setItem(HISTORY_KEY, engine.export_history());
  }
  if (els.log.checked) {
    logEvent({ status: "reverted", shorthand, form });
  }
  pendingUndo = null;
  render();
}

function logEvent(event) {
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

els.editor.addEventListener("input", () => {
  confirmPendingUndo(); // typing past the insertion confirms the choice
  render();
});
els.editor.addEventListener("click", render);
els.editor.addEventListener("keyup", (e) => {
  if (["ArrowLeft", "ArrowRight", "Home", "End"].includes(e.key)) render();
});

// Keyboard selection from the editor: a digit picks that suggestion outright
// (the abbreviation alphabet is Cyrillic, so digits are free to repurpose
// while the strip is up); Tab steps into the strip for arrow/forms control.
els.editor.addEventListener("keydown", (e) => {
  const firstBtn = els.strip.querySelector("button");
  if (e.key === "Tab" && !e.shiftKey && firstBtn) {
    e.preventDefault();
    firstBtn.tabIndex = 0;
    firstBtn.focus();
    return;
  }
  if (
    current.groups.length &&
    e.key >= "1" &&
    e.key <= "9" &&
    !e.ctrlKey &&
    !e.altKey &&
    !e.metaKey
  ) {
    const idx = +e.key - 1;
    if (idx < current.groups.length) {
      e.preventDefault(); // select the suggestion instead of typing the digit
      chooseGroup(idx, false);
    }
  }
});

// Navigation within the suggestion strip (roving focus over the chips).
els.strip.addEventListener("keydown", (e) => {
  const mains = stripMains();
  const i = mains.indexOf(document.activeElement);
  if (i === -1) return; // focus is on the undo chip or nothing actionable

  if (e.key === "Escape" || e.key === "ArrowUp") {
    e.preventDefault();
    els.editor.focus();
    return;
  }
  // The undo chip is also a `.chip-main` but has no suggestion behind it:
  // let Enter fire its native revert and ignore digits/arrows.
  if (mains[i].classList.contains("undo")) return;
  const focusAt = (j) => {
    const b = mains[(j + mains.length) % mains.length];
    document.activeElement.tabIndex = -1;
    b.tabIndex = 0;
    b.focus();
  };
  if (e.key === "ArrowRight") {
    e.preventDefault();
    focusAt(i + 1);
  } else if (e.key === "ArrowLeft") {
    e.preventDefault();
    focusAt(i - 1);
  } else if (e.key === "Home") {
    e.preventDefault();
    focusAt(0);
  } else if (e.key === "End") {
    e.preventDefault();
    focusAt(mains.length - 1);
  } else if (e.key === "ArrowDown") {
    e.preventDefault();
    const g = mains[i]._group;
    if (g && g.variants.length) openForms(mains[i], g, current.word, current.context, true);
  } else if (e.key === "Enter" || e.key === " ") {
    e.preventDefault();
    chooseGroup(mains[i]._rank, false);
  } else if (e.key >= "1" && e.key <= "9" && !e.ctrlKey && !e.altKey && !e.metaKey) {
    const idx = +e.key - 1;
    if (idx < current.groups.length) {
      e.preventDefault();
      chooseGroup(idx, false);
    }
  }
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
window.addEventListener("beforeunload", confirmPendingUndo);

els.clear.addEventListener("click", () => {
  if (!confirm("Удалить локальную историю и лог принятий?")) return;
  pendingUndo = null;
  localStorage.removeItem(HISTORY_KEY);
  localStorage.removeItem(LOG_KEY);
  if (engine) engine.import_history("");
  els.status.textContent = "Локальные данные очищены.";
});

boot();
