//! Candidate scoring: a linear synthesis of the signals validated by the
//! spelling-correction / IME literature.
//!
//! ```text
//! score(candidate) =
//!   w_skel  * skeleton_match
//! + w_suf   * suffix_compatibility
//! - w_edit  * weighted_edit_distance
//! + w_freq  * log_frequency
//! + w_ctx   * context_lm_score
//! + w_user  * user_history_prior
//! + w_morph * morph_compatibility
//! ```
//!
//! `morph_compatibility` is case agreement with a preceding preposition
//! (`в работе`, not `в работу`), available once the lexicon carries
//! grammemes; it is a soft, never-negative boost.

/// Weights of the linear ranking model. Tuned on the offline benchmark
/// (`abbrev-cli bench`), not by intuition.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Weights {
    pub skeleton: f32,
    pub suffix: f32,
    /// Reward for the candidate literally starting with the typed input
    /// (plain completion: `степ` → степени, not the fuzzier стоп).
    pub prefix: f32,
    pub edit: f32,
    pub freq: f32,
    pub context: f32,
    pub user: f32,
    /// Reward for grammatical case agreement with the left context
    /// (preposition government).
    pub morph: f32,
}

impl Default for Weights {
    fn default() -> Self {
        Self {
            // The stem skeleton must outweigh raw frequency: for `тстрния`
            // the user means тестирования, not the more frequent история.
            skeleton: 3.0,
            suffix: 1.0,
            prefix: 2.0,
            edit: 1.5,
            freq: 0.6,
            context: 1.0,
            user: 1.0,
            morph: 1.0,
        }
    }
}

/// Per-candidate signals collected by the engine before scoring.
#[derive(Debug, Clone, Copy)]
pub struct Signals {
    /// Graded skeleton agreement in [0, 1]: 1.0 — input skeleton equals the
    /// candidate's, otherwise the common-prefix share of the input skeleton
    /// (`тстрн` vs `тстрвн` → 4/5). Users keep the first letters of the stem,
    /// so prefix agreement of skeletons is the strongest stem signal.
    pub skeleton_match: f32,
    /// Longest common ending of input and candidate, in chars, capped at 3
    /// and normalized to [0, 1].
    pub suffix_compatibility: f32,
    /// Common char prefix of input and candidate, normalized by input
    /// length: 1.0 means the candidate is a pure completion of the input.
    pub prefix_agreement: f32,
    /// Weighted edit distance (lower is better).
    pub edit_distance: f32,
    /// `ln(1 + ipm)` of the candidate form.
    pub log_frequency: f32,
    /// Context-model score (0.0 when no model is plugged in).
    pub context: f32,
    /// User-history prior.
    pub user_prior: f32,
    /// Case agreement with the preceding preposition (0.0 or 1.0).
    pub morph_compatibility: f32,
}

pub fn score(signals: &Signals, w: &Weights) -> f32 {
    w.skeleton * signals.skeleton_match
        + w.suffix * signals.suffix_compatibility
        + w.prefix * signals.prefix_agreement
        - w.edit * signals.edit_distance
        + w.freq * signals.log_frequency
        + w.context * signals.context
        + w.user * signals.user_prior
        + w.morph * signals.morph_compatibility
}

/// Longest common prefix of two char slices.
pub fn common_prefix_len(a: &[char], b: &[char]) -> usize {
    a.iter().zip(b.iter()).take_while(|(x, y)| x == y).count()
}

/// Longest common ending of two char slices, capped at `cap`.
pub fn common_ending_len(a: &[char], b: &[char], cap: usize) -> usize {
    a.iter()
        .rev()
        .zip(b.iter().rev())
        .take(cap)
        .take_while(|(x, y)| x == y)
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frequency_breaks_ties() {
        let w = Weights::default();
        let base = Signals {
            skeleton_match: 1.0,
            suffix_compatibility: 0.0,
            prefix_agreement: 0.0,
            edit_distance: 0.5,
            log_frequency: 0.0,
            context: 0.0,
            user_prior: 0.0,
            morph_compatibility: 0.0,
        };
        let frequent = Signals {
            log_frequency: 5.0,
            ..base
        };
        assert!(score(&frequent, &w) > score(&base, &w));
    }

    #[test]
    fn common_prefix() {
        let a: Vec<char> = "тстрн".chars().collect();
        let b: Vec<char> = "тстрвн".chars().collect();
        assert_eq!(common_prefix_len(&a, &b), 4);
        let c: Vec<char> = "нстрн".chars().collect();
        assert_eq!(common_prefix_len(&a, &c), 0);
    }

    #[test]
    fn common_ending() {
        let a: Vec<char> = "тстрние".chars().collect();
        let b: Vec<char> = "тестирование".chars().collect();
        assert_eq!(common_ending_len(&a, &b, 3), 3);
        let c: Vec<char> = "привет".chars().collect();
        assert_eq!(common_ending_len(&a, &c, 3), 0);
    }
}
