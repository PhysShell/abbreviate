//! Bigram LM builder: a raw text corpus in → the `#abbrev-lm v1` artifact
//! out (consumed by `abbrev_core::BigramModel`).
//!
//! Counts unigrams and adjacent-pair bigrams over corpus lines, restricted
//! to words present in the engine lexicon (ids instead of strings keep the
//! count tables compact). Lossy pruning bounds memory on huge corpora:
//! when the bigram table outgrows a threshold, singleton pairs are
//! dropped — fine for top-K extraction, which only wants frequent pairs.

use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::process::ExitCode;

use abbrev_core::Lexicon;
use abbrev_core::alphabet::normalize;

/// Prune singleton bigrams when the table grows past this many entries.
const PRUNE_THRESHOLD: usize = 12_000_000;

pub fn cmd_bigrams(args: Vec<String>) -> ExitCode {
    let mut corpus: Option<String> = None;
    let mut lexicon_path: Option<String> = None;
    let mut output: Option<String> = None;
    let mut top = 150_000usize;
    let mut min_count = 3u64;
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--lexicon" => lexicon_path = it.next().cloned(),
            "-o" | "--output" => output = it.next().cloned(),
            "--top" => match it.next().and_then(|v| v.parse().ok()) {
                Some(v) => top = v,
                None => return fail("--top needs a number"),
            },
            "--min-count" => match it.next().and_then(|v| v.parse().ok()) {
                Some(v) => min_count = v,
                None => return fail("--min-count needs a number"),
            },
            other => corpus = Some(other.to_string()),
        }
    }
    let (Some(corpus), Some(lexicon_path), Some(output)) = (corpus, lexicon_path, output) else {
        return fail(
            "usage: lexicon-builder bigrams <corpus.txt> --lexicon <lexicon.tsv> -o <lm.tsv>",
        );
    };

    let lexicon_tsv = match std::fs::read_to_string(&lexicon_path) {
        Ok(t) => t,
        Err(e) => return fail(&format!("cannot read {lexicon_path}: {e}")),
    };
    let lexicon = match Lexicon::from_tsv_str(&lexicon_tsv) {
        Ok(l) => l,
        Err(e) => return fail(&e.to_string()),
    };
    // Vocabulary: normalized form → dense id.
    let mut vocab: HashMap<String, u32> = HashMap::new();
    let mut words: Vec<String> = Vec::new();
    for (_, entry) in lexicon.iter() {
        let norm = normalize(&entry.form);
        vocab.entry(norm.clone()).or_insert_with(|| {
            words.push(norm);
            (words.len() - 1) as u32
        });
    }

    let file = match std::fs::File::open(&corpus) {
        Ok(f) => f,
        Err(e) => return fail(&format!("cannot read {corpus}: {e}")),
    };
    let mut reader = BufReader::new(file);
    let mut unigrams: Vec<u64> = vec![0; words.len()];
    let mut bigrams: HashMap<u64, u64> = HashMap::new();
    let mut pruned = 0u64;
    let mut lines = 0u64;
    let mut token_ids: Vec<u32> = Vec::new();
    let mut buf: Vec<u8> = Vec::new();
    loop {
        buf.clear();
        // Read raw bytes and decode lossily: real-world corpora contain
        // stray non-UTF-8 bytes, and they must not silently truncate the
        // count (a corrupted slice once produced a "valid" empty LM).
        // Genuine I/O errors are still fatal.
        match reader.read_until(b'\n', &mut buf) {
            Ok(0) => break,
            Ok(_) => {}
            Err(e) => return fail(&format!("cannot read {corpus}: {e}")),
        }
        let line = String::from_utf8_lossy(&buf);
        lines += 1;
        token_ids.clear();
        for token in tokenize(&line) {
            if let Some(&id) = vocab.get(token.as_str()) {
                token_ids.push(id);
                unigrams[id as usize] += 1;
            } else {
                // OOV breaks adjacency: «привет ZZZ мир» is not a bigram.
                token_ids.push(u32::MAX);
            }
        }
        for pair in token_ids.windows(2) {
            let (a, b) = (pair[0], pair[1]);
            if a != u32::MAX && b != u32::MAX {
                *bigrams
                    .entry(u64::from(a) << 32 | u64::from(b))
                    .or_insert(0) += 1;
            }
        }
        if bigrams.len() > PRUNE_THRESHOLD {
            let before = bigrams.len();
            bigrams.retain(|_, &mut c| c > 1);
            pruned += (before - bigrams.len()) as u64;
        }
    }

    // Top-K bigrams by count.
    let mut ranked: Vec<(u64, u64)> = bigrams
        .into_iter()
        .filter(|&(_, c)| c >= min_count)
        .collect();
    ranked.sort_unstable_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    ranked.truncate(top);
    if ranked.is_empty() {
        return fail(&format!(
            "no bigrams counted over {lines} lines — wrong corpus or lexicon?"
        ));
    }

    let mut out = String::from(
        "#abbrev-lm v1 — built by `lexicon-builder bigrams` (OpenSubtitles ru slice)\n",
    );
    for (id, &count) in unigrams.iter().enumerate() {
        if count > 0 {
            out.push_str(&format!("u\t{}\t{}\n", words[id], count));
        }
    }
    for &(key, count) in &ranked {
        let (a, b) = ((key >> 32) as u32, key as u32);
        out.push_str(&format!(
            "b\t{}\t{}\t{}\n",
            words[a as usize], words[b as usize], count
        ));
    }
    if let Err(e) = std::fs::write(&output, out) {
        return fail(&format!("cannot write {output}: {e}"));
    }
    eprintln!(
        "lm written to {output}: {} lines of corpus, {} unigram types, {} bigrams kept \
         ({pruned} singletons pruned)",
        lines,
        unigrams.iter().filter(|&&c| c > 0).count(),
        ranked.len(),
    );
    ExitCode::SUCCESS
}

/// Cyrillic word tokens, normalized (lowercase, `ё→е`); everything else
/// is a separator.
fn tokenize(line: &str) -> Vec<String> {
    normalize(line)
        .split(|c: char| !matches!(c, 'а'..='я' | '-'))
        .filter(|t| !t.is_empty() && *t != "-")
        .map(str::to_string)
        .collect()
}

fn fail(message: &str) -> ExitCode {
    eprintln!("error: {message}");
    ExitCode::FAILURE
}
