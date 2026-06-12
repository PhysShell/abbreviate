//! Context plumbing.
//!
//! The MVP runs without a context model, but the engine is built around a
//! trait so a platform shell can plug in anything from a compact n-gram
//! model to a quantized transformer reranker without touching the core.

/// Left context of the word being typed.
#[derive(Debug, Clone, Default)]
pub struct Context {
    /// Previous words of the sentence, most recent last, raw user spelling.
    pub previous_words: Vec<String>,
}

impl Context {
    pub fn new(previous_words: Vec<String>) -> Self {
        Self { previous_words }
    }
}

/// Pluggable contextual scorer. Higher is better; `0.0` is neutral.
pub trait ContextModel: Send + Sync {
    fn score(&self, context: &Context, candidate_form: &str, candidate_lemma: &str) -> f32;
}

/// Default model: no contextual signal.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoContext;

impl ContextModel for NoContext {
    fn score(&self, _context: &Context, _form: &str, _lemma: &str) -> f32 {
        0.0
    }
}
