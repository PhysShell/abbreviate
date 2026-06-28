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

/// Entries older than this many half-lives have a prior below `0.5^16`
/// (~1.5e-5) — negligible against every other ranking signal. Past the
/// horizon they are evicted to bound memory in long-lived sessions; since
/// their contribution is ~0, removal changes no ranking.
const PRUNE_HALF_LIVES: f32 = 16.0;

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
    /// normalized (lowercase, `ё→е`) before storage, the same way
    /// [`prior`](Self::prior) normalizes its lookup.
    pub fn note(&mut self, word: &str) {
        let key = normalize(word.trim());
        if key.is_empty() {
            return;
        }
        self.tick += 1;
        self.last_seen.insert(key, self.tick);
        // Bound memory in long-lived sessions: `note` runs once per committed
        // word, so without eviction `last_seen` would grow with the whole
        // session vocabulary. Entries past the prune horizon have a negligible
        // prior (see `PRUNE_HALF_LIVES`), so dropping them changes no ranking.
        // Sweep only past twice the horizon, keeping `note` amortized O(1):
        // all `last_seen` ticks are distinct, so at most `horizon` entries
        // survive a sweep, and we add at most `horizon` before the next one.
        let horizon = self.prune_horizon();
        if self.last_seen.len() as u64 > 2 * horizon {
            let cutoff = self.tick.saturating_sub(horizon);
            self.last_seen.retain(|_, &mut seen| seen > cutoff);
        }
    }

    /// Recency prior for a raw `word`, in `[0, 1]`: `1.0` the instant after it
    /// was observed, halving every `half_life` words since. `0.0` for a form
    /// never seen this session — neutral, never negative (absence of recency
    /// must not bury a candidate). The input is normalized the same way as
    /// [`note`](Self::note), so the same spelling always matches.
    pub fn prior(&self, word: &str) -> f32 {
        self.prior_normalized(&normalize(word.trim()))
    }

    /// Hot-path variant of [`prior`](Self::prior) for a key the caller has
    /// already normalized (the engine computes the form's normalized spelling
    /// once per candidate, so re-normalizing here would allocate per
    /// candidate per keystroke for nothing).
    pub(crate) fn prior_normalized(&self, form_norm: &str) -> f32 {
        match self.last_seen.get(form_norm) {
            Some(&seen) => {
                let elapsed = self.tick.saturating_sub(seen) as f32;
                0.5f32.powf(elapsed / self.half_life)
            }
            None => 0.0,
        }
    }

    /// Tick age past which an entry's prior is negligible and it is evicted.
    fn prune_horizon(&self) -> u64 {
        (self.half_life * PRUNE_HALF_LIVES).ceil() as u64
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
    fn note_and_prior_agree_on_raw_spelling() {
        let mut c = SessionCache::default();
        c.note("  Синхрофазотрён ");
        // prior normalizes its input the same way note does, so any case / ё /
        // whitespace variant of the same word matches (no normalize-only trap).
        assert_eq!(c.prior("Синхрофазотрён"), 1.0);
        assert_eq!(c.prior("синхрофазотрен"), 1.0);
        assert_eq!(c.prior(" СИНХРОФАЗОТРЕН "), 1.0);
        // The hot-path variant still expects an already-normalized key.
        assert_eq!(c.prior_normalized("синхрофазотрен"), 1.0);
    }

    #[test]
    fn long_session_evicts_stale_entries() {
        let mut c = SessionCache::with_half_life(4.0);
        let horizon = (4.0 * PRUNE_HALF_LIVES).ceil() as usize;
        // Far more unique words than the horizon: memory must stay bounded.
        for i in 0..10_000 {
            c.note(&format!("слово{i}"));
        }
        assert!(
            c.last_seen.len() <= 2 * horizon,
            "cache grew unbounded: {}",
            c.last_seen.len()
        );
        // A long-evicted word reads as neutral; a recent one still boosts.
        assert_eq!(c.prior("слово0"), 0.0);
        assert!(c.prior("слово9999") > 0.9);
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
