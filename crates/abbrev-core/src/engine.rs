//! Engine facade: the only type platform shells need to talk to.
//!
//! The engine is sans-IO: it never touches files, network, clocks or
//! threads. Shells feed it a lexicon and context, persist its history
//! blob, and render its suggestions. That keeps the core trivially
//! portable across Android, iOS, WASM and desktop.

use std::collections::{HashMap, HashSet};

use crate::alphabet::{is_plain_russian, normalize, skeleton};
use crate::context::{Context, ContextModel, NoContext};
use crate::edit::{EditCosts, weighted_distance};
use crate::history::UserHistory;
use crate::index::{Indexes, delete_variants};
use crate::lexicon::{EntryId, Lexicon};
use crate::mask::Masker;
use crate::morph;
use crate::paradigm::{ParadigmGroup, Paradigms};
use crate::rank::{Signals, Weights, common_ending_len, common_prefix_len, score};
use crate::recency::SessionCache;
use crate::shortcuts::Shortcuts;
use crate::tone::{Register, ToneMeter};

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
    /// Offer a masked twin (`долбоёб → дол@#&б`) next to any candidate whose
    /// lemma is on the [`Masker`] censor list. Off by default — masking costs
    /// nothing and changes nothing until a host opts in (the *when* is shell
    /// policy; see `mask.rs` and `docs/RESEARCH-RECENCY-CACHE.md` §5.2).
    pub mask: bool,
    /// Gate masking on window tone (§5.1): when set, twins are offered only
    /// while the register meter reads `Polite` — soften profanity when writing
    /// politely, leave it alone in a crude/among-friends window where it is
    /// affectionate. Requires [`mask`](Self::mask). With **no** tone-marker
    /// list loaded there is nothing to read tone from, so the gate is inert and
    /// masking stays ungated (mask always) rather than silently going dark.
    /// Off by default, preserving the pre-tone behaviour.
    pub mask_when_polite: bool,
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
            mask: false,
            mask_when_polite: false,
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
    session: SessionCache,
    context_model: Box<dyn ContextModel>,
    shortcuts: Shortcuts,
    masker: Masker,
    tone: ToneMeter,
    paradigms: Option<Paradigms>,
    config: EngineConfig,
}

/// Score given to an exact conventional-shortcut hit — above any ranked
/// lexicon candidate, so a typed shorthand is always offered first.
const SHORTCUT_SCORE: f32 = 1000.0;

/// One ranked candidate from either source: a lexicon entry (by id) or an
/// out-of-lexicon session word (a fully-built [`Suggestion`]). Merging the
/// two as one sortable list lets a freshly-typed OOV word interleave with
/// lexicon candidates by score, while the lexicon path stays untouched.
enum Ranked {
    Lexicon(f32, EntryId),
    Oov(Suggestion),
}

impl Ranked {
    fn score(&self) -> f32 {
        match self {
            Ranked::Lexicon(score, _) => *score,
            Ranked::Oov(s) => s.score,
        }
    }

    fn into_suggestion(self, lexicon: &Lexicon) -> Suggestion {
        match self {
            Ranked::Lexicon(score, id) => {
                let entry = lexicon.get(id);
                Suggestion {
                    form: entry.form.clone(),
                    lemma: entry.lemma.clone(),
                    score,
                }
            }
            Ranked::Oov(s) => s,
        }
    }
}

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
            session: SessionCache::default(),
            context_model: Box::new(NoContext),
            shortcuts: Shortcuts::default(),
            masker: Masker::default(),
            tone: ToneMeter::new(),
            paradigms: None,
            config,
        }
    }

    /// Plugs in a contextual reranker (n-gram LM, neural model, ...).
    pub fn set_context_model(&mut self, model: Box<dyn ContextModel>) {
        self.context_model = model;
    }

    /// Loads build-time declension paradigms (`ru-hold-groups.tsv`), used to
    /// render an ordered hold-popup grid (case×number for nouns, case×gender
    /// for adjectives) instead of the lexicon's incomplete frequency-sorted
    /// form pile.
    pub fn set_paradigms(&mut self, paradigms: Paradigms) {
        self.paradigms = Some(paradigms);
    }

    /// Loads the conventional-shortcuts layer (exact-match shorthand).
    pub fn set_shortcuts(&mut self, shortcuts: Shortcuts) {
        self.shortcuts = shortcuts;
    }

    /// Loads the profanity-masking censor list. Inert unless
    /// [`EngineConfig::mask`] is also set (the *when* is the host's call).
    pub fn set_masker(&mut self, masker: Masker) {
        self.masker = masker;
    }

    /// Loads the tone/register marker meter (§5.1). Consumes the same
    /// [`note_word`](Self::note_word) stream; read via [`register`](Self::register).
    pub fn set_tone_meter(&mut self, tone: ToneMeter) {
        self.tone = tone;
    }

    /// Flips the masking gate at runtime (§5.2), so a shell can bind it to a
    /// user setting without rebuilding the engine. `mask` is the master switch;
    /// `mask_when_polite` additionally tone-gates it (see
    /// [`EngineConfig::mask_when_polite`]). Inert until a censor list is loaded
    /// via [`set_masker`](Self::set_masker).
    pub fn set_masking(&mut self, mask: bool, mask_when_polite: bool) {
        self.config.mask = mask;
        self.config.mask_when_polite = mask_when_polite;
    }

    /// Coarse register of the recent window, from the tone meter — `Neutral`
    /// until a marker list is loaded and enough recent signal accrues.
    pub fn register(&self) -> Register {
        self.tone.register()
    }

    /// Signed window tone in `[-1, 1]` (`+` polite, `-` crude, `0` no signal).
    pub fn tone(&self) -> f32 {
        self.tone.tone()
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
            return self.masked(out, limit);
        }
        for ranked in self.ranked(input, context, limit) {
            if out.len() >= limit {
                break;
            }
            let suggestion = ranked.into_suggestion(&self.lexicon);
            if seen.insert(normalize(&suggestion.form)) {
                out.push(suggestion);
            }
        }
        self.masked(out, limit)
    }

    /// Whether masking should emit twins right now: enabled, a non-empty
    /// censor list, and — when tone-gated (§5.1) — a `Polite` window. Reading
    /// the meter here makes §5.2 a live client of §5.1.
    fn masking_active(&self) -> bool {
        if !self.config.mask || self.masker.is_empty() {
            return false;
        }
        // The tone gate needs a marker list to read tone from. With an empty
        // meter there is no basis to gate on (`register()` is always
        // `Neutral`), so masking stays ungated rather than silently going dark
        // — matching the field doc.
        !self.config.mask_when_polite
            || self.tone.is_empty()
            || self.tone.register() == Register::Polite
    }

    /// Inserts a masked twin (`долбоёб → дол@#&б`) immediately after every
    /// suggestion whose lemma is censored, then truncates to `limit`. The
    /// original is left in place — masking *offers*, never substitutes — so a
    /// host can render both and the user picks. A no-op (plain truncate) when
    /// masking is off or the censor list is empty, which is the default and
    /// costs nothing. Run as a post-step so it never disturbs ranking or the
    /// surface-form dedup above.
    fn masked(&self, items: Vec<Suggestion>, limit: usize) -> Vec<Suggestion> {
        if !self.masking_active() {
            let mut items = items;
            items.truncate(limit);
            return items;
        }
        let mut out = Vec::with_capacity(items.len() + 1);
        // Twins skip the caller's surface-form dedup, so two distinct forms
        // that mask to the same string (`блять`/`блядь` → `бл@#ь`) would emit
        // it twice. Dedup twins here against everything already shown — both
        // originals and earlier twins — preserving the flat API's invariant.
        let mut seen: HashSet<String> = HashSet::new();
        for s in items {
            let twin = if self.masker.is_masked_lemma(&s.lemma) {
                Masker::mask_form(&s.form)
            } else {
                None
            };
            let score = s.score;
            seen.insert(normalize(&s.form));
            out.push(s);
            // The twin is its own lemma (the masked string) with no paradigm,
            // so nothing profane leaks through the lemma field and a "hold"
            // shows no forms.
            if let Some(form) = twin
                && seen.insert(normalize(&form))
            {
                out.push(Suggestion {
                    lemma: form.clone(),
                    form,
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
        // Last context word, for preposition→case agreement (morph signal).
        let prev_word = context
            .previous_words
            .last()
            .map(|w| normalize(w.trim()))
            .unwrap_or_default();

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
                morph_compatibility: morph::compatibility(&prev_word, &entry.tags),
                recency_prior: self.session.prior_normalized(&form_norm),
            };
            scored.push((score(&signals, &self.config.weights), id));
        }

        scored.sort_by(|a, b| b.0.total_cmp(&a.0));
        scored
    }

    /// Lexicon and out-of-lexicon candidates merged into one score-ordered
    /// list. Lexicon entries lead ties (stable sort over a lexicon-first
    /// vector), since they carry a lemma and corpus frequency.
    fn ranked(&self, input: &str, context: &Context, limit: usize) -> Vec<Ranked> {
        let mut out: Vec<Ranked> = self
            .scored(input, context, limit)
            .into_iter()
            .map(|(score, id)| Ranked::Lexicon(score, id))
            .collect();
        out.extend(
            self.oov_suggestions(input, context)
                .into_iter()
                .map(Ranked::Oov),
        );
        out.sort_by(|a, b| b.score().total_cmp(&a.score()));
        out
    }

    /// Out-of-lexicon retrieval (Part 2): session words the user has typed
    /// that are *not* in the lexicon, scored as candidates so a freshly-used
    /// novel word (`синхрофазотрон`) is reachable from its abbreviation.
    ///
    /// Variant B — a parallel path producing `Suggestion`s directly, bypassing
    /// the `EntryId` pipeline. An OOV word has no corpus frequency, lemma or
    /// grammemes, so its float comes from `recency_prior` (and any user
    /// history): present while the topic is live, sinking below lexicon words
    /// as it decays. Words already in the lexicon are skipped here — the
    /// lexicon path plus the recency signal already cover them.
    fn oov_suggestions(&self, input: &str, context: &Context) -> Vec<Suggestion> {
        if self.session.is_empty() {
            return Vec::new();
        }
        let norm = normalize(input.trim());
        let input_chars: Vec<char> = norm.chars().collect();
        if input_chars.len() < self.config.min_input_len || !is_protected_safe(&norm) {
            return Vec::new();
        }
        let input_skeleton = skeleton(&norm);
        let skeleton_chars: Vec<char> = input_skeleton.chars().collect();
        if skeleton_chars.is_empty() {
            return Vec::new();
        }
        let cutoff = self.config.edit_cutoff_base
            + self.config.edit_cutoff_per_char * input_chars.len() as f32;

        let mut out: Vec<Suggestion> = Vec::new();
        for word in self.session.words() {
            // In-lexicon words are handled by the lexicon path (+recency).
            if !self.indexes.by_form.exact(word.norm, 1).is_empty() {
                continue;
            }
            // Graded stem agreement, same rule as the lexicon path. Gate the
            // expensive edit-distance DP on a shared skeleton prefix: a word
            // with no leading consonant in common is never an abbreviation of
            // this input, and skipping it keeps the per-keystroke scan cheap.
            let form_skeleton_chars: Vec<char> = word.skeleton.chars().collect();
            let skeleton_match = if word.skeleton == input_skeleton {
                1.0
            } else {
                common_prefix_len(&skeleton_chars, &form_skeleton_chars) as f32
                    / skeleton_chars.len() as f32
            };
            if skeleton_match == 0.0 {
                continue;
            }
            let form_chars: Vec<char> = word.norm.chars().collect();
            let Some(distance) =
                weighted_distance(&input_chars, &form_chars, &self.config.costs, cutoff)
            else {
                continue;
            };
            let signals = Signals {
                skeleton_match,
                suffix_compatibility: common_ending_len(&input_chars, &form_chars, 3) as f32 / 3.0,
                prefix_agreement: common_prefix_len(&input_chars, &form_chars) as f32
                    / input_chars.len() as f32,
                edit_distance: distance,
                // No corpus frequency / lemma / grammemes for an OOV word.
                log_frequency: 0.0,
                context: self
                    .context_model
                    .score(context, word.display, word.display),
                user_prior: self.history.prior(&input_skeleton, word.norm),
                morph_compatibility: 0.0,
                recency_prior: word.recency_prior,
            };
            out.push(Suggestion {
                form: word.display.to_string(),
                lemma: word.display.to_string(),
                score: score(&signals, &self.config.weights),
            });
        }
        // Deterministic order: `words()` iterates a HashMap, so ties must be
        // broken on a stable key. Sorting here (score desc, then form) plus
        // the stable merge in `ranked` keeps identical inputs producing
        // identical top-N — the engine's determinism invariant.
        out.sort_by(|a, b| {
            b.score
                .total_cmp(&a.score)
                .then_with(|| a.form.cmp(&b.form))
        });
        out
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
                return self.masked_groups(groups, limit);
            }
        }
        for ranked in self.ranked(input, context, limit) {
            let (best, variants) = match ranked {
                Ranked::Lexicon(score, id) => {
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
                    (best, variants)
                }
                // An OOV word is its own lemma with no sibling forms (no
                // paradigm); it gets a single-form group.
                Ranked::Oov(best) => {
                    if !seen_lemmas.insert(normalize(&best.lemma)) {
                        continue;
                    }
                    (best, Vec::new())
                }
            };
            groups.push(SuggestionGroup {
                lemma: best.lemma.clone(),
                best,
                variants,
            });
            if groups.len() == limit {
                break;
            }
        }
        self.masked_groups(groups, limit)
    }

    /// Grouped-strip counterpart of [`Self::masked`]: inserts a masked twin
    /// group (single form, no variants) after every group whose lemma is
    /// censored, then truncates to `limit`. No-op by default.
    fn masked_groups(&self, groups: Vec<SuggestionGroup>, limit: usize) -> Vec<SuggestionGroup> {
        if !self.masking_active() {
            let mut groups = groups;
            groups.truncate(limit);
            return groups;
        }
        let mut out = Vec::with_capacity(groups.len() + 1);
        // Dedup twin groups by their visible form, as in `masked`: two
        // censored lemmas can mask to the same string.
        let mut seen_twins: HashSet<String> = HashSet::new();
        for g in groups {
            let twin = if self.masker.is_masked_lemma(&g.lemma) {
                Masker::mask_form(&g.best.form)
            } else {
                None
            };
            let score = g.best.score;
            out.push(g);
            if let Some(form) = twin
                && seen_twins.insert(normalize(&form))
            {
                out.push(SuggestionGroup {
                    lemma: form.clone(),
                    best: Suggestion {
                        lemma: form.clone(),
                        form,
                        score,
                    },
                    variants: Vec::new(),
                });
            }
        }
        out.truncate(limit);
        out
    }

    /// All forms of a lemma for the "hold a suggestion to see its forms" UI.
    ///
    /// Prefers the complete, case-ordered generated paradigm
    /// ([`set_paradigms`](Self::set_paradigms)) when one exists for the lemma;
    /// otherwise falls back to the lexicon's (incomplete) forms, most
    /// frequent first.
    pub fn forms_of_lemma(&self, lemma: &str) -> Vec<String> {
        if let Some(forms) = self.paradigms.as_ref().and_then(|p| p.forms(lemma)) {
            return forms;
        }
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

    /// Structured declension of a lemma — number (and, for adjectives,
    /// gender) groups, each a case-ordered list — for a grouped hold-popup.
    /// `None` when no generated paradigm is loaded for the lemma (e.g. a
    /// verb/adverb, or absent from the artifact); callers should then fall
    /// back to [`forms_of_lemma`](Self::forms_of_lemma).
    pub fn paradigm_of_lemma(&self, lemma: &str) -> Option<&[ParadigmGroup]> {
        self.paradigms.as_ref().and_then(|p| p.get(lemma))
    }

    /// Records a **confirmed** suggestion (picked and kept); future
    /// rankings adapt toward this pair.
    pub fn accept(&mut self, input: &str, form: &str) {
        let input_skeleton = skeleton(&normalize(input.trim()));
        self.history.confirm(&input_skeleton, &normalize(form));
    }

    /// Records a **reverted** suggestion (undone/edited after insertion);
    /// future rankings adapt away from this pair (the prior can go negative).
    pub fn reject(&mut self, input: &str, form: &str) {
        let input_skeleton = skeleton(&normalize(input.trim()));
        self.history.reject(&input_skeleton, &normalize(form));
    }

    /// Notes a word observed in the current context (a committed word, not
    /// the in-progress input). Feeds the ephemeral session recency cache so
    /// a freshly-used word floats up in subsequent rankings and decays as the
    /// conversation moves on. The shell drives this stream (one call per
    /// committed word); the cache is local and never persisted or synced.
    pub fn note_word(&mut self, word: &str) {
        self.session.note(word);
        self.tone.note(word);
    }

    /// Clears the session recency cache — the shell calls this when the
    /// context changes (e.g. a different app/field), so recency never leaks
    /// across contexts.
    pub fn reset_session(&mut self) {
        self.session.reset();
        self.tone.reset();
    }

    /// History blob for the shell to persist (privacy stays shell-side).
    pub fn export_history(&self) -> String {
        self.history.to_tsv()
    }

    pub fn import_history(&mut self, tsv: &str) {
        self.history = UserHistory::from_tsv(tsv);
    }

    /// Merges another device's history blob into this one (sum of counters)
    /// — the engine-side hook for cross-device sync.
    pub fn merge_history(&mut self, tsv: &str) {
        self.history.merge(&UserHistory::from_tsv(tsv));
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
/// user's business. Shared with the session cache (a learned OOV word is
/// held to the same bar as typed input — see `SessionCache::note`).
fn is_protected_safe(norm: &str) -> bool {
    is_plain_russian(norm)
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
    fn masking_is_opt_in_and_offers_a_twin() {
        use crate::mask::Masker;
        // A censored word and an innocent look-alike with a *different* lemma.
        let tsv = "редиска\tредиска\t100\tNOUN,inan,femn,sing,nomn\n\
                   редис\tредис\t100\tNOUN,inan,masc,sing,nomn\n";
        let mask_cfg = EngineConfig {
            mask: true,
            ..EngineConfig::default()
        };
        let mut e = Engine::with_config(Lexicon::from_tsv_str(tsv).unwrap(), mask_cfg);
        e.set_masker(Masker::from_list_str("редиска\n").unwrap());

        // Censored lemma → masked twin offered right after the original; the
        // original is never removed.
        let forms = top_forms(&e, "редиска", 5);
        assert!(forms.contains(&"редиска".to_string()), "{forms:?}");
        let i = forms.iter().position(|f| f == "редиска").unwrap();
        assert_eq!(forms.get(i + 1).map(String::as_str), Some("ред@#&а"));

        // Scunthorpe-safe: `редис` shares a prefix but its lemma is not
        // censored, so it gets no twin of its own — `редис` is present and the
        // string that *would* be its mask (`ре@#с`) never appears. (The
        // censored `редиска` may still surface here via fuzzy retrieval, masked
        // correctly — that is not what this checks.)
        let plain = top_forms(&e, "редис", 5);
        assert!(plain.contains(&"редис".to_string()), "{plain:?}");
        assert_eq!(Masker::mask_form("редис").as_deref(), Some("ре@#с"));
        assert!(!plain.contains(&"ре@#с".to_string()), "{plain:?}");

        // The masked twin is its own lemma — nothing profane leaks through it.
        let groups = e.suggest_grouped("редиска", &Context::default(), 5);
        let twin = groups.iter().find(|g| g.best.form == "ред@#&а").unwrap();
        assert_eq!(twin.lemma, "ред@#&а");
        assert!(twin.variants.is_empty());
    }

    #[test]
    fn duplicate_twins_are_deduped() {
        use crate::mask::Masker;
        // Two distinct censored forms that mask to the *same* string must not
        // both surface it — the flat API dedups by visible form.
        let tsv = "блять\tблядь\t300\tNOUN\n\
                   блядь\tблядь\t200\tNOUN\n";
        let mask_cfg = EngineConfig {
            mask: true,
            ..EngineConfig::default()
        };
        let mut e = Engine::with_config(Lexicon::from_tsv_str(tsv).unwrap(), mask_cfg);
        e.set_masker(Masker::from_list_str("блядь\n").unwrap());
        let forms = top_forms(&e, "блять", 20);
        let masked: Vec<_> = forms.iter().filter(|f| f.contains('@')).collect();
        assert_eq!(masked.len(), 1, "twin should appear once, got {forms:?}");
        // The grouped API dedups too.
        let groups = e.suggest_grouped("блять", &Context::default(), 20);
        let masked_groups = groups.iter().filter(|g| g.best.form.contains('@')).count();
        assert_eq!(masked_groups, 1, "twin group should appear once");
    }

    #[test]
    fn tone_gate_masks_only_in_a_polite_window() {
        use crate::mask::Masker;
        use crate::tone::{Register, ToneMeter};
        let tsv = "долбоёб\tдолбоёб\t100\tNOUN\n";
        let cfg = EngineConfig {
            mask: true,
            mask_when_polite: true,
            ..EngineConfig::default()
        };
        let mut e = Engine::with_config(Lexicon::from_tsv_str(tsv).unwrap(), cfg);
        e.set_masker(Masker::from_list_str("долбоёб\n").unwrap());
        let mut tm = ToneMeter::new();
        tm.markers_from_tsv_str("пожалуйста\t+\nхрень\t-\n")
            .unwrap();
        e.set_tone_meter(tm);

        let masked = |e: &Engine| top_forms(e, "долбоёб", 5).iter().any(|f| f.contains('@'));
        // The grouped API shares the gate — assert it too, so a future
        // divergence between `masked` and `masked_groups` is caught.
        let grouped_masked = |e: &Engine| {
            e.suggest_grouped("долбоёб", &Context::default(), 5)
                .iter()
                .any(|g| g.best.form.contains('@'))
        };

        // Neutral window (no markers noted yet): gated → no twin.
        assert_eq!(e.register(), Register::Neutral);
        assert!(!masked(&e), "neutral window must not mask");
        assert!(!grouped_masked(&e), "neutral grouped window must not mask");

        // Polite window → twin offered.
        for _ in 0..3 {
            e.note_word("пожалуйста");
        }
        assert_eq!(e.register(), Register::Polite);
        assert!(masked(&e), "polite window must mask");
        assert!(grouped_masked(&e), "polite grouped window must mask");

        // Crude window → gate closes again.
        e.reset_session();
        for _ in 0..3 {
            e.note_word("хрень");
        }
        assert_eq!(e.register(), Register::Crude);
        assert!(!masked(&e), "crude window must not mask");
        assert!(!grouped_masked(&e), "crude grouped window must not mask");
    }

    #[test]
    fn tone_gate_without_markers_masks_ungated() {
        use crate::mask::Masker;
        // mask_when_polite set, but no tone meter installed: the gate has
        // nothing to read, so masking must stay ungated (mask always) rather
        // than silently emitting no twins.
        let tsv = "долбоёб\tдолбоёб\t100\tNOUN\n";
        let cfg = EngineConfig {
            mask: true,
            mask_when_polite: true,
            ..EngineConfig::default()
        };
        let mut e = Engine::with_config(Lexicon::from_tsv_str(tsv).unwrap(), cfg);
        e.set_masker(Masker::from_list_str("долбоёб\n").unwrap());
        assert_eq!(e.register(), Register::Neutral);
        assert!(
            top_forms(&e, "долбоёб", 5).iter().any(|f| f.contains('@')),
            "empty tone meter must not disable masking"
        );
    }

    #[test]
    fn ungated_masking_ignores_tone() {
        use crate::mask::Masker;
        // mask on, mask_when_polite off (PR #29 behaviour): always masks,
        // regardless of window tone.
        let tsv = "долбоёб\tдолбоёб\t100\tNOUN\n";
        let cfg = EngineConfig {
            mask: true,
            ..EngineConfig::default()
        };
        let mut e = Engine::with_config(Lexicon::from_tsv_str(tsv).unwrap(), cfg);
        e.set_masker(Masker::from_list_str("долбоёб\n").unwrap());
        assert!(
            top_forms(&e, "долбоёб", 5).iter().any(|f| f.contains('@')),
            "ungated masking should mask in any window"
        );
    }

    #[test]
    fn set_masking_toggles_at_runtime() {
        use crate::mask::Masker;
        // Engine built with masking off (default); a shell flips it on later.
        let tsv = "долбоёб\tдолбоёб\t100\tNOUN\n";
        let mut e = Engine::new(Lexicon::from_tsv_str(tsv).unwrap());
        e.set_masker(Masker::from_list_str("долбоёб\n").unwrap());
        let masked = |e: &Engine| top_forms(e, "долбоёб", 5).iter().any(|f| f.contains('@'));
        assert!(!masked(&e), "off by default");
        e.set_masking(true, false);
        assert!(masked(&e), "runtime enable");
        e.set_masking(false, false);
        assert!(!masked(&e), "runtime disable");
    }

    #[test]
    fn masking_off_by_default_costs_nothing() {
        use crate::mask::Masker;
        let tsv = "редиска\tредиска\t100\tNOUN,inan,femn,sing,nomn\n";
        // Default config (mask: false): even with a censor list loaded, no twin.
        let mut e = Engine::new(Lexicon::from_tsv_str(tsv).unwrap());
        e.set_masker(Masker::from_list_str("редиска\n").unwrap());
        let forms = top_forms(&e, "редиска", 5);
        assert!(forms.iter().all(|f| !f.contains('@')), "{forms:?}");
    }

    #[test]
    fn preposition_picks_the_case() {
        // `рбт` is case-ambiguous across работа's forms (all share skeleton
        // рбт). Without context, frequency picks the nominative; a preposition
        // pulls its governed case to the top.
        let tsv = "работа\tработа\t500\tNOUN,inan,femn,sing,nomn\n\
                   работе\tработа\t200\tNOUN,inan,femn,sing,loct\n\
                   работу\tработа\t300\tNOUN,inan,femn,sing,accs\n\
                   работы\tработа\t250\tNOUN,inan,femn,sing,gent\n";
        let e = Engine::new(Lexicon::from_tsv_str(tsv).unwrap());
        let top = |ctx: &[&str]| {
            e.suggest(
                "рбт",
                &Context::new(ctx.iter().map(|s| s.to_string()).collect()),
                1,
            )
            .first()
            .map(|s| s.form.clone())
            .unwrap_or_default()
        };
        assert_eq!(top(&[]), "работа"); // frequency, nominative
        // «в» governs loc/acc → работе or работу, never the nominative.
        assert!(matches!(top(&["в"]).as_str(), "работе" | "работу"));
        // «для» governs genitive → работы.
        assert_eq!(top(&["для"]), "работы");
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
    fn recency_floats_a_session_word_then_decays() {
        // Two forms share skeleton првт; frequency alone prefers привет.
        let tsv = "привет\tпривет\t150\nприват\tприват\t100\n";
        let mut e = Engine::new(Lexicon::from_tsv_str(tsv).unwrap());
        assert_eq!(top_forms(&e, "првт", 1), vec!["привет"]);
        // A word used in this session floats to the top of the ambiguity.
        e.note_word("приват");
        assert_eq!(top_forms(&e, "првт", 1), vec!["приват"]);
        // As the conversation moves on, the boost decays and frequency wins
        // back; resetting the context (e.g. a new app) does it immediately.
        e.reset_session();
        assert_eq!(top_forms(&e, "првт", 1), vec!["привет"]);
    }

    #[test]
    fn recency_normalizes_observed_word() {
        // Shell may hand a capitalized / ё-spelled committed word; it must
        // still match the normalized candidate form.
        let tsv = "привет\tпривет\t150\nприват\tприват\t100\n";
        let mut e = Engine::new(Lexicon::from_tsv_str(tsv).unwrap());
        e.note_word("ПРИВАТ");
        assert_eq!(top_forms(&e, "првт", 1), vec!["приват"]);
    }

    #[test]
    fn oov_word_is_reachable_after_note() {
        // "синхрофазотрон" is not in the demo lexicon, so its abbreviation
        // finds nothing — until the user types it once.
        let mut e = engine();
        assert!(
            !top_forms(&e, "снхрфзтрн", 5)
                .iter()
                .any(|f| f == "синхрофазотрон"),
            "unreachable before note"
        );
        e.note_word("синхрофазотрон");
        assert_eq!(
            top_forms(&e, "снхрфзтрн", 5).first().map(String::as_str),
            Some("синхрофазотрон"),
            "a freshly-typed OOV word is reachable from its skeleton"
        );
        // Clearing the session context drops it again.
        e.reset_session();
        assert!(
            !top_forms(&e, "снхрфзтрн", 5)
                .iter()
                .any(|f| f == "синхрофазотрон"),
            "unreachable after reset"
        );
    }

    #[test]
    fn oov_word_preserves_display_spelling() {
        // The inserted form keeps the user's capitalization, not the
        // normalized matching key.
        let mut e = engine();
        e.note_word("Синхрофазотрон");
        assert_eq!(
            top_forms(&e, "снхрфзтрн", 1),
            vec!["Синхрофазотрон"],
            "display spelling is restored on the suggestion"
        );
    }

    #[test]
    fn in_lexicon_word_is_not_duplicated_by_oov() {
        // Noting an in-lexicon word must not also surface it via the OOV path:
        // it stays a single, lemma-grouped candidate (boosted by recency).
        let mut e = engine();
        e.note_word("привет");
        let count = top_forms(&e, "првт", 5)
            .iter()
            .filter(|f| *f == "привет")
            .count();
        assert_eq!(count, 1, "привет must appear exactly once");
    }

    #[test]
    fn oov_word_forms_its_own_group() {
        let mut e = engine();
        e.note_word("синхрофазотрон");
        let groups = e.suggest_grouped("снхрфзтрн", &Context::default(), 5);
        let group = groups
            .iter()
            .find(|g| g.lemma == "синхрофазотрон")
            .expect("OOV word is a group");
        assert_eq!(group.best.form, "синхрофазотрон");
        assert!(
            group.variants.is_empty(),
            "an OOV word has no sibling forms"
        );
    }

    #[test]
    fn oov_path_respects_protected_input() {
        // A non-word token handed to note_word (digit/punctuation) must never
        // be surfaced as an OOV suggestion — same protected-input rule as the
        // input side, so a clean abbreviation can't leak пароль1.
        let mut e = engine();
        e.note_word("пароль1");
        e.note_word("привет!");
        assert!(
            e.suggest("прль", &Context::default(), 5).is_empty(),
            "digit-bearing token must not be surfaced"
        );
        assert!(
            !top_forms(&e, "првт", 5).iter().any(|f| f == "привет!"),
            "punctuation token must not be surfaced"
        );
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

    #[test]
    fn paradigms_override_forms_of_lemma_with_case_order() {
        use crate::paradigm::{Number, Paradigms};
        let mut e = engine();
        // Without paradigms: structured lookup is empty, forms come from the
        // lexicon (frequency-sorted, possibly incomplete).
        assert!(e.paradigm_of_lemma("работа").is_none());

        e.set_paradigms(Paradigms::from_tsv_str(
            "работа\tsing\tработа|работы|работе|работу|работой|работе\n\
             работа\tplur\tработы|работ|работам|работы|работами|работах\n",
        ));

        // forms_of_lemma now returns the complete, case-ordered paradigm.
        let forms = e.forms_of_lemma("работа");
        assert_eq!(forms[0], "работа");
        assert!(forms.contains(&"работам".to_string())); // plural form not in demo lexicon
        // Structured grid is available and singular-first.
        let groups = e.paradigm_of_lemma("работа").unwrap();
        assert_eq!(groups[0].number, Number::Singular);

        // A lemma with no paradigm still falls back to the lexicon forms.
        assert!(e.paradigm_of_lemma("привет").is_none());
        assert!(!e.forms_of_lemma("привет").is_empty());
    }
}
