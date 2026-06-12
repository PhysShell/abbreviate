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
use crate::index::Indexes;
use crate::lexicon::{EntryId, Lexicon};
use crate::rank::{Signals, Weights, common_ending_len, score};

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

pub struct Engine {
    lexicon: Lexicon,
    indexes: Indexes,
    by_lemma: HashMap<String, Vec<EntryId>>,
    history: UserHistory,
    context_model: Box<dyn ContextModel>,
    config: EngineConfig,
}

impl Engine {
    pub fn new(lexicon: Lexicon) -> Self {
        Self::with_config(lexicon, EngineConfig::default())
    }

    pub fn with_config(lexicon: Lexicon, config: EngineConfig) -> Self {
        let indexes = Indexes::build(&lexicon);
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
            config,
        }
    }

    /// Plugs in a contextual reranker (n-gram LM, neural model, ...).
    pub fn set_context_model(&mut self, model: Box<dyn ContextModel>) {
        self.context_model = model;
    }

    pub fn config(&self) -> &EngineConfig {
        &self.config
    }

    pub fn lexicon(&self) -> &Lexicon {
        &self.lexicon
    }

    /// Ranked suggestions for a (possibly abbreviated) input.
    pub fn suggest(&self, input: &str, context: &Context, limit: usize) -> Vec<Suggestion> {
        let norm = normalize(input.trim());
        let input_chars: Vec<char> = norm.chars().collect();
        if input_chars.len() < self.config.min_input_len || limit == 0 {
            return Vec::new();
        }
        let input_skeleton = skeleton(&norm);
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
            let skeleton_match = if form_skeleton == input_skeleton {
                1.0
            } else if form_skeleton.starts_with(&input_skeleton) {
                0.5
            } else {
                0.0
            };
            let signals = Signals {
                skeleton_match,
                suffix_compatibility: common_ending_len(&input_chars, &form_chars, 3) as f32 / 3.0,
                edit_distance: distance,
                log_frequency: (1.0 + entry.freq.max(0.0)).ln(),
                context: self.context_model.score(context, &entry.form, &entry.lemma),
                user_prior: self.history.prior(&input_skeleton, &form_norm),
            };
            scored.push((score(&signals, &self.config.weights), id));
        }

        scored.sort_by(|a, b| b.0.total_cmp(&a.0));
        scored.truncate(limit);
        scored
            .into_iter()
            .map(|(s, id)| {
                let entry = self.lexicon.get(id);
                Suggestion {
                    form: entry.form.clone(),
                    lemma: entry.lemma.clone(),
                    score: s,
                }
            })
            .collect()
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

    /// Candidate generation: union of skeleton, completion and suffix
    /// buckets, deduplicated. Each source is capped so worst-case work
    /// per keystroke stays bounded.
    fn collect_candidates(&self, norm: &str, input_skeleton: &str) -> Vec<EntryId> {
        let cap = self.config.per_source_cap;
        let mut seen: HashSet<EntryId> = HashSet::new();
        let mut out: Vec<EntryId> = Vec::new();
        let mut push_all = |ids: &[EntryId], out: &mut Vec<EntryId>| {
            for &id in ids {
                if seen.insert(id) {
                    out.push(id);
                }
            }
        };

        // 1. Exact and prefix skeleton buckets: `првт` → привет, приват.
        push_all(self.indexes.by_skeleton.exact(input_skeleton), &mut out);
        push_all(
            &self.indexes.by_skeleton.with_prefix(input_skeleton, cap),
            &mut out,
        );
        // Also try the skeleton minus its last consonant: covers inputs
        // whose final consonants diverge from the target's skeleton
        // (`тстрн` vs `тстрвн` for тестирование).
        let chars: Vec<char> = input_skeleton.chars().collect();
        if chars.len() >= 3 {
            let shorter: String = chars[..chars.len() - 1].iter().collect();
            push_all(
                &self.indexes.by_skeleton.with_prefix(&shorter, cap),
                &mut out,
            );
        }

        // 2. Plain completion: the input may simply be a prefix.
        push_all(&self.indexes.by_form.with_prefix(norm, cap), &mut out);

        // 3. Suffix bucket: route by the longest known ending of the input.
        if let Some(ending) = KNOWN_ENDINGS
            .iter()
            .filter(|e| norm.ends_with(*e))
            .max_by_key(|e| e.chars().count())
        {
            push_all(&self.indexes.with_suffix(ending, cap), &mut out);
        }

        out
    }
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
    fn suffix_route_recovers_long_word() {
        let top = top_forms(&engine(), "тстрние", 3);
        assert!(
            top.iter().any(|f| f == "тестирование"),
            "expected тестирование in top-3, got {top:?}"
        );
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
