//! Local personalization: the engine learns from accepted suggestions.
//!
//! Privacy model: the history never leaves the device. The core only
//! offers TSV import/export; *where* it is stored (and whether at all)
//! is the platform shell's decision.

use std::collections::HashMap;

/// Counts of accepted (input skeleton → form) pairs plus global form counts.
#[derive(Debug, Default)]
pub struct UserHistory {
    by_pair: HashMap<(String, String), u32>,
    by_form: HashMap<String, u32>,
}

impl UserHistory {
    /// Records that the user accepted `form` for an input with `input_skeleton`.
    pub fn accept(&mut self, input_skeleton: &str, form: &str) {
        *self
            .by_pair
            .entry((input_skeleton.to_string(), form.to_string()))
            .or_insert(0) += 1;
        *self.by_form.entry(form.to_string()).or_insert(0) += 1;
    }

    /// Log-scaled prior for ranking. Pair evidence (this exact shorthand →
    /// this form) is weighted above generic form popularity.
    pub fn prior(&self, input_skeleton: &str, form: &str) -> f32 {
        let pair = self
            .by_pair
            .get(&(input_skeleton.to_string(), form.to_string()))
            .copied()
            .unwrap_or(0) as f32;
        let form_count = self.by_form.get(form).copied().unwrap_or(0) as f32;
        2.0 * (1.0 + pair).ln() + 0.5 * (1.0 + form_count).ln()
    }

    /// Serializes to `skeleton<TAB>form<TAB>count` lines (stable order).
    pub fn to_tsv(&self) -> String {
        let mut rows: Vec<_> = self
            .by_pair
            .iter()
            .map(|((skel, form), count)| format!("{skel}\t{form}\t{count}"))
            .collect();
        rows.sort();
        rows.join("\n")
    }

    /// Restores state from [`Self::to_tsv`] output; malformed lines are skipped.
    pub fn from_tsv(tsv: &str) -> Self {
        let mut history = Self::default();
        for line in tsv.lines() {
            let mut parts = line.split('\t');
            if let (Some(skel), Some(form), Some(count)) =
                (parts.next(), parts.next(), parts.next())
                && let Ok(count) = count.trim().parse::<u32>()
            {
                history
                    .by_pair
                    .insert((skel.to_string(), form.to_string()), count);
                *history.by_form.entry(form.to_string()).or_insert(0) += count;
            }
        }
        history
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accept_raises_prior() {
        let mut h = UserHistory::default();
        let before = h.prior("првт", "приват");
        h.accept("првт", "приват");
        h.accept("првт", "приват");
        assert!(h.prior("првт", "приват") > before);
        // Other forms get only the weak global component.
        assert!(h.prior("првт", "приват") > h.prior("прв", "приват"));
    }

    #[test]
    fn tsv_roundtrip() {
        let mut h = UserHistory::default();
        h.accept("првт", "привет");
        h.accept("првт", "привет");
        h.accept("тстрн", "тестирование");
        let restored = UserHistory::from_tsv(&h.to_tsv());
        assert_eq!(restored.prior("првт", "привет"), h.prior("првт", "привет"));
    }
}
