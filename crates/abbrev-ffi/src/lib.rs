//! UniFFI surface of the engine.
//!
//! One thin, object-oriented API consumed by the Kotlin Android IME shell,
//! the Swift iOS shell and any other native frontend. The wrapper owns a
//! mutex because IME callbacks may arrive from different threads; the core
//! itself stays single-threaded and sans-IO.

use std::fmt;
use std::sync::{Arc, Mutex};

use abbrev_core::{Context, Engine, Lexicon};

uniffi::setup_scaffolding!();

/// One ranked suggestion, mirrored into Kotlin/Swift records.
#[derive(uniffi::Record)]
pub struct Suggestion {
    pub form: String,
    pub lemma: String,
    pub score: f64,
}

/// One lemma group for the two-level suggestion strip: tap inserts `best`,
/// hold expands `variants` (sibling forms of the same lemma).
#[derive(uniffi::Record)]
pub struct SuggestionGroup {
    pub lemma: String,
    pub best: Suggestion,
    pub variants: Vec<String>,
}

#[derive(Debug, uniffi::Error)]
pub enum AbbrevError {
    InvalidLexicon { message: String },
}

impl fmt::Display for AbbrevError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidLexicon { message } => write!(f, "invalid lexicon: {message}"),
        }
    }
}

impl std::error::Error for AbbrevError {}

/// Thread-safe engine handle exported over FFI.
#[derive(uniffi::Object)]
pub struct AbbrevEngine {
    inner: Mutex<Engine>,
}

#[uniffi::export]
impl AbbrevEngine {
    /// Engine over the small built-in demo lexicon (smoke tests, demos).
    #[uniffi::constructor]
    pub fn with_demo_lexicon() -> Arc<Self> {
        Arc::new(Self {
            inner: Mutex::new(Engine::new(Lexicon::demo())),
        })
    }

    /// Engine over a lexicon in the engine TSV format
    /// (`form<TAB>lemma<TAB>freq`), e.g. produced by `lexicon-builder`.
    #[uniffi::constructor]
    pub fn from_lexicon_tsv(tsv: String) -> Result<Arc<Self>, AbbrevError> {
        let lexicon = Lexicon::from_tsv_str(&tsv).map_err(|e| AbbrevError::InvalidLexicon {
            message: e.to_string(),
        })?;
        Ok(Arc::new(Self {
            inner: Mutex::new(Engine::new(lexicon)),
        }))
    }

    /// Ranked suggestions for an abbreviated input.
    /// `previous_words` is the left sentence context (may be empty).
    pub fn suggest(
        &self,
        input: String,
        previous_words: Vec<String>,
        limit: u32,
    ) -> Vec<Suggestion> {
        let context = Context::new(previous_words);
        self.inner
            .lock()
            .expect("engine mutex poisoned")
            .suggest(&input, &context, limit as usize)
            .into_iter()
            .map(|s| Suggestion {
                form: s.form,
                lemma: s.lemma,
                score: f64::from(s.score),
            })
            .collect()
    }

    /// Two-level suggestions: one group per lemma (the candidate strip),
    /// each with the best form for the typed ending plus hold-variants.
    pub fn suggest_grouped(
        &self,
        input: String,
        previous_words: Vec<String>,
        limit: u32,
    ) -> Vec<SuggestionGroup> {
        let context = Context::new(previous_words);
        self.inner
            .lock()
            .expect("engine mutex poisoned")
            .suggest_grouped(&input, &context, limit as usize)
            .into_iter()
            .map(|g| SuggestionGroup {
                lemma: g.lemma,
                best: Suggestion {
                    form: g.best.form,
                    lemma: g.best.lemma,
                    score: f64::from(g.best.score),
                },
                variants: g.variants,
            })
            .collect()
    }

    /// Inflected forms of a lemma for the "hold for forms" UI.
    pub fn forms_of_lemma(&self, lemma: String) -> Vec<String> {
        self.inner
            .lock()
            .expect("engine mutex poisoned")
            .forms_of_lemma(&lemma)
    }

    /// Records the suggestion the user accepted (local personalization).
    pub fn accept(&self, input: String, form: String) {
        self.inner
            .lock()
            .expect("engine mutex poisoned")
            .accept(&input, &form);
    }

    /// Opaque history blob; the shell decides where (and whether) to store it.
    pub fn export_history(&self) -> String {
        self.inner
            .lock()
            .expect("engine mutex poisoned")
            .export_history()
    }

    pub fn import_history(&self, blob: String) {
        self.inner
            .lock()
            .expect("engine mutex poisoned")
            .import_history(&blob);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ffi_surface_smoke() {
        let engine = AbbrevEngine::with_demo_lexicon();
        let top = engine.suggest("првт".into(), vec![], 3);
        assert_eq!(top.first().map(|s| s.form.as_str()), Some("привет"));
        engine.accept("првт".into(), "приват".into());
        let blob = engine.export_history();
        assert!(blob.contains("приват"));
    }

    #[test]
    fn rejects_bad_lexicon() {
        assert!(AbbrevEngine::from_lexicon_tsv("каша".into()).is_err());
    }
}
