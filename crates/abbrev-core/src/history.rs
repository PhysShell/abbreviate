//! Local personalization: the engine learns from confirmed suggestions and
//! is penalized by reverted ones.
//!
//! Two principles, both load-bearing for the product roadmap:
//!
//! * **A tap is not a success.** Counting raw acceptances trains on a
//!   corrupted signal (the user may tap, see the result, and undo). So the
//!   history tracks `confirmed` vs `reverted` separately, and the ranking
//!   prior is *net* evidence — it can go negative for pairs the user keeps
//!   rejecting.
//! * **Stay mergeable.** Everything is a sum of counters, so two devices'
//!   histories merge by summing per pair (CRDT-style). This keeps future
//!   cross-device sync to "move an opaque blob + [`merge`]", with no
//!   conflict resolution. Do not add non-summable state (e.g. "last choice"
//!   without counts) — it would break this property.
//!
//! Privacy model: the history never leaves the device. The core only offers
//! TSV import/export and merge; *where* it is stored (and whether at all,
//! and whether it is ever synced) is the platform shell's decision.

use std::collections::HashMap;

/// Confirmed/reverted counts for one (input skeleton → form) pair.
#[derive(Debug, Default, Clone, Copy)]
struct PairStats {
    confirmed: u32,
    reverted: u32,
}

/// Per-user acceptance history. `by_form` (global confirmed popularity) is
/// derived from `by_pair` and kept in sync on every mutation.
#[derive(Debug, Default)]
pub struct UserHistory {
    by_pair: HashMap<(String, String), PairStats>,
    by_form: HashMap<String, u32>,
}

impl UserHistory {
    /// Records that the user **confirmed** `form` for `input_skeleton`
    /// (picked it and kept it). Positive evidence.
    pub fn confirm(&mut self, input_skeleton: &str, form: &str) {
        self.by_pair
            .entry((input_skeleton.to_string(), form.to_string()))
            .or_default()
            .confirmed += 1;
        *self.by_form.entry(form.to_string()).or_insert(0) += 1;
    }

    /// Records that the user **reverted** `form` for `input_skeleton`
    /// (undid/edited it after it was inserted). Negative evidence — it does
    /// not raise global form popularity.
    pub fn reject(&mut self, input_skeleton: &str, form: &str) {
        self.by_pair
            .entry((input_skeleton.to_string(), form.to_string()))
            .or_default()
            .reverted += 1;
    }

    /// Net log-scaled prior for ranking. Pair evidence
    /// (`ln(1+confirmed) − ln(1+reverted)`) dominates generic form
    /// popularity; it is **negative** when reverts outweigh confirmations,
    /// so a repeatedly-rejected pair sinks in the ranking.
    pub fn prior(&self, input_skeleton: &str, form: &str) -> f32 {
        let stats = self
            .by_pair
            .get(&(input_skeleton.to_string(), form.to_string()))
            .copied()
            .unwrap_or_default();
        let pair_signal = (1.0 + stats.confirmed as f32).ln() - (1.0 + stats.reverted as f32).ln();
        let form_count = self.by_form.get(form).copied().unwrap_or(0) as f32;
        2.0 * pair_signal + 0.5 * (1.0 + form_count).ln()
    }

    /// Merges another history into this one by summing counters per pair —
    /// the operation behind cross-device sync. Commutative and idempotent
    /// only up to count addition (it is additive, not set-union), so callers
    /// must merge *deltas* or distinct devices, not the same blob twice.
    pub fn merge(&mut self, other: &UserHistory) {
        for (key, stats) in &other.by_pair {
            let slot = self.by_pair.entry(key.clone()).or_default();
            slot.confirmed += stats.confirmed;
            slot.reverted += stats.reverted;
            if stats.confirmed > 0 {
                *self.by_form.entry(key.1.clone()).or_insert(0) += stats.confirmed;
            }
        }
    }

    /// Serializes to `skeleton<TAB>form<TAB>confirmed<TAB>reverted` lines
    /// (stable order). Pairs with no evidence are omitted.
    pub fn to_tsv(&self) -> String {
        let mut rows: Vec<_> = self
            .by_pair
            .iter()
            .filter(|(_, s)| s.confirmed > 0 || s.reverted > 0)
            .map(|((skel, form), s)| format!("{skel}\t{form}\t{}\t{}", s.confirmed, s.reverted))
            .collect();
        rows.sort();
        rows.join("\n")
    }

    /// Restores from [`Self::to_tsv`] output. Accepts the legacy 3-column
    /// format (`skeleton<TAB>form<TAB>count`, treated as confirmed) so old
    /// persisted blobs still load. Malformed lines are skipped.
    pub fn from_tsv(tsv: &str) -> Self {
        let mut history = Self::default();
        for line in tsv.lines() {
            let mut parts = line.split('\t');
            let (Some(skel), Some(form), Some(confirmed)) =
                (parts.next(), parts.next(), parts.next())
            else {
                continue;
            };
            let Ok(confirmed) = confirmed.trim().parse::<u32>() else {
                continue;
            };
            let reverted = parts
                .next()
                .and_then(|r| r.trim().parse::<u32>().ok())
                .unwrap_or(0);
            history.by_pair.insert(
                (skel.to_string(), form.to_string()),
                PairStats {
                    confirmed,
                    reverted,
                },
            );
            if confirmed > 0 {
                *history.by_form.entry(form.to_string()).or_insert(0) += confirmed;
            }
        }
        history
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn confirm_raises_prior() {
        let mut h = UserHistory::default();
        let before = h.prior("првт", "приват");
        h.confirm("првт", "приват");
        h.confirm("првт", "приват");
        assert!(h.prior("првт", "приват") > before);
        // Other forms get only the weak global component.
        assert!(h.prior("првт", "приват") > h.prior("прв", "приват"));
    }

    #[test]
    fn reverts_push_prior_negative() {
        let mut h = UserHistory::default();
        h.confirm("првт", "приват");
        let positive = h.prior("првт", "приват");
        for _ in 0..4 {
            h.reject("првт", "приват");
        }
        let after = h.prior("првт", "приват");
        assert!(after < positive);
        assert!(
            after < 0.0,
            "repeated reverts must sink the prior, got {after}"
        );
    }

    #[test]
    fn tsv_roundtrip_preserves_confirmed_and_reverted() {
        let mut h = UserHistory::default();
        h.confirm("првт", "привет");
        h.confirm("првт", "привет");
        h.reject("првт", "приват");
        let restored = UserHistory::from_tsv(&h.to_tsv());
        assert_eq!(restored.prior("првт", "привет"), h.prior("првт", "привет"));
        assert_eq!(restored.prior("првт", "приват"), h.prior("првт", "приват"));
    }

    #[test]
    fn legacy_three_column_blob_still_loads() {
        // Old format: skeleton<TAB>form<TAB>count (count == confirmed).
        let h = UserHistory::from_tsv("првт\tпривет\t3");
        let fresh = {
            let mut x = UserHistory::default();
            for _ in 0..3 {
                x.confirm("првт", "привет");
            }
            x
        };
        assert_eq!(h.prior("првт", "привет"), fresh.prior("првт", "привет"));
    }

    #[test]
    fn merge_sums_counters_across_devices() {
        let mut phone = UserHistory::default();
        phone.confirm("првт", "привет");
        let mut laptop = UserHistory::default();
        laptop.confirm("првт", "привет");
        laptop.reject("првт", "приват");

        let mut merged = UserHistory::default();
        merged.merge(&phone);
        merged.merge(&laptop);

        // Two confirms of the same pair across devices stack.
        let mut twice = UserHistory::default();
        twice.confirm("првт", "привет");
        twice.confirm("првт", "привет");
        twice.reject("првт", "приват");
        assert_eq!(merged.to_tsv(), twice.to_tsv());
    }
}
