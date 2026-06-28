//! WASM surface of the engine for web keyboards, browser extensions and
//! webview-based desktop shells (Tauri). Suggestions are returned as a
//! JSON string to keep the JS boundary simple and stable.
//!
//! Build: `wasm-pack build crates/abbrev-wasm --target web`.

use abbrev_core::morph::Case;
use abbrev_core::{
    BigramModel, Context, Engine, Gender, Lexicon, Number, ParadigmGroup, Paradigms, Shortcuts,
};
use serde::Serialize;
use wasm_bindgen::prelude::*;

#[derive(Serialize)]
struct JsSuggestion<'a> {
    form: &'a str,
    lemma: &'a str,
    score: f32,
}

#[derive(Serialize)]
struct JsSuggestionGroup<'a> {
    lemma: &'a str,
    best: JsSuggestion<'a>,
    variants: &'a [String],
}

#[derive(Serialize)]
struct JsParadigmGroup<'a> {
    /// Number label, ready to render: "ед." / "мн.".
    number: &'static str,
    /// Gender label for an adjectival singular ("м. р." / "ж. р." / "с. р."),
    /// or `null` for nouns and plurals. Serialized as JSON `null` so the web
    /// can append it to the group heading only when present.
    gender: Option<&'static str>,
    forms: Vec<JsCaseForm<'a>>,
}

#[derive(Serialize)]
struct JsCaseForm<'a> {
    /// Russian case abbreviation, ready to render: "им.", "род.", …
    case: &'static str,
    form: &'a str,
}

fn js_group(g: &ParadigmGroup) -> JsParadigmGroup<'_> {
    JsParadigmGroup {
        number: match g.number {
            Number::Singular => "ед.",
            Number::Plural => "мн.",
        },
        gender: g.gender.map(|gender| match gender {
            Gender::Masculine => "м. р.",
            Gender::Feminine => "ж. р.",
            Gender::Neuter => "с. р.",
        }),
        forms: g
            .forms
            .iter()
            .map(|cf| JsCaseForm {
                case: case_label(cf.case),
                form: &cf.form,
            })
            .collect(),
    }
}

fn case_label(case: Case) -> &'static str {
    match case {
        Case::Nom => "им.",
        Case::Gen => "род.",
        Case::Dat => "дат.",
        Case::Acc => "вин.",
        Case::Ins => "тв.",
        Case::Loc => "пр.",
    }
}

#[wasm_bindgen]
pub struct WasmEngine {
    inner: Engine,
}

#[wasm_bindgen]
impl WasmEngine {
    /// Creates an engine. Pass a lexicon in the engine TSV format, or
    /// `undefined`/`null` to use the built-in demo lexicon.
    #[wasm_bindgen(constructor)]
    pub fn new(lexicon_tsv: Option<String>) -> Result<WasmEngine, JsError> {
        let lexicon = match lexicon_tsv {
            Some(tsv) => Lexicon::from_tsv_str(&tsv).map_err(|e| JsError::new(&e.to_string()))?,
            None => Lexicon::demo(),
        };
        Ok(Self {
            inner: Engine::new(lexicon),
        })
    }

    /// Ranked suggestions as a JSON array of `{form, lemma, score}`.
    /// `previous_words` is the whitespace-separated left context.
    pub fn suggest_json(&self, input: &str, previous_words: &str, limit: usize) -> String {
        let context = Context::new(
            previous_words
                .split_whitespace()
                .map(String::from)
                .collect(),
        );
        let suggestions = self.inner.suggest(input, &context, limit);
        let view: Vec<JsSuggestion<'_>> = suggestions
            .iter()
            .map(|s| JsSuggestion {
                form: &s.form,
                lemma: &s.lemma,
                score: s.score,
            })
            .collect();
        serde_json::to_string(&view).unwrap_or_else(|_| "[]".to_string())
    }

    /// Two-level suggestions as JSON: `[{lemma, best: {form, lemma, score},
    /// variants: [...]}, ...]` — one group per lemma for the candidate strip.
    pub fn suggest_grouped_json(&self, input: &str, previous_words: &str, limit: usize) -> String {
        let context = Context::new(
            previous_words
                .split_whitespace()
                .map(String::from)
                .collect(),
        );
        let groups = self.inner.suggest_grouped(input, &context, limit);
        let view: Vec<JsSuggestionGroup<'_>> = groups
            .iter()
            .map(|g| JsSuggestionGroup {
                lemma: &g.lemma,
                best: JsSuggestion {
                    form: &g.best.form,
                    lemma: &g.best.lemma,
                    score: g.best.score,
                },
                variants: &g.variants,
            })
            .collect();
        serde_json::to_string(&view).unwrap_or_else(|_| "[]".to_string())
    }

    /// Inflected forms of a lemma as a JSON array of strings (ordered by the
    /// loaded paradigm when available, else by frequency).
    pub fn forms_of_lemma_json(&self, lemma: &str) -> String {
        serde_json::to_string(&self.inner.forms_of_lemma(lemma))
            .unwrap_or_else(|_| "[]".to_string())
    }

    /// Loads build-time declension paradigms (`ru-hold-groups.tsv`) for
    /// grouped hold-popups. Parsing is lenient, so this is infallible.
    pub fn load_paradigms(&mut self, tsv: &str) {
        self.inner.set_paradigms(Paradigms::from_tsv_str(tsv));
    }

    /// Declension of a lemma as JSON:
    /// `[{number, gender, forms: [{case, form}]}]`, singular first and
    /// case-ordered, with display-ready Russian labels (`gender` is `null`
    /// for nouns and plurals). Returns `[]` when no paradigm is loaded for
    /// the lemma — the caller then falls back to `forms_of_lemma_json`.
    pub fn paradigm_of_lemma_json(&self, lemma: &str) -> String {
        let view: Vec<JsParadigmGroup<'_>> = self
            .inner
            .paradigm_of_lemma(lemma)
            .map(|groups| groups.iter().map(js_group).collect())
            .unwrap_or_default();
        serde_json::to_string(&view).unwrap_or_else(|_| "[]".to_string())
    }

    /// Plugs in a bigram language model (`#abbrev-lm v1` TSV artifact).
    pub fn load_language_model(&mut self, tsv: &str) -> Result<(), JsError> {
        let model = BigramModel::from_tsv_str(tsv).map_err(|e| JsError::new(&e.to_string()))?;
        self.inner.set_context_model(Box::new(model));
        Ok(())
    }

    /// Loads the conventional-shortcuts layer.
    pub fn load_shortcuts(&mut self, tsv: &str) -> Result<(), JsError> {
        let shortcuts = Shortcuts::from_tsv_str(tsv).map_err(|e| JsError::new(&e.to_string()))?;
        self.inner.set_shortcuts(shortcuts);
        Ok(())
    }

    /// Records a confirmed suggestion (picked and kept).
    pub fn accept(&mut self, input: &str, form: &str) {
        self.inner.accept(input, form);
    }

    /// Records a reverted suggestion (undone after insertion) — negative.
    pub fn reject(&mut self, input: &str, form: &str) {
        self.inner.reject(input, form);
    }

    /// Notes a word committed in the current context, feeding the ephemeral
    /// session recency cache: a freshly-used word floats up in later rankings
    /// and decays as the conversation moves on. Call once per committed word.
    /// In-memory only — never persisted, never synced.
    pub fn note_word(&mut self, word: &str) {
        self.inner.note_word(word);
    }

    /// Clears the session recency cache (call on a context change, e.g. a
    /// different field/app) so recency never leaks across contexts.
    pub fn reset_session(&mut self) {
        self.inner.reset_session();
    }

    /// Merges another device's history blob (sum of counters) for sync.
    pub fn merge_history(&mut self, blob: &str) {
        self.inner.merge_history(blob);
    }

    pub fn export_history(&self) -> String {
        self.inner.export_history()
    }

    pub fn import_history(&mut self, blob: &str) {
        self.inner.import_history(blob);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_surface_smoke() {
        let engine = WasmEngine::new(None).unwrap();
        let json = engine.suggest_json("првт", "", 3);
        assert!(json.contains("привет"), "{json}");
    }

    #[test]
    fn session_cache_surface_smoke() {
        let mut engine = WasmEngine::new(None).unwrap();
        engine.note_word("приват");
        assert!(engine.suggest_json("првт", "", 3).contains("приват"));
        engine.reset_session();
    }

    #[test]
    fn paradigm_json_surface() {
        let mut engine = WasmEngine::new(None).unwrap();
        engine.load_paradigms("работа\tsing\tработа|работы|работе|работу|работой|работе\n");
        let json = engine.paradigm_of_lemma_json("работа");
        assert!(
            json.contains("\"им.\"") && json.contains("работе"),
            "{json}"
        );
        // A noun group has no gender label (JSON null).
        assert!(json.contains("\"gender\":null"), "{json}");
        // Unknown lemma → empty array so the caller falls back to flat forms.
        assert_eq!(engine.paradigm_of_lemma_json("несуществующее"), "[]");
    }

    #[test]
    fn paradigm_json_adjective_labels_gender() {
        let mut engine = WasmEngine::new(None).unwrap();
        engine.load_paradigms(
            "красный\tsing.masc\tкрасный|красного|красному|красный|красным|красном\n",
        );
        let json = engine.paradigm_of_lemma_json("красный");
        assert!(
            json.contains("\"ед.\"") && json.contains("\"м. р.\""),
            "{json}"
        );
    }
}
