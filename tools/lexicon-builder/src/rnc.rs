//! RNC frequency importer + short-word calibration.
//!
//! Two offline steps that fold the balanced-corpus frequencies of the
//! Russian National Corpus (Lyashevskaya & Sharov, *Частотный словарь
//! современного русского языка*, 2009/2011) into the engine lexicon:
//!
//!   * `rnc <freqrnc2011.csv> -o <rnc-freq.tsv>` — parse the upstream
//!     frequency dictionary (TAB-separated `Lemma PoS Freq(ipm) R D Doc`,
//!     UTF-8) into a clean `lemma<TAB>ipm<TAB>pos` table (Cyrillic lemmas
//!     only, sorted by descending ipm). A faithful, reusable importer.
//!
//!   * `calibrate <lexicon.tsv> --rnc <rnc-freq.tsv> -o <out.tsv>` — raise
//!     the frequency of *short closed-class function words* (prepositions,
//!     conjunctions, particles, interjections, pronominal adverbs) to their
//!     RNC value when the lexicon under-counts them.
//!
//! Why only those words: a subtitle-derived lexicon (OpenSubtitles) skews
//! the register — bookish function words like `ибо`, `при`, `по` are far
//! rarer on screen than in a balanced corpus, so they get out-ranked. The
//! correction is sound precisely for the closed indeclinable classes, where
//! the surface form *is* the lemma, so the lemma-level ipm maps directly to
//! the form's expected count (`ipm × N/1e6`, `N` = the lexicon token total
//! *excluding the words being calibrated* — so the scale never feeds back on
//! our own edits and the pass is idempotent). Declinable parts of speech are
//! left alone: there the lemma ipm is spread across forms and would
//! over-inflate the nominative.
//!
//! Conservative by construction: it only ever *raises* a frequency (a floor),
//! never lowers one, so colloquial particles the subtitles over-count keep
//! their place; and it is capped to short forms (`--max-len`, default 4),
//! the length below which the calibration is benchmark-neutral. Re-running on
//! an already-calibrated lexicon is a no-op. The raw
//! `freqrnc2011.csv` is **not** vendored (its licence is non-commercial,
//! attribution-only); fetch it from <http://dict.ruslang.ru/freq.php>.

use std::collections::HashMap;
use std::process::ExitCode;

use abbrev_core::alphabet::normalize;

/// RNC parts of speech that are closed-class and indeclinable, so the lemma
/// frequency equals the surface form's — the only words we calibrate.
const FUNCTION_POS: [&str; 5] = ["conj", "pr", "part", "intj", "advpro"];

/// `rnc` subcommand: freqrnc2011.csv → `lemma<TAB>ipm<TAB>pos`.
pub fn cmd_rnc(args: Vec<String>) -> ExitCode {
    let mut input: Option<String> = None;
    let mut output: Option<String> = None;
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "-o" | "--output" => output = it.next().cloned(),
            other => input = Some(other.to_string()),
        }
    }
    let (Some(input), Some(output)) = (input, output) else {
        return fail("usage: lexicon-builder rnc <freqrnc2011.csv> -o <rnc-freq.tsv>");
    };
    let raw = match std::fs::read_to_string(&input) {
        Ok(r) => r,
        Err(e) => return fail(&format!("cannot read {input}: {e}")),
    };

    let mut rows: Vec<(String, f64, String)> = Vec::new();
    let mut skipped = 0usize;
    for (i, line) in raw.lines().enumerate() {
        // Skip the header (`Lemma\tPoS\t…`) and any blank line.
        if i == 0 && line.starts_with("Lemma") {
            continue;
        }
        if line.trim().is_empty() {
            continue;
        }
        let mut f = line.split('\t');
        let (Some(lemma), Some(pos), Some(ipm)) = (f.next(), f.next(), f.next()) else {
            skipped += 1;
            continue;
        };
        let Ok(ipm) = ipm.trim().parse::<f64>() else {
            skipped += 1;
            continue;
        };
        if !is_cyrillic_lemma(lemma) {
            skipped += 1;
            continue;
        }
        rows.push((lemma.to_string(), ipm, pos.to_string()));
    }
    // Most frequent first; lemma then PoS as a stable tie-break.
    rows.sort_by(|a, b| {
        b.1.total_cmp(&a.1)
            .then_with(|| a.0.cmp(&b.0))
            .then_with(|| a.2.cmp(&b.2))
    });

    let mut out = String::from(
        "# RNC lemma frequencies (instances per million): lemma\tipm\tpos\n\
         # Source: Ляшевская О.Н., Шаров С.А., Частотный словарь современного\n\
         # русского языка (НКРЯ), 2009/2011 — http://dict.ruslang.ru/freq.php\n",
    );
    for (lemma, ipm, pos) in &rows {
        out.push_str(&format!("{lemma}\t{ipm}\t{pos}\n"));
    }
    if let Err(e) = std::fs::write(&output, out) {
        return fail(&format!("cannot write {output}: {e}"));
    }
    eprintln!(
        "rnc table written to {output}: {} rows ({skipped} skipped)",
        rows.len()
    );
    ExitCode::SUCCESS
}

/// `calibrate` subcommand: floor short function-word frequencies in a lexicon
/// to their RNC value.
pub fn cmd_calibrate(args: Vec<String>) -> ExitCode {
    let mut lexicon: Option<String> = None;
    let mut rnc_path: Option<String> = None;
    let mut output: Option<String> = None;
    let mut max_len = 4usize;
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--rnc" => rnc_path = it.next().cloned(),
            "-o" | "--output" => output = it.next().cloned(),
            "--max-len" => match it.next().and_then(|v| v.parse().ok()) {
                Some(v) => max_len = v,
                None => return fail("--max-len needs a number"),
            },
            other => lexicon = Some(other.to_string()),
        }
    }
    let (Some(lexicon), Some(rnc_path), Some(output)) = (lexicon, rnc_path, output) else {
        return fail(
            "usage: lexicon-builder calibrate <lexicon.tsv> --rnc <rnc-freq.tsv> \
             -o <out.tsv> [--max-len 4]",
        );
    };
    let lex_raw = match std::fs::read_to_string(&lexicon) {
        Ok(r) => r,
        Err(e) => return fail(&format!("cannot read {lexicon}: {e}")),
    };
    let rnc_raw = match std::fs::read_to_string(&rnc_path) {
        Ok(r) => r,
        Err(e) => return fail(&format!("cannot read {rnc_path}: {e}")),
    };

    // Function-word floor table: normalized short lemma → summed ipm.
    let mut floor: HashMap<String, f64> = HashMap::new();
    for line in rnc_raw.lines() {
        if line.starts_with('#') || line.trim().is_empty() {
            continue;
        }
        let mut f = line.split('\t');
        let (Some(lemma), Some(ipm), Some(pos)) = (f.next(), f.next(), f.next()) else {
            continue;
        };
        if !FUNCTION_POS.contains(&pos.trim()) {
            continue;
        }
        let Ok(ipm) = ipm.trim().parse::<f64>() else {
            continue;
        };
        let norm = normalize(lemma);
        if norm.chars().count() > max_len {
            continue;
        }
        *floor.entry(norm).or_insert(0.0) += ipm;
    }

    // Token total of the (non-calibrated part of the) lexicon defines the
    // ipm→count scale. Words we floor are excluded from the denominator: their
    // counts are exactly what we rewrite, so leaving them out keeps the scale
    // independent of any prior calibration and makes the pass idempotent
    // (re-running on the output changes nothing).
    let mut total = 0f64;
    for line in lex_raw.lines() {
        if line.starts_with('#') || line.trim().is_empty() {
            continue;
        }
        let mut cols = line.split('\t');
        let Some(form) = cols.next() else { continue };
        // `nth(1)` skips the lemma column and yields the frequency.
        let Some(freq) = cols.nth(1).and_then(|v| v.trim().parse::<f64>().ok()) else {
            continue;
        };
        if !floor.contains_key(&normalize(form)) {
            total += freq;
        }
    }
    if total <= 0.0 {
        return fail("lexicon has no usable frequency column");
    }
    let scale = total / 1e6;

    // Rewrite in place: only the floored rows change, so the diff stays small.
    let mut out = String::new();
    let mut raised = 0usize;
    for line in lex_raw.lines() {
        if line.starts_with('#') || line.trim().is_empty() {
            out.push_str(line);
            out.push('\n');
            continue;
        }
        let cols: Vec<&str> = line.split('\t').collect();
        if cols.len() < 3 {
            out.push_str(line);
            out.push('\n');
            continue;
        }
        let form = cols[0];
        // A row whose frequency does not parse is data we don't understand —
        // pass it through untouched rather than floor it from a bogus 0.0.
        let Ok(cur) = cols[2].trim().parse::<f64>() else {
            out.push_str(line);
            out.push('\n');
            continue;
        };
        // Compare the rounded target against the current count so re-running on
        // an already-calibrated lexicon is a true no-op (no rounding-noise
        // "raises").
        let new_freq = floor
            .get(&normalize(form))
            .map(|ipm| (ipm * scale).round() as i64);
        match new_freq {
            Some(new_freq) if (new_freq as f64) > cur => {
                let mut rebuilt = format!("{form}\t{}\t{new_freq}", cols[1]);
                // Preserve the optional 4th grammeme column verbatim.
                for extra in &cols[3..] {
                    rebuilt.push('\t');
                    rebuilt.push_str(extra);
                }
                out.push_str(&rebuilt);
                out.push('\n');
                raised += 1;
            }
            _ => {
                out.push_str(line);
                out.push('\n');
            }
        }
    }
    if let Err(e) = std::fs::write(&output, out) {
        return fail(&format!("cannot write {output}: {e}"));
    }
    eprintln!(
        "calibrated {output}: raised {raised} short function words \
         (max-len {max_len}, scale {scale:.2})"
    );
    ExitCode::SUCCESS
}

/// A Cyrillic lemma (lowercase letters, `ё`, optional hyphen). RNC also lists
/// punctuation and Latin tokens, which the engine lexicon never carries.
fn is_cyrillic_lemma(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .all(|c| matches!(c, 'а'..='я' | 'А'..='Я' | 'ё' | 'Ё' | '-'))
}

fn fail(message: &str) -> ExitCode {
    eprintln!("error: {message}");
    ExitCode::FAILURE
}

#[cfg(test)]
mod tests {
    use super::*;

    // Tiny freqrnc-format fixture: a function word (preposition), a long
    // function word, a noun homograph, and a Latin row to be dropped.
    const RNC_FIXTURE: &str = "\
Lemma\tPoS\tFreq(ipm)\tR\tD\tDoc
при\tpr\t1550.8\t100\t97\t1000
поскольку\tconj\t300.0\t90\t90\t500
стол\ts\t200.0\t80\t80\t400
the\ts\t9.9\t10\t10\t10";

    #[test]
    fn function_pos_set_is_indeclinable_closed_class() {
        assert!(FUNCTION_POS.contains(&"pr"));
        assert!(FUNCTION_POS.contains(&"conj"));
        // Declinable classes must be excluded (lemma ipm ≠ form count).
        assert!(!FUNCTION_POS.contains(&"s"));
        assert!(!FUNCTION_POS.contains(&"v"));
        assert!(!FUNCTION_POS.contains(&"a"));
        assert!(!FUNCTION_POS.contains(&"spro"));
    }

    #[test]
    fn cyrillic_lemma_filter_drops_latin_and_empty() {
        assert!(is_cyrillic_lemma("при"));
        assert!(is_cyrillic_lemma("из-за"));
        assert!(!is_cyrillic_lemma("the"));
        assert!(!is_cyrillic_lemma(""));
    }

    /// End-to-end through temp files: import the fixture, then calibrate a
    /// lexicon that under-counts `при` and over-counts a colloquial particle.
    #[test]
    fn calibrate_raises_only_short_function_words_and_only_upward() {
        // Per-run subdirectory keeps parallel test runs from colliding.
        let dir = std::env::temp_dir().join(format!(
            "rnc_fix_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let csv = dir.join("rnc_fix.csv");
        let table = dir.join("rnc_fix_table.tsv");
        let lex = dir.join("rnc_fix_lex.tsv");
        let out = dir.join("rnc_fix_out.tsv");
        std::fs::write(&csv, RNC_FIXTURE).unwrap();

        // `ExitCode` has no `PartialEq`, so we verify success through the
        // artifact: the fresh per-run dir means a failed import leaves no
        // stale table to mask the failure downstream.
        cmd_rnc(vec![
            csv.to_string_lossy().into(),
            "-o".into(),
            table.to_string_lossy().into(),
        ]);
        assert!(
            std::fs::read_to_string(&table).unwrap().contains("при\t"),
            "rnc import did not produce the expected table"
        );

        // Non-candidate token total ≈ 1_000_000 → scale ≈ 1.0, so ipm maps to
        // count ~1:1. `при` is under-counted (100 < 1550) and excluded from the
        // denominator; `стол` is a noun (skipped even though short);
        // `поскольку` is long (> max-len 4, skipped).
        std::fs::write(
            &lex,
            "# header\n\
             при\tпри\t100\n\
             стол\tстол\t100\tNOUN\n\
             поскольку\tпоскольку\t100\n\
             эх\tэх\t999999\n",
        )
        .unwrap();
        cmd_calibrate(vec![
            lex.to_string_lossy().into(),
            "--rnc".into(),
            table.to_string_lossy().into(),
            "-o".into(),
            out.to_string_lossy().into(),
        ]);
        let result = std::fs::read_to_string(&out).unwrap();

        // `при` raised to its RNC count (1551 ≈ round(1550.8)).
        assert!(result.contains("при\tпри\t1551"), "{result}");
        // `стол` untouched: declinable noun, not a function word.
        assert!(result.contains("стол\tстол\t100\tNOUN"), "{result}");
        // `поскольку` untouched: longer than max-len 4.
        assert!(result.contains("поскольку\tпоскольку\t100"), "{result}");
        // Header and ordering preserved (in-place rewrite).
        assert!(result.starts_with("# header\n"), "{result}");
    }
}
