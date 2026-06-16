//! Property-based / fuzz tests over the engine's public API.
//!
//! Unit tests pin specific cases; these pin *invariants* over thousands of
//! generated inputs (proptest, a test-only dev-dependency — it never enters
//! the shipped cdylib/wasm, so the zero-runtime-dependency rule still holds).
//!
//! Two flavors:
//! * **Fuzz** — feed parsers arbitrary bytes; the property is "never panics".
//! * **Algebraic** — assert laws the design relies on (e.g. `UserHistory`
//!   merge is commutative, the CRDT property that makes cross-device sync a
//!   blob copy + merge).

use abbrev_core::alphabet::{is_sign, is_vowel, normalize, skeleton};
use abbrev_core::history::UserHistory;
use abbrev_core::{Context, Engine, Lexicon, Number, Paradigms};
use proptest::prelude::*;
use std::collections::HashSet;

/// Messy text that exercises the TSV parsers: cyrillic, ascii, the field
/// separators (tab, newline, pipe) and digits, in any arrangement.
fn messy_tsv() -> impl Strategy<Value = String> {
    proptest::string::string_regex("[а-яёА-ЯЁa-zA-Z0-9\t\n|.;_-]{0,80}").unwrap()
}

// ---- Parsers never panic on arbitrary input -------------------------------

proptest! {
    #[test]
    fn lexicon_parse_never_panics(tsv in messy_tsv()) {
        // Result is Ok or Err; the only failure we care about is a panic.
        let _ = Lexicon::from_tsv_str(&tsv);
    }

    #[test]
    fn paradigms_parse_never_panics(tsv in messy_tsv()) {
        let p = Paradigms::from_tsv_str(&tsv);
        // Touch the result so nothing is optimized away.
        prop_assert!(p.len() <= tsv.lines().count());
    }
}

// ---- Alphabet invariants --------------------------------------------------

proptest! {
    #[test]
    fn skeleton_drops_all_vowels_and_signs(s in "\\PC{0,40}") {
        // Holds for *any* unicode input, normalized or not.
        let sk = skeleton(&normalize(&s));
        for c in sk.chars() {
            prop_assert!(!is_vowel(c), "skeleton kept a vowel: {c:?}");
            prop_assert!(!is_sign(c), "skeleton kept a sign: {c:?}");
        }
    }

    #[test]
    fn normalize_is_idempotent_and_drops_yo(s in "[a-zA-Zа-яёА-ЯЁ0-9 .,!-]{0,40}") {
        let n = normalize(&s);
        prop_assert_eq!(normalize(&n), n.clone(), "normalize not idempotent");
        prop_assert!(!n.contains('ё') && !n.contains('Ё'), "ё survived: {n:?}");
    }
}

// ---- UserHistory: CRDT merge laws (the cross-device-sync invariant) -------

const SKELETONS: [&str; 4] = ["првт", "тя", "рбт", "стрн"];
const FORMS: [&str; 5] = ["привет", "приват", "тебя", "работа", "сторона"];

/// Builds a history by replaying ops over a small fixed pair alphabet, so
/// counters actually collide and accumulate.
fn history_from(ops: &[(usize, usize, bool)]) -> UserHistory {
    let mut h = UserHistory::default();
    for &(s, f, confirm) in ops {
        let (sk, fm) = (SKELETONS[s % SKELETONS.len()], FORMS[f % FORMS.len()]);
        if confirm {
            h.confirm(sk, fm);
        } else {
            h.reject(sk, fm);
        }
    }
    h
}

fn ops_strategy() -> impl Strategy<Value = Vec<(usize, usize, bool)>> {
    prop::collection::vec((0usize..4, 0usize..5, any::<bool>()), 0..24)
}

proptest! {
    #[test]
    fn merge_is_commutative(a in ops_strategy(), b in ops_strategy()) {
        let (ha, hb) = (history_from(&a), history_from(&b));

        let mut ab = UserHistory::default();
        ab.merge(&ha);
        ab.merge(&hb);

        let mut ba = UserHistory::default();
        ba.merge(&hb);
        ba.merge(&ha);

        // to_tsv is sorted+stable, so it is a canonical form of the state.
        prop_assert_eq!(ab.to_tsv(), ba.to_tsv());
    }

    #[test]
    fn merge_is_associative(a in ops_strategy(), b in ops_strategy(), c in ops_strategy()) {
        let (ha, hb, hc) = (history_from(&a), history_from(&b), history_from(&c));

        // (a∘b)∘c
        let mut left = UserHistory::default();
        left.merge(&ha);
        left.merge(&hb);
        left.merge(&hc);

        // a∘(b∘c)
        let mut bc = UserHistory::default();
        bc.merge(&hb);
        bc.merge(&hc);
        let mut right = UserHistory::default();
        right.merge(&ha);
        right.merge(&bc);

        prop_assert_eq!(left.to_tsv(), right.to_tsv());
    }

    #[test]
    fn merging_empty_is_identity(a in ops_strategy()) {
        let ha = history_from(&a);
        let mut merged = UserHistory::default();
        merged.merge(&ha);
        merged.merge(&UserHistory::default());
        prop_assert_eq!(merged.to_tsv(), ha.to_tsv());
    }

    #[test]
    fn tsv_roundtrip_is_stable(a in ops_strategy()) {
        let tsv = history_from(&a).to_tsv();
        prop_assert_eq!(UserHistory::from_tsv(&tsv).to_tsv(), tsv);
    }
}

// ---- Paradigms: structural invariants -------------------------------------

proptest! {
    #[test]
    fn flattened_forms_are_first_occurrence_dedup(
        sing in prop::collection::vec("[а-я]{1,6}", 6),
        plur in prop::collection::vec("[а-я]{1,6}", 6),
    ) {
        let lemma = "тест";
        // Deliberately list plural before singular: parsing must reorder.
        let tsv = format!(
            "{lemma}\tplur\t{}\n{lemma}\tsing\t{}\n",
            plur.join("|"),
            sing.join("|"),
        );
        let p = Paradigms::from_tsv_str(&tsv);
        let groups = p.get(lemma).expect("both groups have non-empty cells");

        // Singular always leads, regardless of file order.
        prop_assert_eq!(groups[0].number, Number::Singular);
        prop_assert_eq!(groups[1].number, Number::Plural);

        // forms() == first-occurrence dedup of the groups' forms, in order.
        let mut expected = Vec::new();
        let mut seen = HashSet::new();
        for g in groups {
            for cf in &g.forms {
                if seen.insert(cf.form.clone()) {
                    expected.push(cf.form.clone());
                }
            }
        }
        prop_assert_eq!(p.forms(lemma).unwrap(), expected.clone());

        // And the result truly has no duplicates.
        let mut once = HashSet::new();
        for f in &expected {
            prop_assert!(once.insert(f), "duplicate in forms(): {f}");
        }
    }
}

// ---- Engine: robustness on arbitrary input --------------------------------

proptest! {
    #[test]
    fn suggest_never_panics_and_respects_limit(
        input in "\\PC{0,24}",
        prev in "[а-яё ]{0,24}",
        limit in 0usize..12,
    ) {
        let engine = Engine::new(Lexicon::demo());
        let ctx = Context::new(prev.split_whitespace().map(String::from).collect());

        let flat = engine.suggest(&input, &ctx, limit);
        prop_assert!(flat.len() <= limit);

        let groups = engine.suggest_grouped(&input, &ctx, limit);
        prop_assert!(groups.len() <= limit);
        let mut lemmas = HashSet::new();
        for g in &groups {
            // A group never lists its own best form among the hold-variants.
            prop_assert!(!g.variants.contains(&g.best.form));
            // One group per lemma (normalized): no duplicate lemmas in the strip.
            prop_assert!(lemmas.insert(normalize(&g.lemma)), "duplicate lemma group");
        }
    }
}

// ---- Engine: suggestion invariants (the snapshot's independent oracle) -----
//
// These hold regardless of ranking coefficients, so they cross-check any
// future variant (e.g. ending-aware re-inflection) without trusting the
// golden snapshot: determinism makes the snapshot reproducible, and "every
// surfaced form is real" guards against synthesizing a form that is not an
// actual inflection. When re-inflection lands, extend `known` with the
// lemma's paradigm cells.

proptest! {
    #[test]
    fn suggestions_are_deterministic_real_and_ranked(
        input in "[а-яё-]{0,16}",
        prev in "[а-яё ]{0,16}",
        limit in 0usize..8,
    ) {
        let engine = Engine::new(Lexicon::demo());
        let known: HashSet<String> = engine
            .lexicon()
            .iter()
            .map(|(_, e)| normalize(&e.form))
            .collect();
        let ctx = Context::new(prev.split_whitespace().map(String::from).collect());

        let first = engine.suggest_grouped(&input, &ctx, limit);
        let again = engine.suggest_grouped(&input, &ctx, limit);

        // Determinism: identical results run-to-run — the property that lets a
        // golden snapshot be a trustworthy oracle in the first place.
        prop_assert_eq!(first.len(), again.len(), "non-deterministic group count");

        let mut prev_score = f32::INFINITY;
        for (a, b) in first.iter().zip(&again) {
            prop_assert_eq!(&a.best.form, &b.best.form, "non-deterministic form");
            prop_assert!((a.best.score - b.best.score).abs() < f32::EPSILON);

            // Finite scores, non-increasing across the strip.
            prop_assert!(a.best.score.is_finite(), "non-finite score");
            prop_assert!(a.best.score <= prev_score, "strip not sorted by score");
            prev_score = a.best.score;

            // No hallucinations: best form and every hold-variant are real
            // forms the lexicon actually carries.
            prop_assert!(
                known.contains(&normalize(&a.best.form)),
                "surfaced unknown form: {}",
                a.best.form
            );
            for v in &a.variants {
                prop_assert!(known.contains(&normalize(v)), "unknown variant: {v}");
            }
        }
    }
}
