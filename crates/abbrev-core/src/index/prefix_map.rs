//! Ordered string → entry-id multimap with prefix scans.
//!
//! A `BTreeMap` range scan is enough for the MVP scale (a few hundred
//! thousand forms). If profiling on device shows otherwise, this is the
//! single place to swap in an FST/DAWG without touching the engine.

use std::collections::BTreeMap;
use std::ops::Bound;

use crate::lexicon::EntryId;

#[derive(Debug, Default)]
pub struct PrefixMap {
    map: BTreeMap<String, Vec<EntryId>>,
}

impl PrefixMap {
    pub fn insert(&mut self, key: String, id: EntryId) {
        self.map.entry(key).or_default().push(id);
    }

    /// Ids stored under exactly `key`.
    pub fn exact(&self, key: &str) -> &[EntryId] {
        self.map.get(key).map(Vec::as_slice).unwrap_or(&[])
    }

    /// Ids stored under any key starting with `prefix`, capped at `cap`.
    pub fn with_prefix(&self, prefix: &str, cap: usize) -> Vec<EntryId> {
        if prefix.is_empty() {
            return Vec::new();
        }
        let mut out = Vec::new();
        let range = self
            .map
            .range::<String, _>((Bound::Included(prefix.to_string()), Bound::Unbounded));
        for (key, ids) in range {
            if !key.starts_with(prefix) {
                break;
            }
            for &id in ids {
                if out.len() >= cap {
                    return out;
                }
                out.push(id);
            }
        }
        out
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefix_scan_respects_cap_and_boundaries() {
        let mut m = PrefixMap::default();
        m.insert("првт".into(), 0);
        m.insert("првт".into(), 1);
        m.insert("првтл".into(), 2);
        m.insert("прж".into(), 3);
        assert_eq!(m.exact("првт"), &[0, 1]);
        assert_eq!(m.with_prefix("првт", 10), vec![0, 1, 2]);
        assert_eq!(m.with_prefix("првт", 2), vec![0, 1]);
        assert!(m.with_prefix("я", 10).is_empty());
    }
}
