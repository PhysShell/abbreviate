//! `expand` subcommand (variant B): fold the missing inflected forms of
//! already-known lemmas back into the engine lexicon.
//!
//! ~50% of declinable lemmas appear in the OpenSubtitles 50k list with a
//! single surface form (долгосрочный → only `долгосрочной`), so a typed
//! ending can't be honoured by retrieval (`длгсрчная` never reaches
//! `долгосрочная`). The full paradigm is already generated in
//! `ru-hold-groups.tsv`; this injects the cells absent from the lexicon.
//!
//! Frequency: a synthetic form gets `round(max_present × ratio)` of the
//! lemma's most frequent present form, clamped strictly below its least
//! frequent present form. Real forms therefore stay ranked above synthetic
//! ones for ambiguous inputs (`арми`→армии, not армиями), while skeleton and
//! suffix signals still lift the exact ending match (`длгсрчная`→долгосрочная).
//! Unlike runtime re-inflection, the form carries a real frequency, so the
//! ranker — not a coincidental ending — decides.
//!
//! Only forms absent from the lexicon are added, so re-running on an already
//! expanded lexicon is a no-op (idempotent). No new lemmas ever enter.

use std::collections::BTreeMap;
use std::collections::HashSet;
use std::process::ExitCode;

use abbrev_core::alphabet::normalize;

const CASES: [&str; 6] = ["nomn", "gent", "datv", "accs", "ablt", "loct"];

pub fn cmd_expand(args: Vec<String>) -> ExitCode {
    let mut lexicon: Option<String> = None;
    let mut holdgroups: Option<String> = None;
    let mut output: Option<String> = None;
    let mut ratio = 0.25f64;
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "-o" | "--output" => output = it.next().cloned(),
            "--ratio" => match it.next().and_then(|v| v.parse().ok()) {
                Some(v) => ratio = v,
                None => return fail("--ratio needs a number"),
            },
            other if lexicon.is_none() => lexicon = Some(other.to_string()),
            other if holdgroups.is_none() => holdgroups = Some(other.to_string()),
            other => return fail(&format!("unexpected argument {other}")),
        }
    }
    let (Some(lexicon), Some(holdgroups), Some(output)) = (lexicon, holdgroups, output) else {
        return fail(
            "usage: lexicon-builder expand <lexicon.tsv> <hold-groups.tsv> -o <out.tsv> [--ratio 0.25]",
        );
    };
    let lex_raw = match std::fs::read_to_string(&lexicon) {
        Ok(r) => r,
        Err(e) => return fail(&format!("cannot read {lexicon}: {e}")),
    };
    let hold_raw = match std::fs::read_to_string(&holdgroups) {
        Ok(r) => r,
        Err(e) => return fail(&format!("cannot read {holdgroups}: {e}")),
    };

    // Original lexicon, preserved verbatim, plus the lookup tables.
    let mut header: Vec<&str> = Vec::new();
    let mut body: Vec<&str> = Vec::new();
    let mut present: HashSet<String> = HashSet::new();
    let mut lemma_freqs: BTreeMap<String, Vec<f64>> = BTreeMap::new();
    for line in lex_raw.lines() {
        if line.starts_with('#') {
            header.push(line);
            continue;
        }
        if line.trim().is_empty() {
            continue;
        }
        body.push(line);
        let mut f = line.split('\t');
        let (Some(form), Some(lemma), Some(freq)) = (f.next(), f.next(), f.next()) else {
            continue;
        };
        present.insert(normalize(form));
        if let Ok(fr) = freq.trim().parse::<f64>() {
            lemma_freqs.entry(normalize(lemma)).or_default().push(fr);
        }
    }

    // Paradigm: lemma → ordered (group, cells). Sorted lemma iteration keeps
    // the synthetic block deterministic.
    let mut para: BTreeMap<String, Vec<(String, Vec<String>)>> = BTreeMap::new();
    for line in hold_raw.lines() {
        if line.starts_with('#') || line.trim().is_empty() {
            continue;
        }
        let mut f = line.split('\t');
        let (Some(lemma), Some(group), Some(cells)) = (f.next(), f.next(), f.next()) else {
            continue;
        };
        para.entry(normalize(lemma))
            .or_default()
            .push((group.to_string(), cells.split('|').map(normalize).collect()));
    }

    let mut added: Vec<String> = Vec::new();
    for (lemma, groups) in &para {
        let Some(freqs) = lemma_freqs.get(lemma) else {
            continue; // only enrich lemmas the lexicon already attests
        };
        let base = freqs.iter().cloned().fold(0.0_f64, f64::max);
        let lo = freqs.iter().cloned().fold(f64::INFINITY, f64::min);
        // Below the least frequent real form so real forms keep their edge.
        let fill = ((base * ratio).round() as i64)
            .min((lo as i64 - 1).max(1))
            .max(1);
        let is_adj = groups.iter().any(|(g, _)| g.starts_with("sing."));
        let mut seen: HashSet<String> = HashSet::new();
        for (group, cells) in groups {
            for (case, form) in CASES.iter().zip(cells) {
                if form.is_empty() || present.contains(form) || !seen.insert(form.clone()) {
                    continue;
                }
                let tag = if is_adj {
                    match group.strip_prefix("sing.") {
                        Some(gender) => format!("ADJF {gender},sing,{case}"),
                        None => format!("ADJF plur,{case}"),
                    }
                } else {
                    format!("NOUN {group},{case}")
                };
                added.push(format!("{form}\t{lemma}\t{fill}\t{tag}"));
            }
        }
    }
    added.sort();

    let mut out = String::new();
    for h in &header {
        out.push_str(h);
        out.push('\n');
    }
    out.push_str(&format!(
        "# + {} paradigm forms folded in by `lexicon-builder expand` (ratio {ratio})\n",
        added.len()
    ));
    for line in body {
        out.push_str(line);
        out.push('\n');
    }
    for line in &added {
        out.push_str(line);
        out.push('\n');
    }
    if let Err(e) = std::fs::write(&output, out) {
        return fail(&format!("cannot write {output}: {e}"));
    }
    eprintln!("expanded {output}: +{} synthetic forms", added.len());
    ExitCode::SUCCESS
}

fn fail(message: &str) -> ExitCode {
    eprintln!("error: {message}");
    ExitCode::FAILURE
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adds_missing_forms_below_present_and_is_idempotent() {
        let dir = std::env::temp_dir().join(format!("expand_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let lex = dir.join("lex.tsv");
        let hold = dir.join("hold.tsv");
        let out = dir.join("out.tsv");
        let out2 = dir.join("out2.tsv");
        // One present form of an adjective; full paradigm in hold-groups.
        std::fs::write(&lex, "красная\tкрасный\t100\tADJF femn,sing,nomn\n").unwrap();
        std::fs::write(
            &hold,
            "красный\tsing.femn\tкрасная|красной|красной|красную|красной|красной\n\
             красный\tsing.masc\tкрасный|красного|красному|красный|красным|красном\n",
        )
        .unwrap();

        cmd_expand(vec![
            lex.to_string_lossy().into(),
            hold.to_string_lossy().into(),
            "-o".into(),
            out.to_string_lossy().into(),
        ]);
        let r = std::fs::read_to_string(&out).unwrap();
        // A missing cell, added below the present form (round(100*0.25)=25),
        // with a tag synthesized from group + case.
        assert!(
            r.contains("красному\tкрасный\t25\tADJF masc,sing,datv"),
            "{r}"
        );
        // The present form is preserved verbatim, not duplicated/reweighted.
        assert!(
            r.contains("красная\tкрасный\t100\tADJF femn,sing,nomn"),
            "{r}"
        );

        // Idempotent: expanding the expansion adds nothing.
        cmd_expand(vec![
            out.to_string_lossy().into(),
            hold.to_string_lossy().into(),
            "-o".into(),
            out2.to_string_lossy().into(),
        ]);
        let lines = |s: &str| s.lines().filter(|l| !l.starts_with('#')).count();
        assert_eq!(lines(&r), lines(&std::fs::read_to_string(&out2).unwrap()));
    }
}
