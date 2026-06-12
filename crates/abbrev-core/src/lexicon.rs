//! Lexicon: surface forms as first-class objects.
//!
//! The engine deliberately ranks *inflected surface forms*, not lemmas:
//! Russian keyboard suggestions must respect endings. Lemmas are kept on
//! every entry so the UI can group sibling forms ("hold for forms").

use std::fmt;

/// One surface form of the lexicon.
#[derive(Debug, Clone, PartialEq)]
pub struct LexiconEntry {
    /// Surface form as it should be displayed (original spelling, may keep `ё`).
    pub form: String,
    /// Lemma the form belongs to.
    pub lemma: String,
    /// Frequency prior, instructions-per-million or any monotone count.
    pub freq: f32,
}

/// Identifier of an entry inside a [`Lexicon`] (index into the entry list).
pub type EntryId = u32;

/// Immutable, memory-resident lexicon.
#[derive(Debug, Default)]
pub struct Lexicon {
    entries: Vec<LexiconEntry>,
}

/// Error produced while parsing lexicon sources.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LexiconError {
    pub line: usize,
    pub message: String,
}

impl fmt::Display for LexiconError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "lexicon line {}: {}", self.line, self.message)
    }
}

impl std::error::Error for LexiconError {}

impl Lexicon {
    /// Parses the engine TSV format: `form<TAB>lemma<TAB>freq`.
    /// Empty lines and lines starting with `#` are skipped.
    pub fn from_tsv_str(tsv: &str) -> Result<Self, LexiconError> {
        let mut entries = Vec::new();
        for (i, raw) in tsv.lines().enumerate() {
            let line = raw.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let mut parts = line.split('\t');
            let (form, lemma, freq) = match (parts.next(), parts.next(), parts.next()) {
                (Some(f), Some(l), Some(q)) => (f, l, q),
                _ => {
                    return Err(LexiconError {
                        line: i + 1,
                        message: format!("expected `form\\tlemma\\tfreq`, got `{raw}`"),
                    });
                }
            };
            // Fail fast on extra columns: they signal an incompatible
            // artifact version or a broken generator, not optional data.
            if parts.next().is_some() {
                return Err(LexiconError {
                    line: i + 1,
                    message: format!("unexpected extra column in `{raw}`"),
                });
            }
            let freq: f32 = freq.trim().parse().map_err(|_| LexiconError {
                line: i + 1,
                message: format!("bad frequency `{freq}`"),
            })?;
            entries.push(LexiconEntry {
                form: form.trim().to_string(),
                lemma: lemma.trim().to_string(),
                freq,
            });
        }
        Ok(Self { entries })
    }

    /// Small built-in lexicon for demos and tests. Real deployments load a
    /// full lexicon built by `tools/lexicon-builder` from OpenCorpora и др.
    pub fn demo() -> Self {
        Self::from_tsv_str(include_str!("../data/demo.tsv"))
            .expect("embedded demo lexicon must be valid")
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn get(&self, id: EntryId) -> &LexiconEntry {
        &self.entries[id as usize]
    }

    pub fn iter(&self) -> impl Iterator<Item = (EntryId, &LexiconEntry)> {
        self.entries
            .iter()
            .enumerate()
            .map(|(i, e)| (i as EntryId, e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_tsv() {
        let lex = Lexicon::from_tsv_str("# comment\nпривет\tпривет\t220.5\n\n").unwrap();
        assert_eq!(lex.len(), 1);
        assert_eq!(lex.get(0).form, "привет");
    }

    #[test]
    fn rejects_malformed_lines() {
        assert!(Lexicon::from_tsv_str("привет").is_err());
        assert!(Lexicon::from_tsv_str("привет\tпривет\tмного").is_err());
        // Extra columns mean a broken or incompatible artifact.
        assert!(Lexicon::from_tsv_str("привет\tпривет\t1.0\tлишнее").is_err());
    }

    #[test]
    fn demo_lexicon_loads() {
        assert!(Lexicon::demo().len() > 50);
    }
}
