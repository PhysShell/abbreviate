//! Ordered string → entry multimap with frequency-aware prefix scans.
//!
//! Buckets store `(freq_bits, id)` and are sorted by frequency (descending)
//! once after build, so `exact` is a slice take and `with_prefix` is an
//! exact top-k by frequency over the key range (bounded min-heap) — the
//! caps keep the *most frequent* entries, never the alphabetically first.
//!
//! A `BTreeMap` range scan is enough for the MVP scale (a few hundred
//! thousand forms). If profiling on device shows otherwise, this is the
//! single place to swap in an FST/DAWG without touching the engine.

use std::cmp::Reverse;
use std::collections::{BTreeMap, BinaryHeap};
use std::ops::Bound;

use crate::lexicon::EntryId;

/// Non-negative f32 frequencies compare correctly as raw IEEE-754 bits.
fn freq_bits(freq: f32) -> u32 {
    freq.max(0.0).to_bits()
}

#[derive(Debug, Default)]
pub struct PrefixMap {
    map: BTreeMap<String, Vec<(u32, EntryId)>>,
}

impl PrefixMap {
    pub fn insert(&mut self, key: String, id: EntryId, freq: f32) {
        self.map.entry(key).or_default().push((freq_bits(freq), id));
    }

    /// Sorts every bucket by frequency, descending. Call once after build;
    /// lookups assume it.
    pub fn finalize(&mut self) {
        for bucket in self.map.values_mut() {
            bucket.sort_unstable_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
        }
    }

    /// Up to `cap` ids stored under exactly `key`, most frequent first.
    pub fn exact(&self, key: &str, cap: usize) -> Vec<EntryId> {
        match self.map.get(key) {
            Some(bucket) => bucket.iter().take(cap).map(|&(_, id)| id).collect(),
            None => Vec::new(),
        }
    }

    /// The `cap` most frequent ids under any key starting with `prefix`.
    ///
    /// Exact top-k: a bounded min-heap over the whole range; per-bucket
    /// early exit relies on buckets being frequency-sorted.
    pub fn with_prefix(&self, prefix: &str, cap: usize) -> Vec<EntryId> {
        if prefix.is_empty() || cap == 0 {
            return Vec::new();
        }
        let mut heap: BinaryHeap<Reverse<(u32, EntryId)>> = BinaryHeap::with_capacity(cap + 1);
        let range = self
            .map
            .range::<String, _>((Bound::Included(prefix.to_string()), Bound::Unbounded));
        for (key, bucket) in range {
            if !key.starts_with(prefix) {
                break;
            }
            for &(bits, id) in bucket {
                if heap.len() < cap {
                    heap.push(Reverse((bits, id)));
                } else if heap
                    .peek()
                    .is_some_and(|&Reverse((min_bits, _))| bits > min_bits)
                {
                    heap.pop();
                    heap.push(Reverse((bits, id)));
                } else {
                    // Bucket is frequency-sorted: the rest is no better.
                    break;
                }
            }
        }
        let mut top: Vec<(u32, EntryId)> = heap.into_iter().map(|Reverse(pair)| pair).collect();
        top.sort_unstable_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
        top.into_iter().map(|(_, id)| id).collect()
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

    fn built(entries: &[(&str, EntryId, f32)]) -> PrefixMap {
        let mut m = PrefixMap::default();
        for &(key, id, freq) in entries {
            m.insert(key.to_string(), id, freq);
        }
        m.finalize();
        m
    }

    #[test]
    fn exact_is_frequency_ordered_and_capped() {
        let m = built(&[("првт", 0, 10.0), ("првт", 1, 500.0), ("првт", 2, 50.0)]);
        assert_eq!(m.exact("првт", 10), vec![1, 2, 0]);
        assert_eq!(m.exact("првт", 2), vec![1, 2]);
        assert!(m.exact("нет", 10).is_empty());
    }

    #[test]
    fn prefix_scan_keeps_most_frequent_across_keys() {
        // The frequent id 3 lives under the lexicographically *last* key:
        // an alphabetical cap would drop it, a frequency cap must not.
        let m = built(&[
            ("прав", 0, 5.0),
            ("прав", 1, 7.0),
            ("привет", 2, 3.0),
            ("прост", 3, 900.0),
        ]);
        assert_eq!(m.with_prefix("пр", 2), vec![3, 1]);
        assert_eq!(m.with_prefix("пр", 10), vec![3, 1, 0, 2]);
        assert!(m.with_prefix("я", 10).is_empty());
        assert!(m.with_prefix("", 10).is_empty());
    }
}
