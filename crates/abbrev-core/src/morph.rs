//! Lightweight Russian morphology: just enough grammemes to make case
//! agreement a ranking signal.
//!
//! The lexicon carries an optional OpenCorpora-style grammeme tag per form
//! (e.g. `NOUN,inan,femn,sing,loct`, produced offline by pymorphy3). At
//! ranking time the only thing we read is the **case**, and we check it
//! against the case a preceding preposition governs. This is a *soft*
//! signal: a match boosts, a mismatch is neutral (many prepositions govern
//! several cases, so we never penalize).

/// Russian grammatical case.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Case {
    Nom,
    Gen,
    Dat,
    Acc,
    Ins,
    Loc,
}

/// Extracts the case from an OpenCorpora-style grammeme tag string, if any.
/// Unknown / caseless tags (verbs, adverbs, empty) yield `None`.
pub fn case_of(tags: &str) -> Option<Case> {
    tags.split([',', ' ']).find_map(|g| {
        Some(match g {
            "nomn" => Case::Nom,
            "gent" | "gen1" | "gen2" => Case::Gen,
            "datv" => Case::Dat,
            "accs" | "acc2" => Case::Acc,
            "ablt" => Case::Ins,
            "loct" | "loc1" | "loc2" => Case::Loc,
            _ => return None,
        })
    })
}

/// Cases a preposition can govern. A soft whitelist: multi-case prepositions
/// list every plausible case, so the signal rewards agreement without
/// punishing the ambiguous ones. The argument must be normalized
/// (lowercase, `ё→е`).
pub fn governed_cases(prep: &str) -> &'static [Case] {
    use Case::*;
    match prep {
        "в" | "во" | "на" => &[Loc, Acc],
        "о" | "об" | "обо" | "при" => &[Loc],
        "для" | "без" | "безо" | "до" | "из" | "изо" | "из-за" | "из-под" | "от" | "ото" | "у"
        | "около" | "возле" | "кроме" | "ради" | "вместо" | "вокруг" | "после" | "против"
        | "среди" | "сверх" => &[Gen],
        "к" | "ко" | "благодаря" | "вопреки" | "согласно" | "навстречу" => {
            &[Dat]
        }
        "по" => &[Dat, Loc, Acc],
        "с" | "со" => &[Ins, Gen],
        "над" | "надо" | "под" | "подо" | "перед" | "передо" | "между" | "меж" => {
            &[Ins]
        }
        "за" => &[Ins, Acc],
        "про" | "через" | "черезо" | "сквозь" => &[Acc],
        _ => &[],
    }
}

/// `1.0` if `tags`' case is one the preposition `prev` governs, else `0.0`.
/// Neutral (0.0) whenever there is no preposition, no case, or no match —
/// never negative, so it can only help.
pub fn compatibility(prev: &str, tags: &str) -> f32 {
    let governed = governed_cases(prev);
    if governed.is_empty() {
        return 0.0;
    }
    match case_of(tags) {
        Some(c) if governed.contains(&c) => 1.0,
        _ => 0.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_case_from_tag() {
        assert_eq!(case_of("NOUN,inan,femn,sing,loct"), Some(Case::Loc));
        assert_eq!(case_of("NOUN,inan,femn,sing,gent"), Some(Case::Gen));
        assert_eq!(case_of("VERB,impf,intr,sing,3per"), None);
        assert_eq!(case_of(""), None);
    }

    #[test]
    fn preposition_governs_case() {
        // в + предложный → совместимо; в + родительный → нет.
        assert_eq!(compatibility("в", "NOUN,sing,loct"), 1.0);
        assert_eq!(compatibility("в", "NOUN,sing,gent"), 0.0);
        // для + родительный → совместимо.
        assert_eq!(compatibility("для", "NOUN,sing,gent"), 1.0);
        // не предлог → нейтрально.
        assert_eq!(compatibility("очень", "NOUN,sing,loct"), 0.0);
    }
}
