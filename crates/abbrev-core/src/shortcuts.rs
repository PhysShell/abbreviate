//! Conventional shortcuts: a separate exact-match layer for community
//! shorthand that the fuzzy machinery cannot recover reliably
//! (`спс → спасибо`, `крч → короче`, `мб → может быть`).
//!
//! Kept apart from the lexicon on purpose: these are *conventions*, not
//! corrupted dictionary words, and mixing them into frequency ranking
//! would distort ordinary suggestions. They fire only on an exact
//! normalized match of the typed shorthand — including inputs shorter than
//! the fuzzy minimum, which is exactly where shorthand like `мб` lives.

use std::collections::HashMap;
use std::fmt;

use crate::alphabet::normalize;

/// One expansion of a shorthand: the surface form to insert and its lemma
/// (for grouping; equals the form when there is no paradigm).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Expansion {
    pub form: String,
    pub lemma: String,
}

#[derive(Debug, Default)]
pub struct Shortcuts {
    map: HashMap<String, Vec<Expansion>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShortcutError {
    pub line: usize,
    pub message: String,
}

impl fmt::Display for ShortcutError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "shortcuts line {}: {}", self.line, self.message)
    }
}

impl std::error::Error for ShortcutError {}

impl Shortcuts {
    /// Parses `shorthand<TAB>form[<TAB>lemma]`. The lemma defaults to the
    /// form. Empty lines and `#` comments are skipped. Multiple lines with
    /// the same shorthand stack as ordered expansions.
    pub fn from_tsv_str(tsv: &str) -> Result<Self, ShortcutError> {
        let mut map: HashMap<String, Vec<Expansion>> = HashMap::new();
        for (i, raw) in tsv.lines().enumerate() {
            let line = raw.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let mut parts = line.split('\t');
            let (Some(shorthand), Some(form)) = (parts.next(), parts.next()) else {
                return Err(ShortcutError {
                    line: i + 1,
                    message: format!("expected `shorthand\\tform[\\tlemma]`, got `{raw}`"),
                });
            };
            let lemma = parts.next().unwrap_or(form);
            if parts.next().is_some() {
                return Err(ShortcutError {
                    line: i + 1,
                    message: format!("unexpected extra column in `{raw}`"),
                });
            }
            map.entry(normalize(shorthand.trim()))
                .or_default()
                .push(Expansion {
                    form: form.trim().to_string(),
                    lemma: lemma.trim().to_string(),
                });
        }
        Ok(Self { map })
    }

    /// Expansions for an already-normalized shorthand.
    pub fn get(&self, normalized: &str) -> &[Expansion] {
        self.map.get(normalized).map(Vec::as_slice).unwrap_or(&[])
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_normalizes_keys() {
        let s = Shortcuts::from_tsv_str("спс\tспасибо\nМБ\tможет быть\nкрч\tкороче\n").unwrap();
        assert_eq!(s.get("спс")[0].form, "спасибо");
        assert_eq!(s.get("спс")[0].lemma, "спасибо");
        // Key is normalized, so uppercase input matches.
        assert_eq!(s.get("мб")[0].form, "может быть");
        assert!(s.get("нет").is_empty());
    }

    #[test]
    fn lemma_column_is_optional() {
        let s = Shortcuts::from_tsv_str("чел\tчеловек\tчеловек\n").unwrap();
        assert_eq!(s.get("чел")[0].lemma, "человек");
    }

    #[test]
    fn rejects_malformed() {
        assert!(Shortcuts::from_tsv_str("спс").is_err());
        assert!(Shortcuts::from_tsv_str("спс\tспасибо\tспасибо\tлишнее").is_err());
    }
}
