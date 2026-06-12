//! WASM surface of the engine for web keyboards, browser extensions and
//! webview-based desktop shells (Tauri). Suggestions are returned as a
//! JSON string to keep the JS boundary simple and stable.
//!
//! Build: `wasm-pack build crates/abbrev-wasm --target web`.

use abbrev_core::{Context, Engine, Lexicon};
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

    /// Inflected forms of a lemma as a JSON array of strings.
    pub fn forms_of_lemma_json(&self, lemma: &str) -> String {
        serde_json::to_string(&self.inner.forms_of_lemma(lemma))
            .unwrap_or_else(|_| "[]".to_string())
    }

    pub fn accept(&mut self, input: &str, form: &str) {
        self.inner.accept(input, form);
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
}
