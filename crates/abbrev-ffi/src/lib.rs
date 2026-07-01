//! UniFFI surface of the engine.
//!
//! One thin, object-oriented API consumed by the Kotlin Android IME shell,
//! the Swift iOS shell and any other native frontend. The wrapper owns a
//! mutex because IME callbacks may arrive from different threads; the core
//! itself stays single-threaded and sans-IO.

use std::fmt;
use std::sync::{Arc, Mutex};

use abbrev_core::morph::Case;
use abbrev_core::{
    BigramModel, Context, Engine, Gender, Lexicon, Masker, Number, Paradigms,
    Register as CoreRegister, Shortcuts, ToneMeter,
};

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

/// Grammatical number of a declension group.
#[derive(uniffi::Enum)]
pub enum GrammaticalNumber {
    Singular,
    Plural,
}

/// Grammatical gender of a declension group. Present only on an adjective's
/// singular groups; `None` for nouns and any plural.
#[derive(uniffi::Enum)]
pub enum GrammaticalGender {
    Masculine,
    Feminine,
    Neuter,
}

/// Coarse register of the recent window, from the tone meter (§5.1). Drives
/// the profanity-masking gate and can colour the shell's UI.
#[derive(uniffi::Enum)]
pub enum Register {
    Polite,
    Neutral,
    Crude,
}

impl From<CoreRegister> for Register {
    fn from(r: CoreRegister) -> Self {
        match r {
            CoreRegister::Polite => Register::Polite,
            CoreRegister::Neutral => Register::Neutral,
            CoreRegister::Crude => Register::Crude,
        }
    }
}

/// Russian grammatical case (the six declension cases).
#[derive(uniffi::Enum)]
pub enum GrammaticalCase {
    Nominative,
    Genitive,
    Dative,
    Accusative,
    Instrumental,
    Locative,
}

/// One declension cell: a case and the surface form filling it.
#[derive(uniffi::Record)]
pub struct CaseForm {
    pub case: GrammaticalCase,
    pub form: String,
}

/// A lemma's declension for one number (and, for adjectives, gender),
/// case-ordered — backs a grouped hold-popup (singular first; within the
/// singular, masculine → feminine → neuter).
#[derive(uniffi::Record)]
pub struct ParadigmGroup {
    pub number: GrammaticalNumber,
    /// Gender of an adjectival singular group; `None` for nouns and plurals.
    pub gender: Option<GrammaticalGender>,
    pub forms: Vec<CaseForm>,
}

fn to_ffi_group(group: &abbrev_core::ParadigmGroup) -> ParadigmGroup {
    ParadigmGroup {
        number: match group.number {
            Number::Singular => GrammaticalNumber::Singular,
            Number::Plural => GrammaticalNumber::Plural,
        },
        gender: group.gender.map(|g| match g {
            Gender::Masculine => GrammaticalGender::Masculine,
            Gender::Feminine => GrammaticalGender::Feminine,
            Gender::Neuter => GrammaticalGender::Neuter,
        }),
        forms: group
            .forms
            .iter()
            .map(|cf| CaseForm {
                case: to_ffi_case(cf.case),
                form: cf.form.clone(),
            })
            .collect(),
    }
}

fn to_ffi_case(case: Case) -> GrammaticalCase {
    match case {
        Case::Nom => GrammaticalCase::Nominative,
        Case::Gen => GrammaticalCase::Genitive,
        Case::Dat => GrammaticalCase::Dative,
        Case::Acc => GrammaticalCase::Accusative,
        Case::Ins => GrammaticalCase::Instrumental,
        Case::Loc => GrammaticalCase::Locative,
    }
}

#[derive(Debug, uniffi::Error)]
pub enum AbbrevError {
    // NB: the field is `reason`, not `message` — a variant field named `message`
    // collides with `Throwable.message` in UniFFI's generated Kotlin (the error
    // enum maps to a `kotlin.Exception` subclass).
    InvalidLexicon { reason: String },
    InvalidLanguageModel { reason: String },
    InvalidShortcuts { reason: String },
    InvalidMaskList { reason: String },
    InvalidToneMarkers { reason: String },
}

impl fmt::Display for AbbrevError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidLexicon { reason } => write!(f, "invalid lexicon: {reason}"),
            Self::InvalidLanguageModel { reason } => {
                write!(f, "invalid language model: {reason}")
            }
            Self::InvalidShortcuts { reason } => write!(f, "invalid shortcuts: {reason}"),
            Self::InvalidMaskList { reason } => write!(f, "invalid mask list: {reason}"),
            Self::InvalidToneMarkers { reason } => write!(f, "invalid tone markers: {reason}"),
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
            reason: e.to_string(),
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

    /// Inflected forms of a lemma for the "hold for forms" UI (ordered by the
    /// loaded paradigm when available, else by frequency).
    pub fn forms_of_lemma(&self, lemma: String) -> Vec<String> {
        self.inner
            .lock()
            .expect("engine mutex poisoned")
            .forms_of_lemma(&lemma)
    }

    /// Loads build-time declension paradigms (`ru-hold-groups.tsv`) for
    /// grouped hold-popups. Parsing is lenient, so this is infallible.
    pub fn load_paradigms(&self, tsv: String) {
        self.inner
            .lock()
            .expect("engine mutex poisoned")
            .set_paradigms(Paradigms::from_tsv_str(&tsv));
    }

    /// Structured declension of a lemma (singular first, case-ordered) for a
    /// grouped hold-popup. Empty when no paradigm is loaded for the lemma
    /// (e.g. a verb/adverb, or absent): the shell then falls back to
    /// `forms_of_lemma`.
    pub fn paradigm_of_lemma(&self, lemma: String) -> Vec<ParadigmGroup> {
        self.inner
            .lock()
            .expect("engine mutex poisoned")
            .paradigm_of_lemma(&lemma)
            .map(|groups| groups.iter().map(to_ffi_group).collect())
            .unwrap_or_default()
    }

    /// Plugs in a bigram language model (`#abbrev-lm v1` TSV artifact);
    /// `previous_words` passed to `suggest` then rerank candidates.
    pub fn load_language_model(&self, tsv: String) -> Result<(), AbbrevError> {
        let model =
            BigramModel::from_tsv_str(&tsv).map_err(|e| AbbrevError::InvalidLanguageModel {
                reason: e.to_string(),
            })?;
        self.inner
            .lock()
            .expect("engine mutex poisoned")
            .set_context_model(Box::new(model));
        Ok(())
    }

    /// Loads the conventional-shortcuts layer (`shorthand<TAB>form[<TAB>lemma]`).
    pub fn load_shortcuts(&self, tsv: String) -> Result<(), AbbrevError> {
        let shortcuts =
            Shortcuts::from_tsv_str(&tsv).map_err(|e| AbbrevError::InvalidShortcuts {
                reason: e.to_string(),
            })?;
        self.inner
            .lock()
            .expect("engine mutex poisoned")
            .set_shortcuts(shortcuts);
        Ok(())
    }

    /// Loads the profanity-masking censor list (one lemma per line; §5.2).
    /// Inert until [`set_masking`](Self::set_masking) turns the gate on — the
    /// list is *what* to mask, the gate is *when*.
    pub fn load_mask_list(&self, list: String) -> Result<(), AbbrevError> {
        let masker = Masker::from_list_str(&list).map_err(|e| AbbrevError::InvalidMaskList {
            reason: e.to_string(),
        })?;
        self.inner
            .lock()
            .expect("engine mutex poisoned")
            .set_masker(masker);
        Ok(())
    }

    /// Loads the tone/register marker list (`word<TAB>sign`, sign in `[-1, 1]`;
    /// §5.1). Feeds off the same `note_word` stream; read via
    /// [`register`](Self::register). Enables the `mask_when_polite` gate.
    pub fn load_tone_markers(&self, tsv: String) -> Result<(), AbbrevError> {
        let mut meter = ToneMeter::new();
        meter
            .markers_from_tsv_str(&tsv)
            .map_err(|e| AbbrevError::InvalidToneMarkers {
                reason: e.to_string(),
            })?;
        self.inner
            .lock()
            .expect("engine mutex poisoned")
            .set_tone_meter(meter);
        Ok(())
    }

    /// Turns profanity masking on/off at runtime, bound to a user setting.
    /// `enabled` is the master switch; `when_polite` additionally tone-gates it
    /// so a masked twin is offered only in a polite window (§5.1 gates §5.2).
    /// Off by default and inert until a mask list is loaded.
    pub fn set_masking(&self, enabled: bool, when_polite: bool) {
        self.inner
            .lock()
            .expect("engine mutex poisoned")
            .set_masking(enabled, when_polite);
    }

    /// Coarse register of the recent window (from the tone meter), for a shell
    /// that wants to show or act on it. `Neutral` until markers are loaded and
    /// recent signal accrues.
    pub fn register(&self) -> Register {
        self.inner
            .lock()
            .expect("engine mutex poisoned")
            .register()
            .into()
    }

    /// Records a confirmed suggestion (picked and kept).
    pub fn accept(&self, input: String, form: String) {
        self.inner
            .lock()
            .expect("engine mutex poisoned")
            .accept(&input, &form);
    }

    /// Records a reverted suggestion (undone/edited after insertion) —
    /// negative signal; the pair's ranking prior can go negative.
    pub fn reject(&self, input: String, form: String) {
        self.inner
            .lock()
            .expect("engine mutex poisoned")
            .reject(&input, &form);
    }

    /// Notes a word the user committed in the current context, feeding the
    /// ephemeral session recency cache: a freshly-used word floats up in
    /// later rankings and decays as the conversation moves on. The shell
    /// calls this once per committed word. The cache is in-memory only —
    /// never persisted, never synced.
    pub fn note_word(&self, word: String) {
        self.inner
            .lock()
            .expect("engine mutex poisoned")
            .note_word(&word);
    }

    /// Clears the session recency cache. The shell calls this when the
    /// context changes (e.g. a different app/field), so recency never leaks
    /// across contexts.
    pub fn reset_session(&self) {
        self.inner
            .lock()
            .expect("engine mutex poisoned")
            .reset_session();
    }

    /// Merges another device's history blob (sum of counters) for sync.
    pub fn merge_history(&self, blob: String) {
        self.inner
            .lock()
            .expect("engine mutex poisoned")
            .merge_history(&blob);
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
    fn note_word_raises_recent_form_score() {
        let engine = AbbrevEngine::with_demo_lexicon();
        let score_of = |e: &AbbrevEngine| {
            e.suggest("првт".into(), vec![], 5)
                .into_iter()
                .find(|s| s.form == "приват")
                .map(|s| s.score)
        };
        let before = score_of(&engine).expect("приват is a candidate");
        engine.note_word("приват".into());
        let after = score_of(&engine).expect("приват is a candidate");
        assert!(
            after > before,
            "recency must raise the score: {before} -> {after}"
        );
        // Resetting the context drops the boost back.
        engine.reset_session();
        assert_eq!(score_of(&engine), Some(before));
    }

    #[test]
    fn rejects_bad_lexicon() {
        assert!(AbbrevEngine::from_lexicon_tsv("каша".into()).is_err());
    }

    #[test]
    fn masking_gate_over_ffi() {
        let engine =
            AbbrevEngine::from_lexicon_tsv("долбоёб\tдолбоёб\t100\tNOUN\n".into()).unwrap();
        engine.load_mask_list("долбоёб\n".into()).unwrap();
        engine
            .load_tone_markers("пожалуйста\t+\nхрень\t-\n".into())
            .unwrap();
        let masked = |e: &AbbrevEngine| {
            e.suggest("долбоёб".into(), vec![], 5)
                .iter()
                .any(|s| s.form.contains('@'))
        };

        // Off by default even with a list loaded.
        assert!(!masked(&engine));

        // Ungated: masks in any window.
        engine.set_masking(true, false);
        assert!(masked(&engine));

        // Tone-gated: only in a polite window.
        engine.set_masking(true, true);
        assert!(matches!(engine.register(), Register::Neutral));
        assert!(!masked(&engine), "neutral window must not mask");
        for _ in 0..3 {
            engine.note_word("пожалуйста".into());
        }
        assert!(matches!(engine.register(), Register::Polite));
        assert!(masked(&engine), "polite window must mask");
    }

    #[test]
    fn rejects_bad_mask_and_tone() {
        let engine = AbbrevEngine::with_demo_lexicon();
        assert!(engine.load_mask_list("два слова".into()).is_err());
        assert!(engine.load_tone_markers("слово\tвверх".into()).is_err());
    }

    #[test]
    fn paradigm_ffi_surface() {
        let engine = AbbrevEngine::with_demo_lexicon();
        engine.load_paradigms("работа\tsing\tработа|работы|работе|работу|работой|работе\n".into());
        let groups = engine.paradigm_of_lemma("работа".into());
        assert_eq!(groups.len(), 1);
        assert!(matches!(groups[0].number, GrammaticalNumber::Singular));
        assert!(matches!(
            groups[0].forms[0].case,
            GrammaticalCase::Nominative
        ));
        assert_eq!(groups[0].forms[0].form, "работа");
        assert!(groups[0].gender.is_none(), "nouns have no gender axis");
        // Unknown lemma → empty, so the shell falls back to forms_of_lemma.
        assert!(engine.paradigm_of_lemma("несуществующее".into()).is_empty());
    }

    #[test]
    fn paradigm_ffi_adjective_carries_gender() {
        let engine = AbbrevEngine::with_demo_lexicon();
        engine.load_paradigms(
            "красный\tsing.masc\tкрасный|красного|красному|красный|красным|красном\n\
             красный\tplur\tкрасные|красных|красным|красные|красными|красных\n"
                .into(),
        );
        let groups = engine.paradigm_of_lemma("красный".into());
        assert_eq!(groups.len(), 2);
        assert!(matches!(
            groups[0].gender,
            Some(GrammaticalGender::Masculine)
        ));
        assert!(groups[1].gender.is_none(), "plural has no gender");
    }
}
