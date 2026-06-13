//! Engine facade: the only type platform shells need to talk to.
//!
//! The engine is sans-IO: it never touches files, network, clocks or
//! threads. Shells feed it a lexicon and context, persist its history
//! blob, and render its suggestions. That keeps the core trivially
//! portable across Android, iOS, WASM and desktop.

use std::collections::{HashMap, HashSet};

use crate::alphabet::{normalize, skeleton};
use crate::context::{Context, ContextModel, NoContext};
use crate::edit::{EditCosts, weighted_distance};
use crate::history::UserHistory;
use crate::index::{Indexes, delete_variants};
use crate::lexicon::{EntryId, Lexicon};
use crate::rank::{Signals, Weights, common_ending_len, common_prefix_len, score};
use crate::shortcuts::Shortcuts;

/// Endings used to route an input like `тстрние` into the right
/// reverse-suffix bucket. Ordered longest-first at lookup time.
const KNOWN_ENDINGS: [&str; 12] = [
    "ование",
    "ение",
    "ание",
    "ние",
    "ция",
    "ость",
    "ться",
    "ство",
    "ать",
    "ить",
    "еть",
    "ия",
];

/// Tunables of the suggestion pipeline.
#[derive(Debug, Clone, Copy)]
pub struct EngineConfig {
    /// Inputs shorter than this (in chars) produce no suggestions.
    pub min_input_len: usize,
    /// Cap per candidate source (skeleton bucket, suffix bucket, ...).
    pub per_source_cap: usize,
    /// Edit-distance cutoff: `base + per_char * input_len`.
    pub edit_cutoff_base: f32,
    pub edit_cutoff_per_char: f32,
    /// Build and query the SymSpell-style skeleton delete index. Costs
    /// memory (every skeleton minus one char); disable on tight devices.
    pub typo_tolerance: bool,
    /// Minimum input-skeleton length for typo-tolerant retrieval; short
    /// skeletons make distance-1 matches meaningless.
    pub fuzzy_skeleton_min_len: usize,
    pub weights: Weights,
    pub costs: EditCosts,
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            min_input_len: 3,
            per_source_cap: 2000,
            edit_cutoff_base: 1.0,
            edit_cutoff_per_char: 0.3,
            typo_tolerance: true,
            fuzzy_skeleton_min_len: 3,
            weights: Weights::default(),
            costs: EditCosts::default(),
        }
    }
}

/// One ranked suggestion.
#[derive(Debug, Clone, PartialEq)]
pub struct Suggestion {
    /// Surface form to insert.
    pub form: String,
    /// Lemma, for grouping and the "hold for forms" UI.
    pub lemma: String,
    pub score: f32,
}

/// Two-level candidate model for the suggestion strip:
/// horizontally — different lemmas, vertically (on hold) — forms of one.
/// `best` is chosen by the ranking, i.e. the typed ending picks the form
/// (`тстрние` → тестирование, `тстрния` → тестирования).
#[derive(Debug, Clone, PartialEq)]
pub struct SuggestionGroup {
    pub lemma: String,
    /// Best form of this lemma for the current input; inserted on tap.
    pub best: Suggestion,
    /// Sibling forms for the hold-to-expand list, most frequent first.
    pub variants: Vec<String>,
}

pub struct Engine {
    lexicon: Lexicon,
    indexes: Indexes,
    by_lemma: HashMap<String, Vec<EntryId>>,
    history: UserHistory,
    context_model: Box<dyn ContextModel>,
    shortcuts: Shortcuts,
    config: EngineConfig,
}

/// Score given to an exact conventional-shortcut hit — above any ranked
/// lexicon candidate, so a typed shorthand is always offered first.
const SHORTCUT_SCORE: f32 = 1000.0;

impl Engine {
    pub fn new(lexicon: Lexicon) -> Self {
        Self::with_config(lexicon, EngineConfig::default())
    }

    pub fn with_config(lexicon: Lexicon, config: EngineConfig) -> Self {
        let indexes = Indexes::build(&lexicon, config.typo_tolerance);
        let mut by_lemma: HashMap<String, Vec<EntryId>> = HashMap::new();
        for (id, entry) in lexicon.iter() {
            by_lemma
                .entry(normalize(&entry.lemma))
                .or_default()
                .push(id);
        }
        Self {
            lexicon,
            indexes,
            by_lemma,
            history: UserHistory::default(),
            context_model: Box::new(NoContext),
            shortcuts: Shortcuts::default(),
            config,
        }
    }

    /// Plugs in a contextual reranker (n-gram LM, neural model, ...).
    pub fn set_context_model(&mut self, model: Box<dyn ContextModel>) {
        self.context_model = model;
    }

    /// Loads the conventional-shortcuts layer (exact-match shorthand).
    pub fn set_shortcuts(&mut self, shortcuts: Shortcuts) {
        self.shortcuts = shortcuts;
    }

    /// Replaces the ranking weights without rebuilding indexes — weights do
    /// not affect retrieval, so this is cheap (used by `abbrev tune`).
    pub fn set_weights(&mut self, weights: Weights) {
        self.config.weights = weights;
    }

    pub fn config(&self) -> &EngineConfig {
        &self.config
    }

    pub fn lexicon(&self) -> &Lexicon {
        &self.lexicon
    }

    /// Ranked suggestions for a (possibly abbreviated) input.
    ///
    /// Protected input rule: anything that is not a plain Russian word —
    /// digits, Latin letters, `_`, `@`, URLs, code — is left untouched
    /// (returns no suggestions).
    pub fn suggest(&self, input: &str, context: &Context, limit: usize) -> Vec<Suggestion> {
        if limit == 0 {
            return Vec::new();
        }
        // Conventional shortcuts first (exact match, even below the fuzzy
        // length threshold), then ranked lexicon candidates, deduped.
        let norm = normalize(input.trim());
        let mut seen: HashSet<String> = HashSet::new();
        let mut out: Vec<Suggestion> = Vec::new();
        for exp in self.shortcuts.get(&norm) {
            if seen.insert(normalize(&exp.form)) {
                out.push(Suggestion {
                    form: exp.form.clone(),
                    lemma: exp.lemma.clone(),
                    score: SHORTCUT_SCORE,
                });
            }
        }
        // Exact shortcuts already fill the strip: skip the full fuzzy
        // retrieval/ranking entirely (a top-1 shortcut must not pay it).
        if out.len() >= limit {
            out.truncate(limit);
            return out;
        }
        for (score, id) in self.scored(input, context, limit) {
            if out.len() >= limit {
                break;
            }
            let entry = self.lexicon.get(id);
            if seen.insert(normalize(&entry.form)) {
                out.push(Suggestion {
                    form: entry.form.clone(),
                    lemma: entry.lemma.clone(),
                    score,
                });
            }
        }
        out.truncate(limit);
        out
    }

    /// Full ranked candidate list (score, id), best first. The grouped
    /// view needs the complete list: truncating before grouping lets one
    /// form-rich lemma push other lemmas out of the strip.
    fn scored(&self, input: &str, context: &Context, limit: usize) -> Vec<(f32, EntryId)> {
        let norm = normalize(input.trim());
        let input_chars: Vec<char> = norm.chars().collect();
        if input_chars.len() < self.config.min_input_len || limit == 0 || !is_protected_safe(&norm)
        {
            return Vec::new();
        }
        let input_skeleton = skeleton(&norm);
        let skeleton_chars: Vec<char> = input_skeleton.chars().collect();
        let cutoff = self.config.edit_cutoff_base
            + self.config.edit_cutoff_per_char * input_chars.len() as f32;

        let mut scored: Vec<(f32, EntryId)> = Vec::new();
        for id in self.collect_candidates(&norm, &input_skeleton) {
            let entry = self.lexicon.get(id);
            let form_norm = normalize(&entry.form);
            let form_chars: Vec<char> = form_norm.chars().collect();
            let Some(distance) =
                weighted_distance(&input_chars, &form_chars, &self.config.costs, cutoff)
            else {
                continue;
            };
            let form_skeleton = skeleton(&form_norm);
            // Graded stem agreement: exact skeleton match is 1.0, otherwise
            // the share of the input skeleton matched from the first letter
            // (`тстрн` vs `тстрвн` → 0.8; `тстрн` vs `нстрн` → 0.0). Users
            // keep the leading consonants of the stem, so this separates
            // тестирование from настроение for `тстрние`.
            let skeleton_match = if form_skeleton == input_skeleton {
                1.0
            } else if skeleton_chars.is_empty() {
                0.0
            } else {
                let form_skeleton_chars: Vec<char> = form_skeleton.chars().collect();
                common_prefix_len(&skeleton_chars, &form_skeleton_chars) as f32
                    / skeleton_chars.len() as f32
            };
            let signals = Signals {
                skeleton_match,
                suffix_compatibility: common_ending_len(&input_chars, &form_chars, 3) as f32 / 3.0,
                prefix_agreement: common_prefix_len(&input_chars, &form_chars) as f32
                    / input_chars.len() as f32,
                edit_distance: distance,
                log_frequency: (1.0 + entry.freq.max(0.0)).ln(),
                context: self.context_model.score(context, &entry.form, &entry.lemma),
                user_prior: self.history.prior(&input_skeleton, &form_norm),
            };
            scored.push((score(&signals, &self.config.weights), id));
        }

        scored.sort_by(|a, b| b.0.total_cmp(&a.0));
        scored
    }

    /// Two-level suggestions: one group per lemma, in ranking order.
    /// The strip renders `best` per group; hold expands `variants`.
    pub fn suggest_grouped(
        &self,
        input: &str,
        context: &Context,
        limit: usize,
    ) -> Vec<SuggestionGroup> {
        if limit == 0 {
            return Vec::new();
        }
        // Group over the *complete* ranked list: a form-rich lemma must
        // not push other lemmas out of the strip.
        let mut seen_lemmas: HashSet<String> = HashSet::new();
        let mut groups = Vec::new();
        // Conventional shortcuts lead, each as its own group; hold variants
        // come from the lemma's paradigm when it is in the lexicon.
        let norm = normalize(input.trim());
        for exp in self.shortcuts.get(&norm) {
            if !seen_lemmas.insert(normalize(&exp.lemma)) {
                continue;
            }
            let variants = self
                .forms_of_lemma(&exp.lemma)
                .into_iter()
                .filter(|form| *form != exp.form)
                .collect();
            groups.push(SuggestionGroup {
                lemma: exp.lemma.clone(),
                best: Suggestion {
                    form: exp.form.clone(),
                    lemma: exp.lemma.clone(),
                    score: SHORTCUT_SCORE,
                },
                variants,
            });
            if groups.len() == limit {
                return groups;
            }
        }
        for (score, id) in self.scored(input, context, limit) {
            let entry = self.lexicon.get(id);
            if !seen_lemmas.insert(normalize(&entry.lemma)) {
                continue;
            }
            let best = Suggestion {
                form: entry.form.clone(),
                lemma: entry.lemma.clone(),
                score,
            };
            let variants = self
                .forms_of_lemma(&entry.lemma)
                .into_iter()
                .filter(|form| *form != best.form)
                .collect();
            groups.push(SuggestionGroup {
                lemma: best.lemma.clone(),
                best,
                variants,
            });
            if groups.len() == limit {
                break;
            }
        }
        groups
    }

    /// All forms of a lemma, most frequent first — backs the
    /// "hold a suggestion to see inflected variants" UI.
    pub fn forms_of_lemma(&self, lemma: &str) -> Vec<String> {
        let mut ids = self
            .by_lemma
            .get(&normalize(lemma))
            .cloned()
            .unwrap_or_default();
        ids.sort_by(|&a, &b| {
            self.lexicon
                .get(b)
                .freq
                .total_cmp(&self.lexicon.get(a).freq)
        });
        ids.into_iter()
            .map(|id| self.lexicon.get(id).form.clone())
            .collect()
    }

    /// Records an accepted suggestion; future rankings adapt to the user.
    pub fn accept(&mut self, input: &str, form: &str) {
        let input_skeleton = skeleton(&normalize(input.trim()));
        self.history.accept(&input_skeleton, &normalize(form));
    }

    /// History blob for the shell to persist (privacy stays shell-side).
    pub fn export_history(&self) -> String {
        self.history.to_tsv()
    }

    pub fn import_history(&mut self, tsv: &str) {
        self.history = UserHistory::from_tsv(tsv);
    }

    /// Candidate generation: union of skeleton, completion, suffix and
    /// typo-tolerance buckets, deduplicated. Every source is capped at
    /// `per_source_cap` and all index lookups return entries most
    /// frequent first, so the caps are principled (never alphabetical).
    /// Worst-case work per keystroke is bounded by the caps plus the
    /// `with_prefix` scan budget (see `PrefixMap::with_prefix`).
    fn collect_candidates(&self, norm: &str, input_skeleton: &str) -> Vec<EntryId> {
        let cap = self.config.per_source_cap;
        let mut seen: HashSet<EntryId> = HashSet::new();
        let mut out: Vec<EntryId> = Vec::new();
        let mut push = |ids: &[EntryId], out: &mut Vec<EntryId>| {
            for &id in ids.iter().take(cap) {
                if seen.insert(id) {
                    out.push(id);
                }
            }
        };

        // 1. Exact and prefix skeleton buckets: `првт` → привет, приват.
        push(
            &self.indexes.by_skeleton.exact(input_skeleton, cap),
            &mut out,
        );
        push(
            &self.indexes.by_skeleton.with_prefix(input_skeleton, cap),
            &mut out,
        );
        // Also try the skeleton minus its last consonant: covers inputs
        // whose final consonants diverge from the target's skeleton
        // (`тстрн` vs `тстрвн` for тестирование). Gated at length 4 so the
        // shortened prefix is never shorter than 3 chars — 2-char prefixes
        // cover huge ranges for little recall.
        let chars: Vec<char> = input_skeleton.chars().collect();
        if chars.len() >= 4 {
            let shorter: String = chars[..chars.len() - 1].iter().collect();
            push(
                &self.indexes.by_skeleton.with_prefix(&shorter, cap),
                &mut out,
            );
        }

        // 2. Plain completion: the input may simply be a prefix.
        push(&self.indexes.by_form.with_prefix(norm, cap), &mut out);

        // 3. Suffix bucket: route by the longest known ending of the input.
        if let Some(ending) = KNOWN_ENDINGS
            .iter()
            .filter(|e| norm.ends_with(*e))
            .max_by_key(|e| e.chars().count())
        {
            push(&self.indexes.with_suffix(ending, cap), &mut out);
        }

        // 4. Typo tolerance (SymSpell over skeletons, distance ≤ 1): a
        // consonant typo breaks the skeleton, so the buckets above miss
        // the target entirely. Meet in the middle via delete variants:
        // substitution — both sides delete the differing char; extra char
        // on either side — one side's delete equals the other's original.
        if self.config.typo_tolerance && chars.len() >= self.config.fuzzy_skeleton_min_len {
            // At least 1 for tiny *non-zero* caps: `cap / 4` would
            // silently disable typo tolerance for per_source_cap in 1..4.
            // cap = 0 stays 0 — it means "no candidates from sources" and
            // the final push's take(cap) would drop everything anyway.
            // The push also bounds this source's total contribution by cap.
            let per_bucket = cap.div_ceil(4);
            let mut fuzzy: Vec<EntryId> = Vec::new();
            let take = |ids: &[EntryId], fuzzy: &mut Vec<EntryId>| {
                fuzzy.extend_from_slice(&ids[..ids.len().min(per_bucket)]);
            };
            take(
                self.indexes.skeleton_delete_bucket(input_skeleton),
                &mut fuzzy,
            );
            for variant in delete_variants(input_skeleton) {
                take(
                    &self.indexes.by_skeleton.exact(&variant, per_bucket),
                    &mut fuzzy,
                );
                take(self.indexes.skeleton_delete_bucket(&variant), &mut fuzzy);
            }
            // Keep the overall top-`cap` by frequency across the buckets.
            // Sort ties by id and dedup: overlapping buckets yield the
            // same id several times, and duplicates of one frequent word
            // must not eat the cap slots meant for diverse candidates.
            fuzzy.sort_unstable_by(|&a, &b| {
                self.lexicon
                    .get(b)
                    .freq
                    .total_cmp(&self.lexicon.get(a).freq)
                    .then(a.cmp(&b))
            });
            fuzzy.dedup();
            push(&fuzzy, &mut out);
        }

        out
    }
}

/// "Если не уверен — не трогай": the engine only ever reasons about plain
/// Russian words. Numbers, Latin, identifiers, e-mails and URLs are the
/// user's business.
fn is_protected_safe(norm: &str) -> bool {
    norm.chars().all(|c| matches!(c, 'а'..='я' | 'ё' | '-'))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn engine() -> Engine {
        Engine::new(Lexicon::demo())
    }

    fn top_forms(engine: &Engine, input: &str, n: usize) -> Vec<String> {
        engine
            .suggest(input, &Context::default(), n)
            .into_iter()
            .map(|s| s.form)
            .collect()
    }

    #[test]
    fn short_input_is_silent() {
        assert!(engine().suggest("пр", &Context::default(), 5).is_empty());
    }

    #[test]
    fn skeleton_expansion_prefers_frequent_form() {
        let top = top_forms(&engine(), "првт", 3);
        assert_eq!(top.first().map(String::as_str), Some("привет"));
        assert!(top.iter().any(|f| f == "приват"), "top-3: {top:?}");
    }

    #[test]
    fn typed_ending_picks_the_form() {
        // The ending encoded in the abbreviation must select the surface
        // form: тстрниЕ → тестированиЕ, тстрниЯ → тестированиЯ, etc.
        let e = engine();
        assert_eq!(top_forms(&e, "тстрние", 1), vec!["тестирование"]);
        assert_eq!(top_forms(&e, "тстрния", 1), vec!["тестирования"]);
        assert_eq!(top_forms(&e, "тстрнию", 1), vec!["тестированию"]);
    }

    #[test]
    fn protected_input_is_untouched() {
        let e = engine();
        for input in [
            "api_key",
            "прив3т",
            "test@mail.ru",
            "https://пример.рф",
            "x21",
        ] {
            assert!(
                e.suggest(input, &Context::default(), 5).is_empty(),
                "{input} must produce no suggestions"
            );
        }
        // Hyphenated Russian words are still fair game.
        assert!(is_protected_safe("кто-то"));
    }

    #[test]
    fn conventional_shortcuts_win_and_bypass_min_length() {
        use crate::shortcuts::Shortcuts;
        let mut e = engine();
        e.set_shortcuts(Shortcuts::from_tsv_str("спс\tспасибо\nмб\tможет быть\n").unwrap());
        // Exact shorthand is top-1, even though "мб" is below min_input_len.
        assert_eq!(
            top_forms(&e, "спс", 3).first().map(String::as_str),
            Some("спасибо")
        );
        assert_eq!(top_forms(&e, "мб", 3), vec!["может быть"]);
        // When a shortcut already fills the limit, the fuzzy path is
        // skipped but the result is still exactly the shortcut.
        assert_eq!(top_forms(&e, "спс", 1), vec!["спасибо"]);
        // Non-shortcut input is unaffected.
        assert_eq!(top_forms(&e, "првт", 1), vec!["привет"]);
        // A grouped shortcut leads the strip.
        let groups = e.suggest_grouped("спс", &Context::default(), 3);
        assert_eq!(
            groups.first().map(|g| g.best.form.as_str()),
            Some("спасибо")
        );
        // limit 0 yields nothing on both APIs, even for a shortcut hit.
        assert!(e.suggest("спс", &Context::default(), 0).is_empty());
        assert!(e.suggest_grouped("спс", &Context::default(), 0).is_empty());
    }

    #[test]
    fn consonant_typo_is_still_retrieved() {
        // п→р (adjacent keys) inside the skeleton of компьютер: `кмртер`
        // has skeleton кмртр, while компьютер has кмптр — no exact, prefix
        // or suffix bucket can retrieve it. Only the delete index does
        // (shared variant кмтр). This is retrieval, not ranking.
        let top = top_forms(&engine(), "кмртер", 3);
        assert!(top.iter().any(|f| f == "компьютер"), "got {top:?}");
        // With typo tolerance off the word is unreachable.
        let config = EngineConfig {
            typo_tolerance: false,
            ..EngineConfig::default()
        };
        let strict = Engine::with_config(Lexicon::demo(), config);
        let top = top_forms(&strict, "кмртер", 3);
        assert!(!top.iter().any(|f| f == "компьютер"), "got {top:?}");
    }

    #[test]
    fn context_model_flips_ambiguous_expansion() {
        // The dialogue acceptance case: `ну првт` means привет, while
        // `в првт (канал)` means приват — a bigram model must override
        // the raw frequency prior (привет is 15x more frequent).
        use crate::ngram::BigramModel;
        let lm = "#abbrev-lm v1\nu\tв\t1000\nu\tну\t1000\nu\tпривет\t200\n\
                  u\tприват\t20\nb\tв\tприват\t200\nb\tну\tпривет\t150\n";
        let mut e = engine();
        e.set_context_model(Box::new(BigramModel::from_tsv_str(lm).unwrap()));
        let with_ctx = |ctx_word: &str| {
            e.suggest("првт", &Context::new(vec![ctx_word.to_string()]), 1)
                .first()
                .map(|s| s.form.clone())
        };
        assert_eq!(with_ctx("ну").as_deref(), Some("привет"));
        assert_eq!(with_ctx("в").as_deref(), Some("приват"));
        // No context: frequency wins as before.
        assert_eq!(e.suggest("првт", &Context::default(), 1)[0].form, "привет");
    }

    #[test]
    fn tiny_cap_keeps_typo_tolerance_alive() {
        // per_source_cap < 4 must not zero out the per-bucket take and
        // silently disable fuzzy retrieval (review finding).
        let config = EngineConfig {
            per_source_cap: 2,
            ..EngineConfig::default()
        };
        let engine = Engine::with_config(Lexicon::demo(), config);
        let top: Vec<String> = engine
            .suggest("кмртер", &Context::default(), 3)
            .into_iter()
            .map(|s| s.form)
            .collect();
        assert!(top.iter().any(|f| f == "компьютер"), "got {top:?}");
    }

    #[test]
    fn form_rich_lemma_does_not_crowd_out_other_groups() {
        // One lemma with many high-ranked forms must still leave room for
        // the second lemma in the grouped strip (review finding).
        let mut tsv = String::new();
        for ending in ["а", "е", "у", "ы", "ой", "ам", "ах", "ами", "ою"] {
            tsv.push_str(&format!("тест{ending}\tтест\t100\n"));
        }
        tsv.push_str("тесто\tтесто\t90\n");
        let engine = Engine::new(Lexicon::from_tsv_str(&tsv).unwrap());
        let groups = engine.suggest_grouped("теста", &Context::default(), 2);
        let lemmas: Vec<&str> = groups.iter().map(|g| g.lemma.as_str()).collect();
        assert_eq!(lemmas, vec!["тест", "тесто"], "got {lemmas:?}");
    }

    #[test]
    fn grouped_suggestions_collapse_lemmas() {
        let groups = engine().suggest_grouped("тстрние", &Context::default(), 3);
        let first = groups.first().expect("at least one group");
        assert_eq!(first.lemma, "тестирование");
        assert_eq!(first.best.form, "тестирование");
        assert!(
            first.variants.iter().any(|f| f == "тестирования"),
            "hold list must contain sibling forms, got {:?}",
            first.variants
        );
        // One group per lemma: no lemma appears twice in the strip.
        let mut lemmas: Vec<&str> = groups.iter().map(|g| g.lemma.as_str()).collect();
        lemmas.sort_unstable();
        lemmas.dedup();
        assert_eq!(lemmas.len(), groups.len());
    }

    #[test]
    fn plain_prefix_completion_works() {
        let top = top_forms(&engine(), "прив", 5);
        assert!(top.iter().any(|f| f == "привет"), "got {top:?}");
    }

    #[test]
    fn acceptance_personalizes_ranking() {
        let mut e = engine();
        for _ in 0..3 {
            e.accept("првт", "приват");
        }
        let top = top_forms(&e, "првт", 3);
        assert_eq!(top.first().map(String::as_str), Some("приват"));
    }

    #[test]
    fn history_roundtrip_through_export() {
        let mut e = engine();
        for _ in 0..3 {
            e.accept("првт", "приват");
        }
        let blob = e.export_history();
        let mut fresh = engine();
        fresh.import_history(&blob);
        let top = top_forms(&fresh, "првт", 1);
        assert_eq!(top.first().map(String::as_str), Some("приват"));
    }

    #[test]
    fn forms_of_lemma_sorted_by_frequency() {
        let forms = engine().forms_of_lemma("работа");
        assert!(forms.len() >= 2);
        assert_eq!(forms[0], "работа");
    }
}
