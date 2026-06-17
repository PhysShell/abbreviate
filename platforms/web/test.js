// Typing speed tester for the abbreviation engine. Same wasm surface as the
// demo (crates/abbrev-wasm); everything runs offline. Generates a target text,
// counts down 3-2-1, times the run from the first keystroke, and auto-stops when
// the input matches the target. Compares two modes — plain typing (engine off)
// vs. suggestions (engine on) — and reports time, CPM/WPM, taps, and how often
// the accepted suggestion was the top one.

import init, { WasmEngine } from "./pkg/abbrev_wasm.js";

const MIN_LEN = 2;

// Short, punctuation-free Russian lines (lowercase) so completion is an exact
// string match and every token is a plain word the engine can suggest.
const TARGETS = [
  "привет как дела сегодня",
  "я работаю над новым проектом",
  "давай встретимся завтра вечером",
  "спасибо за помощь с задачей",
  "сейчас не могу говорить перезвоню позже",
  "это было очень интересно и полезно",
  "пожалуйста пришли мне документы утром",
  "мы пойдём в кино в субботу",
  "надо подумать над этим решением",
  "всё хорошо не беспокойся обо мне",
  "увидимся на следующей неделе",
  "какой у нас план на сегодня",
];

const els = {
  status: document.getElementById("status"),
  strip: document.getElementById("strip"),
  editor: document.getElementById("editor"),
  target: document.getElementById("target"),
  countdown: document.getElementById("countdown"),
  result: document.getElementById("result"),
  history: document.getElementById("history"),
  newTest: document.getElementById("new-test"),
};

let engine = null;
let target = "";
// Per-run state.
let run = null;

const mode = () => document.querySelector('input[name="mode"]:checked').value;
const norm = (s) => s.replace(/\s+/g, " ").trim().toLowerCase();
const isWordChar = (c) => /[а-яёА-ЯЁ-]/.test(c);

async function fetchText(url) {
  const res = await fetch(url);
  if (!res.ok) throw new Error(`${url}: ${res.status}`);
  return res.text();
}

async function boot() {
  try {
    await init();
    els.status.textContent = "Загрузка словаря…";
    const [lexicon, lm, shortcuts] = await Promise.all([
      fetchText("./assets/lexicon.tsv"),
      fetchText("./assets/lm.tsv").catch(() => null),
      fetchText("./assets/shortcuts.tsv").catch(() => null),
    ]);
    engine = new WasmEngine(lexicon);
    if (lm) engine.load_language_model(lm);
    if (shortcuts) engine.load_shortcuts(shortcuts);
    els.status.textContent = "Готово. Нажмите «Новый текст».";
    els.newTest.disabled = false;
  } catch (e) {
    els.status.textContent = `Ошибка загрузки: ${e.message}. Соберите демо (см. README).`;
    els.status.classList.add("error");
  }
}

// --- a test run -----------------------------------------------------------

function newTest() {
  target = TARGETS[Math.floor(Math.random() * TARGETS.length)];
  run = {
    mode: mode(),
    startedAt: null,
    keystrokes: 0,
    taps: 0,
    accepted: 0,
    acceptedTop: 0,
    done: false,
  };
  els.result.hidden = true;
  els.strip.innerHTML = "";
  els.editor.value = "";
  els.editor.disabled = true;
  renderTarget("");
  countdown(3);
}

function countdown(n) {
  els.countdown.hidden = false;
  if (n > 0) {
    els.countdown.textContent = n;
    setTimeout(() => countdown(n - 1), 700);
  } else {
    els.countdown.textContent = "Печатай!";
    setTimeout(() => {
      els.countdown.hidden = true;
      els.editor.disabled = false;
      els.editor.focus();
    }, 500);
  }
}

function renderTarget(typed) {
  // Green for the correctly typed prefix, the rest plain.
  let i = 0;
  while (i < typed.length && i < target.length && typed[i] === target[i]) i++;
  els.target.innerHTML = "";
  const ok = document.createElement("span");
  ok.className = "ok";
  ok.textContent = target.slice(0, i);
  const rest = document.createElement("span");
  rest.textContent = target.slice(i);
  els.target.append(ok, rest);
}

function caretWord() {
  const text = els.editor.value;
  const caret = els.editor.selectionStart;
  let start = caret;
  while (start > 0 && isWordChar(text[start - 1])) start--;
  const word = text.slice(start, caret);
  const before = text.slice(0, start).match(/[а-яёА-ЯЁ-]+/g) || [];
  return { word, start, end: caret, context: before.slice(-2).join(" ") };
}

function renderStrip() {
  els.strip.innerHTML = "";
  if (run.mode !== "on" || !engine || run.done) return;
  const { word, context } = caretWord();
  if (word.length < MIN_LEN) return;
  let groups;
  try {
    groups = JSON.parse(engine.suggest_grouped_json(word, context, 6));
  } catch {
    return;
  }
  groups.forEach((g, i) => {
    const b = document.createElement("button");
    b.type = "button";
    b.className = "chip-main";
    if (i < 9) {
      const badge = document.createElement("span");
      badge.className = "chip-badge";
      badge.textContent = i + 1;
      b.appendChild(badge);
    }
    b.appendChild(document.createTextNode(g.best.form));
    b.addEventListener("click", () => {
      run.taps++;
      choose(g.best.form, word, context, i);
    });
    els.strip.appendChild(b);
  });
}

function choose(form, shorthand, context, rank) {
  const { start, end } = caretWord();
  const text = els.editor.value;
  els.editor.value = text.slice(0, start) + form + " " + text.slice(end);
  const caret = start + form.length + 1;
  els.editor.setSelectionRange(caret, caret);
  els.editor.focus();
  run.accepted++;
  if (rank === 0) run.acceptedTop++;
  if (engine) engine.accept(shorthand, form);
  onChanged();
}

function onChanged() {
  if (!run || run.done) return;
  if (run.startedAt === null) run.startedAt = performance.now();
  renderTarget(els.editor.value);
  renderStrip();
  if (norm(els.editor.value) === norm(target)) complete();
}

function complete() {
  run.done = true;
  els.strip.innerHTML = "";
  els.editor.disabled = true;
  const seconds = (performance.now() - run.startedAt) / 1000;
  const chars = target.length;
  const words = target.trim().split(/\s+/).length;
  const cpm = Math.round((chars / seconds) * 60);
  const wpm = Math.round((words / seconds) * 60);
  const topPct = run.accepted ? Math.round((run.acceptedTop / run.accepted) * 100) : null;

  els.result.hidden = false;
  els.result.innerHTML =
    `<b>${seconds.toFixed(1)} с</b> · ${cpm} зн/мин · ${wpm} сл/мин · ` +
    `${run.keystrokes + run.taps} нажатий` +
    (run.mode === "on" ? ` · подсказок: ${run.accepted} (верхняя ${topPct ?? "—"}%)` : "");

  const tr = document.createElement("tr");
  tr.innerHTML =
    `<td>${run.mode === "on" ? "с подсказками" : "обычно"}</td>` +
    `<td>${seconds.toFixed(1)} с</td><td>${cpm}</td><td>${wpm}</td>` +
    `<td>${run.keystrokes + run.taps}</td>` +
    `<td>${run.mode === "on" ? run.accepted : "—"}</td>` +
    `<td>${topPct ?? "—"}</td>`;
  els.history.prepend(tr);
}

// --- wiring ---------------------------------------------------------------

els.newTest.addEventListener("click", newTest);

els.editor.addEventListener("input", () => {
  if (!run || run.done) return;
  run.keystrokes++;
  onChanged();
});
els.editor.addEventListener("click", () => run && !run.done && renderStrip());
els.editor.addEventListener("keyup", (e) => {
  if (["ArrowLeft", "ArrowRight", "Home", "End"].includes(e.key)) renderStrip();
});

// Digit picks a suggestion (same as the demo); only while the strip is up.
els.editor.addEventListener("keydown", (e) => {
  if (run?.done || run?.mode !== "on") return;
  if (e.key >= "1" && e.key <= "9" && !e.ctrlKey && !e.altKey && !e.metaKey) {
    const chips = [...els.strip.querySelectorAll("button.chip-main")];
    const chip = chips[+e.key - 1];
    if (chip) {
      e.preventDefault();
      chip.click(); // its handler counts the tap + inserts
    }
  }
});

boot();
