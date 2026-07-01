//! Window tone / register meter (research §5.1).
//!
//! The motivating want is *register switching* — writing "приветик, как ты" to
//! mother and "ты живой, нет?" to a friend — without a per-chat identifier
//! (which an IME cannot get; see §5.1). The insight there: **tone is a
//! property of the content, not the address.** So we read it off the same
//! committed-word stream the recency cache already consumes ([`note`]), never
//! from chat identity, and it costs no new permission.
//!
//! Mechanism, deliberately small and sans-IO:
//!
//! * a build-time **signed marker list** maps a lemma/word to a sign in
//!   `[-1, +1]` — polite markers positive (`пожалуйста`, `здравствуйте`), crude
//!   markers negative (slang, profanity);
//! * two accumulators decayed **per noted word** (logical tick, not wall-clock,
//!   per ADR-0002) track the recent signed average and a confidence mass;
//! * [`register`](ToneMeter::register) reports `Polite` / `Crude` only when
//!   enough recent marker mass supports it, else `Neutral`. A long neutral
//!   stretch fades the mass below the floor, so the window re-centres on its
//!   own — the same self-cleaning behaviour as recency.
//!
//! This is the *measurement*. Two clients read it: the profanity-masking gate
//! (§5.2 — mask when the window is polite) and, later, a register ranking
//! signal (§5.1 — nudge slang ↔ neutral forms). Keeping the meter separate and
//! tested first is the project's "metrics before models" rule.

use std::collections::HashMap;
use std::fmt;

use crate::alphabet::normalize;

/// Coarse register of the recent window, from the tone meter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Register {
    /// Polite / formal window (positive marker mass dominates).
    Polite,
    /// Not enough recent signal either way — the default.
    Neutral,
    /// Crude / slangy window (negative marker mass dominates).
    Crude,
}

/// Tone half-life in words: the influence of a marker halves every this-many
/// noted words. Shorter than recency's 80 — register turns over faster than
/// vocabulary (a couple of polite lines should re-colour the window).
pub const DEFAULT_HALF_LIFE: f32 = 12.0;

/// `|signed average|` must exceed this for a non-`Neutral` verdict — a small
/// dead zone so a lone off-sign marker doesn't flip the register.
const REGISTER_THRESHOLD: f32 = 0.25;

/// Minimum decayed marker mass before the meter commits to a register. Below
/// it the recent window carries too little signal and stays `Neutral`.
const MIN_CONFIDENCE: f32 = 0.5;

/// Exponentially-decayed tone estimate over the committed-word stream.
#[derive(Debug, Default)]
pub struct ToneMeter {
    /// normalized lemma/word -> sign in [-1, 1].
    markers: HashMap<String, f32>,
    /// Decayed signed sum of recent marker signs.
    score: f32,
    /// Decayed count of recent markers (confidence).
    mass: f32,
    /// Per-word decay factor `0.5^(1/half_life)`, precomputed — `note` runs on
    /// a per-committed-word hot path and the half-life never changes.
    decay: f32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToneError {
    pub line: usize,
    pub message: String,
}

impl fmt::Display for ToneError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "tone markers line {}: {}", self.line, self.message)
    }
}

impl std::error::Error for ToneError {}

impl ToneMeter {
    /// Empty meter (no markers) at the default half-life — reports `Neutral`
    /// for everything, so tone is inert until a marker list is loaded.
    pub fn new() -> Self {
        Self::with_half_life(DEFAULT_HALF_LIFE)
    }

    pub fn with_half_life(half_life: f32) -> Self {
        let half_life = half_life.max(f32::MIN_POSITIVE);
        Self {
            markers: HashMap::new(),
            score: 0.0,
            mass: 0.0,
            decay: 0.5f32.powf(1.0 / half_life),
        }
    }

    /// Parses `word_or_lemma<TAB>sign`, where `sign` is a float in `[-1, 1]`
    /// (positive = polite, negative = crude). Blank lines and `#` comments are
    /// skipped; keys are normalized so any spelling/case matches. A bare `+`
    /// or `-` shorthand is accepted for `±1`.
    pub fn markers_from_tsv_str(&mut self, tsv: &str) -> Result<(), ToneError> {
        for (i, raw) in tsv.lines().enumerate() {
            let line = raw.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let mut parts = line.split('\t');
            let (Some(word), Some(sign_str)) = (parts.next(), parts.next()) else {
                return Err(ToneError {
                    line: i + 1,
                    message: format!("expected `word\\tsign`, got `{raw}`"),
                });
            };
            let sign = match sign_str.trim() {
                "+" => 1.0,
                "-" => -1.0,
                s => s.parse::<f32>().map_err(|_| ToneError {
                    line: i + 1,
                    message: format!("sign `{s}` is not a number in [-1, 1]"),
                })?,
            };
            if !(-1.0..=1.0).contains(&sign) || sign.is_nan() {
                return Err(ToneError {
                    line: i + 1,
                    message: format!("sign `{sign}` out of range [-1, 1]"),
                });
            }
            self.markers.insert(normalize(word.trim()), sign);
        }
        Ok(())
    }

    pub fn is_empty(&self) -> bool {
        self.markers.is_empty()
    }

    /// Folds one committed word into the running tone. Every word decays the
    /// accumulators (so tone fades as the window moves on); a marker word also
    /// adds its sign. Non-marker words carry no sign — they only age the
    /// estimate.
    pub fn note(&mut self, word: &str) {
        self.score *= self.decay;
        self.mass *= self.decay;
        if let Some(&sign) = self.markers.get(&normalize(word)) {
            self.score += sign;
            self.mass += 1.0;
        }
    }

    /// Clears the running estimate (marker list is kept). The shell calls this
    /// on a context change, like the recency cache reset.
    pub fn reset(&mut self) {
        self.score = 0.0;
        self.mass = 0.0;
    }

    /// Signed tone of the recent window in `[-1, 1]` (`+` polite, `-` crude,
    /// `0` when there is no recent marker mass).
    pub fn tone(&self) -> f32 {
        if self.mass <= f32::MIN_POSITIVE {
            0.0
        } else {
            (self.score / self.mass).clamp(-1.0, 1.0)
        }
    }

    /// Coarse register, with a confidence floor and a dead zone: `Neutral`
    /// until enough recent marker mass leans clearly one way.
    pub fn register(&self) -> Register {
        if self.mass < MIN_CONFIDENCE {
            return Register::Neutral;
        }
        let tone = self.tone();
        if tone > REGISTER_THRESHOLD {
            Register::Polite
        } else if tone < -REGISTER_THRESHOLD {
            Register::Crude
        } else {
            Register::Neutral
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meter() -> ToneMeter {
        let mut m = ToneMeter::new();
        m.markers_from_tsv_str(
            "пожалуйста\t+\nздравствуйте\t1\nспасибо\t0.7\n\
             блин\t-0.5\nхрень\t-\nдолбоёб\t-1\n",
        )
        .unwrap();
        m
    }

    #[test]
    fn empty_meter_is_neutral() {
        let m = ToneMeter::new();
        assert!(m.is_empty());
        assert_eq!(m.register(), Register::Neutral);
        assert_eq!(m.tone(), 0.0);
    }

    #[test]
    fn polite_and_crude_windows_are_classified() {
        let mut m = meter();
        for _ in 0..3 {
            m.note("пожалуйста");
            m.note("спасибо");
        }
        assert_eq!(m.register(), Register::Polite);
        assert!(m.tone() > 0.0);

        m.reset();
        for _ in 0..3 {
            m.note("хрень");
            m.note("долбоёб");
        }
        assert_eq!(m.register(), Register::Crude);
        assert!(m.tone() < 0.0);
    }

    #[test]
    fn non_markers_do_not_set_a_register() {
        let mut m = meter();
        for w in ["привет", "как", "дела", "сегодня"] {
            m.note(w);
        }
        assert_eq!(m.register(), Register::Neutral);
    }

    #[test]
    fn tone_fades_back_to_neutral_over_a_neutral_stretch() {
        let mut m = meter();
        m.note("здравствуйте");
        assert_eq!(m.register(), Register::Polite);
        // A run of neutral words decays the marker mass below the confidence
        // floor, so the window re-centres itself.
        for _ in 0..40 {
            m.note("слово");
        }
        assert_eq!(m.register(), Register::Neutral);
    }

    #[test]
    fn reset_clears_the_window() {
        let mut m = meter();
        m.note("здравствуйте");
        m.reset();
        assert_eq!(m.register(), Register::Neutral);
        assert_eq!(m.tone(), 0.0);
    }

    #[test]
    fn rejects_malformed_markers() {
        let mut m = ToneMeter::new();
        assert!(m.markers_from_tsv_str("пожалуйста").is_err());
        assert!(m.markers_from_tsv_str("слово\t2.0").is_err());
        assert!(m.markers_from_tsv_str("слово\tвверх").is_err());
    }
}
