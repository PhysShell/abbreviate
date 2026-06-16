//! Build-time–generated declension paradigms for the hold-popup.
//!
//! The lexicon only carries the surface forms that appear in the frequency
//! source, so [`Engine::forms_of_lemma`](crate::Engine::forms_of_lemma) over
//! it returns an incomplete, frequency-ordered pile. This module loads the
//! *complete* declension generated offline
//! (`data/lexicons/ru-hold-groups.tsv`, via `scripts/paradigms.py`) so the
//! "hold a suggestion to see its forms" UI can show an ordered grid instead
//! of a morphological salad: case × number for nouns, and case × gender (in
//! the singular) for adjectives and adjectival pronouns.
//!
//! Sans-IO and zero-dependency like the rest of the core: the shell passes
//! the artifact in as a string. Parsing is **lenient** — a malformed row is
//! skipped, never fatal — because this is a display-only enhancement, not
//! the retrieval-critical lexicon (which, by contrast, rejects bad rows).

use std::collections::{HashMap, HashSet};

use crate::alphabet::normalize;
use crate::morph::Case;

/// Grammatical number of a paradigm group.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Number {
    Singular,
    Plural,
}

/// Grammatical gender of a paradigm group. Only the *singular* of an
/// adjective-shaped lexeme (full adjective or adjectival pronoun) splits by
/// gender; nouns carry lexical gender (so a single group suffices) and the
/// plural never distinguishes gender — both leave this `None`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gender {
    Masculine,
    Feminine,
    Neuter,
}

/// One filled declension cell: a case and the surface form filling it.
#[derive(Debug, Clone, PartialEq)]
pub struct CaseForm {
    pub case: Case,
    pub form: String,
}

/// A lemma's declension for one number (and, for adjectives, gender), in
/// canonical case order (nominative → genitive → dative → accusative →
/// instrumental → locative). Cases with no form (defective paradigms) are
/// omitted.
#[derive(Debug, Clone, PartialEq)]
pub struct ParadigmGroup {
    pub number: Number,
    /// Gender of an adjectival singular group; `None` for nouns and any
    /// plural (see [`Gender`]).
    pub gender: Option<Gender>,
    pub forms: Vec<CaseForm>,
}

/// Sort rank putting the singular before the plural.
fn number_rank(number: Number) -> u8 {
    match number {
        Number::Singular => 0,
        Number::Plural => 1,
    }
}

/// Sort rank ordering a singular's gendered groups masc → femn → neut, with
/// the genderless group (nouns) first.
fn gender_rank(gender: Option<Gender>) -> u8 {
    match gender {
        None => 0,
        Some(Gender::Masculine) => 1,
        Some(Gender::Feminine) => 2,
        Some(Gender::Neuter) => 3,
    }
}

/// Column order of the artifact's pipe-joined forms field — matches the
/// `CASES` list in `scripts/paradigms.py`.
const CASE_ORDER: [Case; 6] = [
    Case::Nom,
    Case::Gen,
    Case::Dat,
    Case::Acc,
    Case::Ins,
    Case::Loc,
];

/// Lemma → declension groups (singular first). Keyed by normalized lemma so
/// lookups line up with the engine's `by_lemma` keys.
#[derive(Debug, Default)]
pub struct Paradigms {
    by_lemma: HashMap<String, Vec<ParadigmGroup>>,
}

impl Paradigms {
    /// Parses the hold-groups TSV: `lemma<TAB>group<TAB>f1|f2|...|f6`, where
    /// `group` is a [group key](Self::parse_group) (`sing`/`plur`, or
    /// `sing.masc`/`sing.femn`/`sing.neut` for an adjective's gendered
    /// singular) and the six pipe-separated forms are in [`CASE_ORDER`] (an
    /// empty slot = no such form). Comment (`#`) and blank lines are skipped,
    /// as is any malformed row (lenient by design).
    pub fn from_tsv_str(tsv: &str) -> Self {
        let mut by_lemma: HashMap<String, Vec<ParadigmGroup>> = HashMap::new();
        for line in tsv.lines() {
            let line = line.trim_end();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let mut parts = line.split('\t');
            // Exactly three tab-separated columns; the trailing `None` guard
            // rejects rows with extra columns.
            let (Some(lemma), Some(group), Some(forms), None) =
                (parts.next(), parts.next(), parts.next(), parts.next())
            else {
                continue;
            };
            let Some((number, gender)) = Self::parse_group(group) else {
                continue;
            };
            let cells: Vec<&str> = forms.split('|').collect();
            if cells.len() != CASE_ORDER.len() {
                continue;
            }
            let forms: Vec<CaseForm> = CASE_ORDER
                .iter()
                .zip(cells)
                .filter(|(_, cell)| !cell.is_empty())
                .map(|(&case, cell)| CaseForm {
                    case,
                    form: cell.to_string(),
                })
                .collect();
            if forms.is_empty() {
                continue;
            }
            by_lemma
                .entry(normalize(lemma))
                .or_default()
                .push(ParadigmGroup {
                    number,
                    gender,
                    forms,
                });
        }
        // Canonical order, regardless of file order (the sorted artifact lists
        // groups alphabetically, e.g. `plur` before `sing.masc`): singular
        // before plural, and within the singular masc → femn → neut. Nouns
        // (gender `None`) lead their singular, but a lemma never mixes a noun
        // and an adjectival paradigm (the generator picks one part of speech).
        for groups in by_lemma.values_mut() {
            groups.sort_by_key(|g| (number_rank(g.number), gender_rank(g.gender)));
        }
        Self { by_lemma }
    }

    /// Parses a group key into `(number, gender)`. Accepts `sing`/`plur` and
    /// the gendered singulars `sing.masc`/`sing.femn`/`sing.neut`; returns
    /// `None` for anything else (including a gender on the plural), so the
    /// caller skips the row.
    fn parse_group(group: &str) -> Option<(Number, Option<Gender>)> {
        match group {
            "sing" => Some((Number::Singular, None)),
            "plur" => Some((Number::Plural, None)),
            "sing.masc" => Some((Number::Singular, Some(Gender::Masculine))),
            "sing.femn" => Some((Number::Singular, Some(Gender::Feminine))),
            "sing.neut" => Some((Number::Singular, Some(Gender::Neuter))),
            _ => None,
        }
    }

    /// Declension groups for `lemma` (normalized internally), or `None` when
    /// the lemma has no paradigm (e.g. a verb/adverb, or absent from the
    /// artifact).
    pub fn get(&self, lemma: &str) -> Option<&[ParadigmGroup]> {
        self.by_lemma.get(&normalize(lemma)).map(Vec::as_slice)
    }

    /// Flattened forms in canonical order (singular cases, then plural),
    /// de-duplicated keeping first occurrence — the ordered replacement for
    /// the frequency-sorted lexicon pile in the hold UI.
    pub fn forms(&self, lemma: &str) -> Option<Vec<String>> {
        let groups = self.get(lemma)?;
        let mut seen = HashSet::new();
        Some(
            groups
                .iter()
                .flat_map(|g| &g.forms)
                .filter(|cell| seen.insert(&cell.form))
                .map(|cell| cell.form.clone())
                .collect(),
        )
    }

    pub fn is_empty(&self) -> bool {
        self.by_lemma.is_empty()
    }

    pub fn len(&self) -> usize {
        self.by_lemma.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "\
# header comment
работа\tplur\tработы|работ|работам|работы|работами|работах
работа\tsing\tработа|работы|работе|работу|работой|работе
тестирование\tsing\tтестирование|тестирования|тестированию|тестирование|тестированием|тестировании
ножницы\tplur\tножницы|ножниц|ножницам|ножницы|ножницами|ножницах";

    #[test]
    fn parses_and_orders_singular_first() {
        let p = Paradigms::from_tsv_str(SAMPLE);
        let groups = p.get("работа").unwrap();
        assert_eq!(groups.len(), 2);
        // Singular leads even though the file lists plural first.
        assert_eq!(groups[0].number, Number::Singular);
        assert_eq!(groups[1].number, Number::Plural);
        assert_eq!(groups[0].forms.len(), 6);
        assert_eq!(groups[0].forms[0].case, Case::Nom);
        assert_eq!(groups[0].forms[0].form, "работа");
        assert_eq!(groups[0].forms[5].case, Case::Loc);
        assert_eq!(groups[0].forms[5].form, "работе");
    }

    #[test]
    fn defective_paradigms_keep_only_the_number_that_exists() {
        let p = Paradigms::from_tsv_str(SAMPLE);
        // Singularia tantum: only singular.
        let t = p.get("тестирование").unwrap();
        assert_eq!(t.len(), 1);
        assert_eq!(t[0].number, Number::Singular);
        // Pluralia tantum: only plural.
        let n = p.get("ножницы").unwrap();
        assert_eq!(n.len(), 1);
        assert_eq!(n[0].number, Number::Plural);
    }

    #[test]
    fn flattened_forms_are_ordered_and_deduped() {
        let p = Paradigms::from_tsv_str(SAMPLE);
        let forms = p.forms("работа").unwrap();
        // Canonical order, first occurrence wins: работе (dat sing) appears
        // once though it fills both dative and locative singular.
        assert_eq!(
            forms,
            vec![
                "работа",
                "работы",
                "работе",
                "работу",
                "работой", // sing (loct работе deduped)
                "работ",
                "работам",
                "работами",
                "работах", // plur (работы deduped)
            ]
        );
    }

    #[test]
    fn lookup_normalizes_lemma() {
        let p = Paradigms::from_tsv_str(SAMPLE);
        assert!(p.get("Работа").is_some());
        assert!(p.get("РАБОТА").is_some());
    }

    #[test]
    fn malformed_rows_are_skipped_not_fatal() {
        let tsv = "\
bad\trow
работа\tsing\tработа|работы|работе|работу|работой|работе
oops\tsing\ttoo|few
дом\tweird\tдом|дома|дому|дом|домом|доме
работа\tplur\tработы|работ|работам|работы|работами|работах\textra";
        let p = Paradigms::from_tsv_str(tsv);
        // Only the one well-formed row survived.
        assert_eq!(p.len(), 1);
        assert_eq!(p.get("работа").unwrap().len(), 1);
        assert!(p.get("дом").is_none());
    }

    #[test]
    fn unknown_lemma_yields_none() {
        let p = Paradigms::from_tsv_str(SAMPLE);
        assert!(p.get("несуществующее").is_none());
        assert!(p.forms("несуществующее").is_none());
    }

    // Adjective: three gendered singulars plus a genderless plural, listed in
    // the file in the artifact's alphabetical group order (plur, then the
    // sing.* keys) to exercise re-sorting.
    const ADJ_SAMPLE: &str = "\
красный\tplur\tкрасные|красных|красным|красные|красными|красных
красный\tsing.femn\tкрасная|красной|красной|красную|красной|красной
красный\tsing.masc\tкрасный|красного|красному|красный|красным|красном
красный\tsing.neut\tкрасное|красного|красному|красное|красным|красном";

    #[test]
    fn adjective_groups_order_masc_femn_neut_then_plural() {
        let p = Paradigms::from_tsv_str(ADJ_SAMPLE);
        let g = p.get("красный").unwrap();
        assert_eq!(g.len(), 4);
        assert_eq!(
            g.iter().map(|x| (x.number, x.gender)).collect::<Vec<_>>(),
            vec![
                (Number::Singular, Some(Gender::Masculine)),
                (Number::Singular, Some(Gender::Feminine)),
                (Number::Singular, Some(Gender::Neuter)),
                (Number::Plural, None),
            ]
        );
        // Each gendered group keeps the full case order.
        assert_eq!(g[0].forms[0].case, Case::Nom);
        assert_eq!(g[0].forms[0].form, "красный");
        assert_eq!(g[1].forms[0].form, "красная");
        assert_eq!(g[2].forms[0].form, "красное");
    }

    #[test]
    fn flattened_adjective_forms_dedupe_across_genders() {
        let p = Paradigms::from_tsv_str(ADJ_SAMPLE);
        let forms = p.forms("красный").unwrap();
        // First occurrence wins: красного (masc gen) is shared by neut gen and
        // appears once; the genderless plural follows the three singulars.
        assert_eq!(forms.first().unwrap(), "красный");
        assert_eq!(forms.last().unwrap(), "красными");
        assert_eq!(forms.iter().filter(|f| *f == "красного").count(), 1);
    }

    #[test]
    fn gender_on_plural_is_rejected() {
        // `plur.masc` is semantically impossible; the lenient parser drops it.
        let p = Paradigms::from_tsv_str(
            "большой\tplur.masc\tбольшие|больших|большим|большие|большими|больших",
        );
        assert!(p.get("большой").is_none());
    }
}
