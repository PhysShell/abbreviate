//! Ephemeral session recency cache: a word used in the current context
//! starts surfacing where it was typed and decays as the topic moves on.
//!
//! This is the in-document **burstiness** half of a classic cache language
//! model (Kuhn & De Mori, 1990): a word that just appeared is much likelier
//! to appear again *soon*. The engine consumes it as one more ranking signal
//! ([`crate::rank`]), additive alongside frequency and context — see
//! `docs/RESEARCH-RECENCY-CACHE.md`.
//!
//! Two invariants distinguish it from [`crate::history::UserHistory`]:
//!
//! * **Sans-IO decay by *logical tick*, not wall-clock.** The core has no
//!   clock (ADR-0002) and must stay deterministic, so "how long ago" is
//!   measured in *words observed*, not seconds. The shell drives the stream
//!   via [`SessionCache::note`]; the same stream replays identically in
//!   tests and benchmarks.
//! * **Ephemeral and unsynced.** Unlike history (CRDT-merged across
//!   devices), the cache is local, never serialized, never merged. It only
//!   ever lives in memory for the current session/app, so privacy is free.

use std::collections::HashMap;

use crate::alphabet::normalize;

/// Default half-life of the recency boost, in *words observed*. A form's
/// prior halves every `half_life` words typed since it was last seen.
pub const DEFAULT_HALF_LIFE: f32 = 80.0;

/// Per-session recency cache: last logical tick at which each form was seen.
#[derive(Debug, Clone)]
pub struct SessionCache {
    /// Monotonic count of observed words; advances once per [`note`](Self::note).
    tick: u64,
    /// Normalized form → tick at which it was last observed.
    last_seen: HashMap<String, u64>,
    /// Decay half-life in observed words.
    half_life: f32,
}

impl Default for SessionCache {
    fn default() -> Self {
        Self::with_half_life(DEFAULT_HALF_LIFE)
    }
}

impl SessionCache {
    /// Creates an empty cache with an explicit decay half-life (in words).
    pub fn with_half_life(half_life: f32) -> Self {
        Self {
            tick: 0,
            last_seen: HashMap::new(),
            // A non-positive half-life would make decay ill-defined; clamp it
            // to a tiny positive so `note`/`prior` stay well-behaved.
            half_life: half_life.max(f32::MIN_POSITIVE),
        }
    }

    /// Records that the shell observed `word` in the current context (a
    /// committed word, not the in-progress input). The raw word is
    /// normalized (lowercase, `ё→е`) before storage, matching the form key
    /// used by [`prior`](Self::prior).
    pub fn note(&mut self, word: &str) {
        let key = normalize(word.trim());
        if key.is_empty() {
            return;
        }
        self.tick += 1;
        self.last_seen.insert(key, self.tick);
    }

    /// Recency prior for an already-normalized form, in `[0, 1]`: `1.0` the
    /// instant after it was observed, halving every `half_life` words since.
    /// `0.0` for a form never seen this session — neutral, never negative
    /// (absence of recency must not bury a candidate).
    pub fn prior(&self, form_norm: &str) -> f32 {
        match self.last_seen.get(form_norm) {
            Some(&seen) => {
                let elapsed = self.tick.saturating_sub(seen) as f32;
                0.5f32.powf(elapsed / self.half_life)
            }
            None => 0.0,
        }
    }

    /// Clears the cache — the shell calls this when the context changes
    /// (e.g. a different app/field), so recency never leaks across contexts.
    pub fn reset(&mut self) {
        self.tick = 0;
        self.last_seen.clear();
    }

    /// Whether nothing has been observed yet (a pure cache has no effect).
    pub fn is_empty(&self) -> bool {
        self.last_seen.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn just_noted_form_has_full_prior() {
        let mut c = SessionCache::default();
        c.note("синхрофазотрон");
        assert_eq!(c.prior("синхрофазотрон"), 1.0);
    }

    #[test]
    fn unseen_form_is_neutral() {
        let c = SessionCache::default();
        assert_eq!(c.prior("синхрофазотрон"), 0.0);
    }

    #[test]
    fn prior_decays_with_words_observed() {
        let mut c = SessionCache::with_half_life(10.0);
        c.note("привет");
        let fresh = c.prior("привет");
        for _ in 0..10 {
            c.note("слово");
        }
        let later = c.prior("привет");
        // Exactly one half-life elapsed → halved (within float tolerance).
        assert!((later - 0.5).abs() < 1e-5, "got {later}");
        assert!(later < fresh);
        // Far down the conversation it is effectively gone.
        for _ in 0..200 {
            c.note("слово");
        }
        assert!(c.prior("привет") < 0.01);
    }

    #[test]
    fn note_normalizes_the_word() {
        let mut c = SessionCache::default();
        c.note("  Синхрофазотрён ");
        // Normalized key (lowercase, ё→е, trimmed) is what prior expects.
        assert_eq!(c.prior("синхрофазотрен"), 1.0);
        assert_eq!(c.prior("Синхрофазотрён"), 0.0);
    }

    #[test]
    fn blank_word_is_ignored() {
        let mut c = SessionCache::default();
        c.note("   ");
        assert!(c.is_empty());
    }

    #[test]
    fn reset_clears_everything() {
        let mut c = SessionCache::default();
        c.note("привет");
        c.reset();
        assert!(c.is_empty());
        assert_eq!(c.prior("привет"), 0.0);
    }

    #[test]
    fn re_noting_refreshes_recency() {
        let mut c = SessionCache::with_half_life(10.0);
        c.note("привет");
        for _ in 0..10 {
            c.note("слово");
        }
        assert!((c.prior("привет") - 0.5).abs() < 1e-5);
        // Mentioning it again resets the clock to full.
        c.note("привет");
        assert_eq!(c.prior("привет"), 1.0);
    }

    #[test]
    fn deterministic_for_same_stream() {
        let run = || {
            let mut c = SessionCache::with_half_life(5.0);
            for w in ["а", "привет", "б", "в", "привет", "г"] {
                c.note(w);
            }
            c.prior("привет")
        };
        assert_eq!(run(), run());
    }
}
