//! Engine indexes. All three indexes are instances of one ordered
//! prefix map, keyed differently:
//!
//! * **skeleton index** — key is the consonant skeleton of the form;
//! * **form index** — key is the normalized form (plain completion);
//! * **suffix index** — key is the reversed normalized form, so suffix
//!   lookups become prefix scans (the reverse-suffix-trie idea from the
//!   MyStem/Segalovich line of work).

mod prefix_map;

pub use prefix_map::PrefixMap;

use crate::alphabet::{normalize, skeleton};
use crate::lexicon::{EntryId, Lexicon};

/// All persistent indexes derived from a lexicon.
#[derive(Debug)]
pub struct Indexes {
    pub by_skeleton: PrefixMap,
    pub by_form: PrefixMap,
    pub by_reversed_form: PrefixMap,
}

impl Indexes {
    pub fn build(lexicon: &Lexicon) -> Self {
        let mut by_skeleton = PrefixMap::default();
        let mut by_form = PrefixMap::default();
        let mut by_reversed_form = PrefixMap::default();
        for (id, entry) in lexicon.iter() {
            let norm = normalize(&entry.form);
            by_skeleton.insert(skeleton(&norm), id);
            by_reversed_form.insert(norm.chars().rev().collect(), id);
            by_form.insert(norm, id);
        }
        Self {
            by_skeleton,
            by_form,
            by_reversed_form,
        }
    }

    /// Entries whose form ends with `suffix` (normalized), capped.
    pub fn with_suffix(&self, suffix: &str, cap: usize) -> Vec<EntryId> {
        let reversed: String = suffix.chars().rev().collect();
        self.by_reversed_form.with_prefix(&reversed, cap)
    }
}
