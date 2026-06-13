//! Compact bigram language model behind [`ContextModel`].
//!
//! Scoring is positive PMI of (previous word → candidate form): unseen
//! pairs are *neutral* (0.0), never punished — corpus coverage is sparse
//! and absence of evidence must not bury frequency-plausible candidates.
//! This is exactly the signal that separates `ну првт → привет` from
//! `в првт → приват`.
//!
//! Artifact format (`#abbrev-lm v1`, TSV, built by `lexicon-builder
//! bigrams` — see tools/):
//!
//! ```text
//! #abbrev-lm v1
//! u<TAB>слово<TAB>count
//! b<TAB>пред<TAB>слово<TAB>count
//! ```

use std::collections::HashMap;
use std::fmt;

use crate::alphabet::normalize;
use crate::context::{Context, ContextModel};

/// Positive-PMI ceiling: one contextual signal must not be able to
/// overpower every other ranking signal combined.
const MAX_PMI: f32 = 4.0;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LmError {
    pub line: usize,
    pub message: String,
}

impl fmt::Display for LmError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "lm line {}: {}", self.line, self.message)
    }
}

impl std::error::Error for LmError {}

/// Bigram LM: unigram counts + `prev → (word → count)`.
#[derive(Debug, Default)]
pub struct BigramModel {
    unigrams: HashMap<String, u64>,
    bigrams: HashMap<String, HashMap<String, u64>>,
    total: u64,
}

impl BigramModel {
    /// Parses the `#abbrev-lm v1` TSV artifact.
    pub fn from_tsv_str(tsv: &str) -> Result<Self, LmError> {
        let mut model = Self::default();
        for (i, raw) in tsv.lines().enumerate() {
            let line = raw.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let err = |message: String| LmError {
                line: i + 1,
                message,
            };
            let mut parts = line.split('\t');
            match parts.next() {
                Some("u") => {
                    let (Some(word), Some(count), None) =
                        (parts.next(), parts.next(), parts.next())
                    else {
                        return Err(err(format!("expected `u\\tword\\tcount`, got `{raw}`")));
                    };
                    let count: u64 = count
                        .parse()
                        .map_err(|_| err(format!("bad count `{count}`")))?;
                    model.total += count;
                    *model.unigrams.entry(normalize(word)).or_insert(0) += count;
                }
                Some("b") => {
                    let (Some(prev), Some(word), Some(count), None) =
                        (parts.next(), parts.next(), parts.next(), parts.next())
                    else {
                        return Err(err(format!(
                            "expected `b\\tprev\\tword\\tcount`, got `{raw}`"
                        )));
                    };
                    let count: u64 = count
                        .parse()
                        .map_err(|_| err(format!("bad count `{count}`")))?;
                    *model
                        .bigrams
                        .entry(normalize(prev))
                        .or_default()
                        .entry(normalize(word))
                        .or_insert(0) += count;
                }
                _ => return Err(err(format!("unknown record `{raw}`"))),
            }
        }
        Ok(model)
    }

    pub fn is_empty(&self) -> bool {
        self.bigrams.is_empty()
    }

    /// Positive PMI of `word` following `prev`; 0.0 when unseen.
    fn pmi(&self, prev: &str, word: &str) -> f32 {
        let Some(&pair) = self.bigrams.get(prev).and_then(|m| m.get(word)) else {
            return 0.0;
        };
        let (Some(&c_prev), Some(&c_word)) = (self.unigrams.get(prev), self.unigrams.get(word))
        else {
            return 0.0;
        };
        if c_prev == 0 || c_word == 0 || self.total == 0 {
            return 0.0;
        }
        let pmi = ((pair as f64 * self.total as f64) / (c_prev as f64 * c_word as f64)).ln();
        (pmi as f32).clamp(0.0, MAX_PMI)
    }
}

impl ContextModel for BigramModel {
    fn score(&self, context: &Context, candidate_form: &str, _lemma: &str) -> f32 {
        // Use the most recent context token that carries a word, stripping
        // surrounding punctuation: `ну,` / `(привет)` must still match LM
        // entries, otherwise contextual reranking silently falls back to no
        // context on ordinary punctuated text.
        let Some(prev) = context
            .previous_words
            .iter()
            .rev()
            .map(|w| context_key(w))
            .find(|k| !k.is_empty())
        else {
            return 0.0;
        };
        self.pmi(&prev, &normalize(candidate_form))
    }
}

/// Russian-word key of a raw context token: normalize (lowercase, `ё→е`)
/// and trim surrounding non-letters. Internal hyphens are kept (`кто-то`),
/// matching the bigram builder's tokenizer.
fn context_key(word: &str) -> String {
    normalize(word)
        .trim_matches(|c: char| !matches!(c, 'а'..='я'))
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    const LM: &str = "#abbrev-lm v1\n\
        u\tв\t1000\n\
        u\tну\t1000\n\
        u\tпривет\t200\n\
        u\tприват\t20\n\
        b\tв\tприват\t200\n\
        b\tну\tпривет\t150\n";

    fn ctx(words: &[&str]) -> Context {
        Context::new(words.iter().map(|w| w.to_string()).collect())
    }

    #[test]
    fn parses_and_scores_associations() {
        let lm = BigramModel::from_tsv_str(LM).unwrap();
        assert!(!lm.is_empty());
        // Seen association is positive, unseen is neutral.
        assert!(lm.score(&ctx(&["в"]), "приват", "приват") > 1.0);
        assert_eq!(lm.score(&ctx(&["в"]), "привет", "привет"), 0.0);
        assert_eq!(lm.score(&ctx(&[]), "привет", "привет"), 0.0);
    }

    #[test]
    fn only_last_context_word_matters() {
        let lm = BigramModel::from_tsv_str(LM).unwrap();
        let with_noise = lm.score(&ctx(&["зайди", "в"]), "приват", "приват");
        let direct = lm.score(&ctx(&["в"]), "приват", "приват");
        assert_eq!(with_noise, direct);
    }

    #[test]
    fn context_punctuation_does_not_drop_the_signal() {
        let lm = BigramModel::from_tsv_str(LM).unwrap();
        let clean = lm.score(&ctx(&["ну"]), "привет", "привет");
        assert!(clean > 0.0);
        // Attached punctuation must not silently disable the LM.
        assert_eq!(lm.score(&ctx(&["ну,"]), "привет", "привет"), clean);
        assert_eq!(lm.score(&ctx(&["(ну)"]), "привет", "привет"), clean);
        // A standalone punctuation token falls back to the prior word.
        assert_eq!(lm.score(&ctx(&["ну", "—"]), "привет", "привет"), clean);
    }

    #[test]
    fn rejects_malformed_artifact() {
        assert!(BigramModel::from_tsv_str("u\tслово").is_err());
        assert!(BigramModel::from_tsv_str("x\tслово\t5").is_err());
        assert!(BigramModel::from_tsv_str("b\tа\tб\tмного").is_err());
    }
}
