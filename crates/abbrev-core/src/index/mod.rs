//! Engine indexes. Three are instances of one ordered prefix map, keyed
//! differently, plus an optional SymSpell-style delete index:
//!
//! * **skeleton index** — key is the consonant skeleton of the form;
//! * **form index** — key is the normalized form (plain completion);
//! * **suffix index** — key is the reversed normalized form, so suffix
//!   lookups become prefix scans (the reverse-suffix-trie idea from the
//!   MyStem/Segalovich line of work);
//! * **skeleton delete index** — every skeleton with one char removed,
//!   for typo-tolerant retrieval (SymSpell): a consonant typo breaks the
//!   skeleton, and without this index the right word is never even
//!   *retrieved*, no matter how good the ranking is.
//!
//! All buckets are frequency-sorted at build time, so every capped lookup
//! returns the most frequent entries, never the alphabetically first.

mod prefix_map;

pub use prefix_map::PrefixMap;

use std::collections::HashMap;

use crate::alphabet::{normalize, skeleton};
use crate::lexicon::{EntryId, Lexicon};

/// Skeletons shorter than this don't get delete variants: the buckets
/// would be enormous and the matches meaningless.
const MIN_DELETE_SKELETON_LEN: usize = 3;

/// All persistent indexes derived from a lexicon.
#[derive(Debug)]
pub struct Indexes {
    pub by_skeleton: PrefixMap,
    pub by_form: PrefixMap,
    pub by_reversed_form: PrefixMap,
    /// `skeleton minus one char` → entries, most frequent first. Empty when
    /// typo tolerance is disabled (it costs memory — a mobile concern).
    by_skeleton_deletes: HashMap<String, Vec<EntryId>>,
}

impl Indexes {
    pub fn build(lexicon: &Lexicon, typo_tolerance: bool) -> Self {
        let mut by_skeleton = PrefixMap::default();
        let mut by_form = PrefixMap::default();
        let mut by_reversed_form = PrefixMap::default();
        let mut deletes: HashMap<String, Vec<(u32, EntryId)>> = HashMap::new();
        for (id, entry) in lexicon.iter() {
            let norm = normalize(&entry.form);
            let skel = skeleton(&norm);
            let freq = entry.freq;
            if typo_tolerance {
                for variant in delete_variants(&skel) {
                    deletes
                        .entry(variant)
                        .or_default()
                        .push((freq.max(0.0).to_bits(), id));
                }
            }
            by_skeleton.insert(skel, id, freq);
            by_reversed_form.insert(norm.chars().rev().collect(), id, freq);
            by_form.insert(norm, id, freq);
        }
        by_skeleton.finalize();
        by_form.finalize();
        by_reversed_form.finalize();
        let by_skeleton_deletes = deletes
            .into_iter()
            .map(|(key, mut bucket)| {
                bucket.sort_unstable_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
                (key, bucket.into_iter().map(|(_, id)| id).collect())
            })
            .collect();
        Self {
            by_skeleton,
            by_form,
            by_reversed_form,
            by_skeleton_deletes,
        }
    }

    /// The `cap` most frequent entries whose form ends with `suffix`.
    pub fn with_suffix(&self, suffix: &str, cap: usize) -> Vec<EntryId> {
        let reversed: String = suffix.chars().rev().collect();
        self.by_reversed_form.with_prefix(&reversed, cap)
    }

    /// Entries whose skeleton minus one char equals `key`, most frequent
    /// first.
    pub fn skeleton_delete_bucket(&self, key: &str) -> &[EntryId] {
        self.by_skeleton_deletes
            .get(key)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }
}

/// All strings obtained by removing exactly one char from `skel`.
pub fn delete_variants(skel: &str) -> Vec<String> {
    let chars: Vec<char> = skel.chars().collect();
    if chars.len() < MIN_DELETE_SKELETON_LEN {
        return Vec::new();
    }
    (0..chars.len())
        .map(|skip| {
            chars
                .iter()
                .enumerate()
                .filter(|(i, _)| *i != skip)
                .map(|(_, c)| *c)
                .collect()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delete_variants_remove_each_char_once() {
        assert_eq!(delete_variants("тстр"), vec!["стр", "ттр", "тср", "тст"]);
        assert!(delete_variants("тс").is_empty());
    }

    #[test]
    fn delete_index_finds_substituted_consonant() {
        // сообщение has skeleton сбщн; a typo з→с gives збщн. The two meet
        // at the shared delete variant бщн.
        let lexicon = Lexicon::demo();
        let indexes = Indexes::build(&lexicon, true);
        let bucket = indexes.skeleton_delete_bucket("бщн");
        assert!(
            bucket.iter().any(|&id| lexicon.get(id).form == "сообщение"),
            "бщн bucket must contain сообщение"
        );
    }

    #[test]
    fn delete_index_disabled_is_empty() {
        let indexes = Indexes::build(&Lexicon::demo(), false);
        assert!(indexes.skeleton_delete_bucket("бщн").is_empty());
    }
}
