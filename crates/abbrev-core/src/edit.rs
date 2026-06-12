//! Weighted Damerau–Levenshtein distance tuned for abbreviated Russian
//! input (noisy-channel error model).
//!
//! Direction matters: we compute the cost of turning the *user input*
//! into a *candidate form*. "Insertion" therefore means "the user omitted
//! this character", which is exactly the operation that must be cheap for
//! vowels (`првт` → `привет`) and for `ь`/`ъ`.

use crate::alphabet::{is_sign, is_vowel, keyboard_adjacent};

/// Operation costs of the error model. All costs are non-negative;
/// a full-price edit is `1.0`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EditCosts {
    /// Candidate has a vowel the input omitted (the core abbreviation move).
    pub insert_vowel: f32,
    /// Candidate has `ь`/`ъ` the input omitted.
    pub insert_sign: f32,
    /// Candidate has a consonant the input omitted.
    pub insert_consonant: f32,
    /// Input has an extra character not present in the candidate.
    pub delete: f32,
    /// Substitution between ЙЦУКЕН-adjacent keys (fat-finger typo).
    pub substitute_neighbor: f32,
    /// Substitution of one vowel by another (unstressed-vowel confusion).
    pub substitute_vowel: f32,
    /// Any other substitution.
    pub substitute: f32,
    /// Adjacent transposition (Damerau).
    pub transpose: f32,
}

impl Default for EditCosts {
    fn default() -> Self {
        Self {
            insert_vowel: 0.25,
            insert_sign: 0.35,
            insert_consonant: 1.0,
            delete: 1.1,
            substitute_neighbor: 0.5,
            substitute_vowel: 0.55,
            substitute: 1.0,
            transpose: 0.6,
        }
    }
}

impl EditCosts {
    fn insert(&self, c: char) -> f32 {
        if is_vowel(c) {
            self.insert_vowel
        } else if is_sign(c) {
            self.insert_sign
        } else {
            self.insert_consonant
        }
    }

    fn substitute(&self, a: char, b: char) -> f32 {
        if a == b {
            0.0
        } else if keyboard_adjacent(a, b) {
            self.substitute_neighbor
        } else if is_vowel(a) && is_vowel(b) {
            self.substitute_vowel
        } else {
            self.substitute
        }
    }
}

/// Weighted distance from `input` to `candidate`, both normalized.
/// Returns `None` if the distance provably exceeds `cutoff`
/// (early abandoning keeps per-keystroke latency bounded).
pub fn weighted_distance(
    input: &[char],
    candidate: &[char],
    costs: &EditCosts,
    cutoff: f32,
) -> Option<f32> {
    let n = input.len();
    let m = candidate.len();
    // prev2/prev/cur are rows of the DP matrix over candidate positions.
    let mut prev2: Vec<f32> = vec![0.0; m + 1];
    let mut prev: Vec<f32> = vec![0.0; m + 1];
    let mut cur: Vec<f32> = vec![0.0; m + 1];
    for j in 1..=m {
        prev[j] = prev[j - 1] + costs.insert(candidate[j - 1]);
    }
    for i in 1..=n {
        cur[0] = prev[0] + costs.delete;
        let mut row_min = cur[0];
        for j in 1..=m {
            let sub = prev[j - 1] + costs.substitute(input[i - 1], candidate[j - 1]);
            let del = prev[j] + costs.delete;
            let ins = cur[j - 1] + costs.insert(candidate[j - 1]);
            let mut best = sub.min(del).min(ins);
            if i > 1
                && j > 1
                && input[i - 1] == candidate[j - 2]
                && input[i - 2] == candidate[j - 1]
            {
                best = best.min(prev2[j - 2] + costs.transpose);
            }
            cur[j] = best;
            row_min = row_min.min(best);
        }
        if row_min > cutoff {
            return None;
        }
        std::mem::swap(&mut prev2, &mut prev);
        std::mem::swap(&mut prev, &mut cur);
    }
    let total = prev[m];
    (total <= cutoff).then_some(total)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dist(a: &str, b: &str) -> f32 {
        let a: Vec<char> = a.chars().collect();
        let b: Vec<char> = b.chars().collect();
        weighted_distance(&a, &b, &EditCosts::default(), f32::MAX).unwrap()
    }

    #[test]
    fn vowel_omission_is_cheap() {
        // привет from првт: two vowel insertions.
        assert!((dist("првт", "привет") - 0.5).abs() < 1e-5);
        // A consonant mismatch is much more expensive.
        assert!(dist("првт", "правда") > dist("првт", "привет"));
    }

    #[test]
    fn identical_strings_cost_zero() {
        assert_eq!(dist("привет", "привет"), 0.0);
    }

    #[test]
    fn transposition_beats_two_substitutions() {
        assert!((dist("пирвет", "привет") - 0.6).abs() < 1e-5);
    }

    #[test]
    fn cutoff_prunes() {
        let a: Vec<char> = "првт".chars().collect();
        let b: Vec<char> = "молоко".chars().collect();
        assert_eq!(weighted_distance(&a, &b, &EditCosts::default(), 0.5), None);
    }
}
