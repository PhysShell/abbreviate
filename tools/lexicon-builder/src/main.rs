//! Offline lexicon pipeline (runs on a developer machine / CI, never on
//! device): raw sources in → validated, deduplicated, frequency-sorted
//! engine TSV out.
//!
//! Current importer accepts loose TSV/CSV-ish lines `form;lemma;freq`
//! (`\t`, `;` or `,` as separators). The `rnc` subcommand imports the RNC
//! frequency dictionary and `calibrate` folds it into a lexicon (see
//! `rnc.rs`); an OpenCorpora-XML importer is the next planned addition — see
//! docs/ARCHITECTURE.md, "Конвейер данных".
//!
//! ```text
//! lexicon-builder input.tsv -o lexicon.tsv [--min-freq 1.0]
//! lexicon-builder bigrams corpus.txt --lexicon lexicon.tsv -o lm.tsv \
//!     [--top 150000] [--min-count 3]
//! lexicon-builder rnc freqrnc2011.csv -o rnc-freq.tsv
//! lexicon-builder calibrate lexicon.tsv --rnc rnc-freq.tsv -o out.tsv \
//!     [--max-len 4]
//! ```

mod bigrams;
mod rnc;

use std::collections::HashMap;
use std::process::ExitCode;

use abbrev_core::alphabet::normalize;

fn main() -> ExitCode {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("bigrams") => return bigrams::cmd_bigrams(args.split_off(1)),
        Some("rnc") => return rnc::cmd_rnc(args.split_off(1)),
        Some("calibrate") => return rnc::cmd_calibrate(args.split_off(1)),
        _ => {}
    }
    let mut input: Option<String> = None;
    let mut output: Option<String> = None;
    let mut min_freq = 0.0f32;
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "-o" | "--output" => output = it.next().cloned(),
            "--min-freq" => {
                let Some(v) = it.next().and_then(|v| v.parse().ok()) else {
                    return fail("--min-freq needs a number");
                };
                min_freq = v;
            }
            other => input = Some(other.to_string()),
        }
    }
    let (Some(input), Some(output)) = (input, output) else {
        return fail(
            "usage:\n  \
             lexicon-builder <input> -o <output> [--min-freq N]\n  \
             lexicon-builder bigrams <corpus.txt> --lexicon <lexicon.tsv> -o <lm.tsv>\n  \
             lexicon-builder rnc <freqrnc2011.csv> -o <rnc-freq.tsv>\n  \
             lexicon-builder calibrate <lexicon.tsv> --rnc <rnc-freq.tsv> -o <out.tsv> [--max-len 4]",
        );
    };
    let raw = match std::fs::read_to_string(&input) {
        Ok(r) => r,
        Err(e) => return fail(&format!("cannot read {input}: {e}")),
    };

    // form → (lemma, freq); duplicates keep the max frequency.
    let mut entries: HashMap<String, (String, f32)> = HashMap::new();
    let mut skipped = 0usize;
    for line in raw.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let fields: Vec<&str> = line
            .split(['\t', ';', ',', ' '])
            .map(str::trim)
            .filter(|f| !f.is_empty())
            .collect();
        let parsed = match fields.as_slice() {
            [form, lemma, freq] => freq.parse::<f32>().ok().map(|q| (*form, *lemma, q)),
            // Bare frequency lists: the form doubles as its own lemma.
            [form, freq] => freq.parse::<f32>().ok().map(|q| (*form, *form, q)),
            _ => None,
        };
        let Some((form, lemma, freq)) = parsed else {
            skipped += 1;
            continue;
        };
        if freq < min_freq || !is_russian_word(form) {
            skipped += 1;
            continue;
        }
        let key = normalize(form);
        let slot = entries.entry(key).or_insert((lemma.to_string(), freq));
        if freq > slot.1 {
            *slot = (lemma.to_string(), freq);
        }
    }

    let mut rows: Vec<(String, String, f32)> = entries
        .into_iter()
        .map(|(form, (lemma, freq))| (form, normalize(&lemma), freq))
        .collect();
    rows.sort_by(|a, b| b.2.total_cmp(&a.2).then_with(|| a.0.cmp(&b.0)));

    let mut out = String::from("# engine lexicon: form\tlemma\tfreq — built by lexicon-builder\n");
    for (form, lemma, freq) in &rows {
        out.push_str(&format!("{form}\t{lemma}\t{freq}\n"));
    }
    if let Err(e) = std::fs::write(&output, out) {
        return fail(&format!("cannot write {output}: {e}"));
    }
    eprintln!(
        "written {} entries to {output} ({skipped} lines skipped)",
        rows.len()
    );
    ExitCode::SUCCESS
}

fn is_russian_word(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .all(|c| matches!(c, 'а'..='я' | 'А'..='Я' | 'ё' | 'Ё' | '-'))
}

fn fail(message: &str) -> ExitCode {
    eprintln!("error: {message}");
    ExitCode::FAILURE
}
