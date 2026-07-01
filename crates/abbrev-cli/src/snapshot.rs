//! Golden-snapshot harness: deterministic engine output over a fixed input
//! set, for regression diffing between engine variants.
//!
//! The idea the snapshot realizes is exactly a Merkle check done by git: the
//! whole-file `hash=…` printed to stderr is the "top" — equal hash ⇒ nothing
//! changed at all. When it differs, `git diff` of the (sorted, score-rounded)
//! snapshot shows only the lines that moved — the drill-down into the changed
//! "subtrees". So the A/B workflow is:
//!
//!   abbrev snapshot cases.tsv --lexicon … -o snap.A.tsv   # variant A
//!   abbrev snapshot cases.tsv --lexicon … -o snap.B.tsv   # variant B
//!   diff snap.A.tsv snap.B.tsv                            # or git diff a baseline
//!
//! Scores are rounded (float noise must not create phantom diffs) and lines
//! are sorted by (input, context), so the output is reproducible run-to-run.
//! Inputs come from any `input⇥expected⇥tag[⇥context]` file — e.g. `abbrev
//! gen` (deterministic for a seed), giving thousands of cases, not a handful.

use std::process::ExitCode;

use abbrev_core::Context;

use crate::{build_engine, parse_opts};

pub fn cmd_snapshot(args: Vec<&str>) -> ExitCode {
    let mut output: Option<String> = None;
    let mut rest: Vec<&str> = Vec::new();
    let mut it = args.into_iter();
    while let Some(arg) = it.next() {
        match arg {
            "-o" | "--output" => match it.next() {
                Some(p) => output = Some(p.to_string()),
                None => return fail("-o needs a path"),
            },
            other => rest.push(other),
        }
    }
    let opts = match parse_opts(rest) {
        Ok(o) => o,
        Err(e) => return fail(&e),
    };
    let Some(cases_path) = opts.positional.first().cloned() else {
        return fail("snapshot needs a cases file: `abbrev snapshot cases.tsv --lexicon …`");
    };
    let raw = match std::fs::read_to_string(&cases_path) {
        Ok(r) => r,
        Err(e) => return fail(&format!("cannot read {cases_path}: {e}")),
    };
    let limit = opts.limit;
    let engine = build_engine(
        opts.lexicon,
        opts.lm,
        opts.shortcuts,
        opts.paradigms,
        opts.masker,
    );

    // input<TAB>context  →  ranked "form:score|form:score|…" (best form per
    // lemma group, in strip order). Context is part of the key because it
    // changes the ranking.
    let mut rows: Vec<(String, String, String)> = Vec::new();
    for line in raw.lines() {
        let line = line.trim_end();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let fields: Vec<&str> = line.split('\t').collect();
        let input = fields[0];
        let context = fields.get(3).copied().unwrap_or("");
        let ctx = Context::new(
            context
                .split_whitespace()
                .map(String::from)
                .collect::<Vec<_>>(),
        );
        let groups = engine.suggest_grouped(input, &ctx, limit);
        let ranked = groups
            .iter()
            .map(|g| format!("{}:{:.2}", g.best.form, g.best.score))
            .collect::<Vec<_>>()
            .join("|");
        rows.push((input.to_string(), context.to_string(), ranked));
    }
    // Deterministic order so the snapshot is reproducible and diffs are local.
    rows.sort();
    rows.dedup();

    let mut body = String::new();
    for (input, context, ranked) in &rows {
        body.push_str(&format!("{input}\t{context}\t{ranked}\n"));
    }

    let header = format!(
        "# engine snapshot: input\\tcontext\\tform:score|… (limit {limit}) — \
         {} cases, hash={:016x}\n",
        rows.len(),
        fnv1a_64(body.as_bytes()),
    );
    let out = header.clone() + &body;
    match output {
        Some(path) => {
            if let Err(e) = std::fs::write(&path, out) {
                return fail(&format!("cannot write {path}: {e}"));
            }
            eprintln!(
                "snapshot written to {path}: {}",
                header.trim_start_matches('#').trim()
            );
        }
        None => print!("{out}"),
    }
    ExitCode::SUCCESS
}

/// FNV-1a 64-bit: a tiny, stable, dependency-free digest of the snapshot body
/// (the "top of the tree" — equal hash means identical behaviour).
fn fnv1a_64(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for &b in bytes {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

fn fail(message: &str) -> ExitCode {
    eprintln!("error: {message}");
    ExitCode::FAILURE
}
