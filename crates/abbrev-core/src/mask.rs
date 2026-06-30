//! Profanity masking: an opt-in, candidate-*offering* substitution layer
//! (`долбоёб → дол@#&б`). A neighbour of [`shortcuts`](crate::shortcuts) —
//! both are deterministic, build-time, sans-IO substitution layers sitting
//! beside the fuzzy machinery — but masking only ever *offers* a masked twin
//! next to the original word; it never silently rewrites it. The original
//! stays in the strip, so the layer remains inside the project's "suggest,
//! don't touch" contract (see `docs/RESEARCH-RECENCY-CACHE.md` §5.2).
//!
//! Two concerns are split on purpose:
//!
//! * **mechanism** (here) — *what* to mask: a build-time set of profanity
//!   **lemmas** plus a deterministic masking rule. Pure and testable.
//! * **policy** (the shell, and the future register signal of §5.1) — *when*
//!   to mask: gating is `EngineConfig::mask` here, and a tone gate later. The
//!   default is **off**, so masking costs nothing and changes nothing until a
//!   host turns it on.
//!
//! **Detection is lemma-keyed, not substring** — the Scunthorpe problem. A
//! naive substring filter censors innocent words (`застрахуй`, place names);
//! Russian makes it worse with inflection and obfuscation. Matching the
//! *lemma* of a candidate (the engine already carries one per suggestion)
//! against the censor set means `застрахуй`, whose lemma is not profane,
//! passes untouched.

use std::collections::HashSet;
use std::fmt;

use crate::alphabet::normalize;

/// A set of profanity lemmas plus the (stateless) masking rule.
#[derive(Debug, Default)]
pub struct Masker {
    lemmas: HashSet<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MaskError {
    pub line: usize,
    pub message: String,
}

impl fmt::Display for MaskError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "mask list line {}: {}", self.line, self.message)
    }
}

impl std::error::Error for MaskError {}

/// Symbols cycled through to replace masked interior letters. The order
/// reproduces the motivating example `долбоёб → дол@#&б`.
const MASK_SYMBOLS: [char; 7] = ['@', '#', '&', '%', '$', '!', '*'];

/// Letters kept at the tail of a masked word (the head keep grows with
/// length; see [`Masker::mask_form`]).
const TAIL_KEEP: usize = 1;

impl Masker {
    /// Parses one profanity **lemma** per line. Blank lines and `#` comments
    /// are skipped; keys are normalized so an inflected/`ё`-spelled list
    /// still matches the engine's normalized lemmas. A line carrying more
    /// than one whitespace-separated token is rejected — a lemma is one word,
    /// and a stray column is almost certainly a mistake.
    pub fn from_list_str(text: &str) -> Result<Self, MaskError> {
        let mut lemmas = HashSet::new();
        for (i, raw) in text.lines().enumerate() {
            let line = raw.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if line.split_whitespace().count() != 1 {
                return Err(MaskError {
                    line: i + 1,
                    message: format!("expected one lemma per line, got `{raw}`"),
                });
            }
            lemmas.insert(normalize(line));
        }
        Ok(Self { lemmas })
    }

    /// True when `lemma` (in any spelling/case) is on the censor list.
    pub fn is_masked_lemma(&self, lemma: &str) -> bool {
        self.lemmas.contains(&normalize(lemma))
    }

    pub fn is_empty(&self) -> bool {
        self.lemmas.is_empty()
    }

    /// Masks a surface form: keeps a head of `ceil(letters / 3)` letters and
    /// the final letter, replacing the interior letters with cycling
    /// [`MASK_SYMBOLS`]. Hyphens are kept in place (`мать-и-мачеха`-shaped
    /// words stay readable around the mask). Deterministic and sans-IO.
    ///
    /// Returns `None` when the word is too short to mask without revealing it
    /// whole (fewer than head + tail + 1 letters) — then there is nothing
    /// worth offering.
    pub fn mask_form(form: &str) -> Option<String> {
        let chars: Vec<char> = form.chars().collect();
        let letters = chars.iter().filter(|c| **c != '-').count();
        let head_keep = letters.div_ceil(3).max(1);
        if head_keep + TAIL_KEEP >= letters {
            return None;
        }
        let mut out = String::with_capacity(form.len());
        let mut letter_idx = 0usize;
        let mut sym = 0usize;
        for &c in &chars {
            if c == '-' {
                out.push('-');
                continue;
            }
            if letter_idx >= head_keep && letter_idx < letters - TAIL_KEEP {
                out.push(MASK_SYMBOLS[sym % MASK_SYMBOLS.len()]);
                sym += 1;
            } else {
                out.push(c);
            }
            letter_idx += 1;
        }
        Some(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_normalizes_lemmas() {
        // `ё` folds to `е` under normalize, and case is ignored.
        let m = Masker::from_list_str("# profanity\nдолбоёб\nХРЕНЬ\n").unwrap();
        assert!(m.is_masked_lemma("долбоеб"));
        assert!(m.is_masked_lemma("Долбоёб"));
        assert!(m.is_masked_lemma("хрень"));
        assert!(!m.is_masked_lemma("страховка"));
    }

    #[test]
    fn rejects_multi_token_lines() {
        assert!(Masker::from_list_str("долбоёб лишнее").is_err());
        assert!(Masker::from_list_str("a\tb").is_err());
    }

    #[test]
    fn empty_list_is_empty() {
        assert!(
            Masker::from_list_str("\n# only a comment\n")
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn masks_interior_keeping_head_and_tail() {
        // The motivating example, exactly: keep "дол", mask "боё" → "@#&",
        // keep final "б".
        assert_eq!(Masker::mask_form("долбоёб").as_deref(), Some("дол@#&б"));
    }

    #[test]
    fn preserves_hyphens() {
        let masked = Masker::mask_form("иди-нахер").unwrap();
        // Hyphen stays at its position; only letters are masked.
        assert!(masked.contains('-'));
        assert_eq!(masked.chars().filter(|c| *c == '-').count(), 1);
        // First and last letters survive.
        assert!(masked.starts_with('и'));
        assert!(masked.ends_with('р'));
    }

    #[test]
    fn too_short_to_mask_returns_none() {
        // Two letters: head(1) + tail(1) leaves nothing to mask.
        assert_eq!(Masker::mask_form("ну"), None);
    }
}
