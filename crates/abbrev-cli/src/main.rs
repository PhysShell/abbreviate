//! Developer CLI: the fastest feedback loop for engine work.
//!
//! ```text
//! abbrev suggest првт [--lexicon path.tsv] [--limit 5] [--context "слова до"] [--grouped] [--paradigms hold.tsv] [--mask list.txt]
//! abbrev repl [--lexicon path.tsv]
//! abbrev bench data/bench/basic.tsv [--lexicon path.tsv] [--errors fails.tsv]
//! abbrev bench cases.tsv --recency [--noise N] [--lexicon path.tsv]
//! abbrev gen --lexicon path.tsv --count 20000 --seed 42 -o cases.tsv
//! ```
//!
//! `bench --recency` runs every case cold (empty session) vs warm
//! (`note_word(expected)` then `--noise N` distractors) and reports the
//! cold→warm top-1/top-3 lift — the measurement behind tuning `w_recency`.

mod generate;
mod snapshot;
mod tune;

use std::collections::BTreeMap;
use std::io::{BufRead, Write as _};
use std::process::ExitCode;
use std::time::Instant;

use abbrev_core::engine::EngineConfig;
use abbrev_core::{BigramModel, Context, Engine, Lexicon, Masker, Paradigms, Shortcuts};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut args = args.iter().map(String::as_str);
    match args.next() {
        Some("suggest") => cmd_suggest(args.collect()),
        Some("repl") => cmd_repl(args.collect()),
        Some("bench") => cmd_bench(args.collect()),
        Some("gen") => generate::cmd_gen(args.collect()),
        Some("snapshot") => snapshot::cmd_snapshot(args.collect()),
        Some("tune") => tune::cmd_tune(args.map(String::from).collect()),
        _ => {
            eprintln!(
                "usage: abbrev <suggest|repl|bench|gen|snapshot|tune> [args]  (see crates/abbrev-cli)"
            );
            ExitCode::FAILURE
        }
    }
}

struct CommonOpts {
    lexicon: Lexicon,
    lm: Option<BigramModel>,
    shortcuts: Option<Shortcuts>,
    paradigms: Option<Paradigms>,
    masker: Option<Masker>,
    limit: usize,
    context: Context,
    positional: Vec<String>,
}

/// Engine over the options' lexicon, with the bigram LM, conventional
/// shortcuts and hold-popup paradigms plugged in when their flags were given.
fn build_engine(
    opts_lexicon: Lexicon,
    lm: Option<BigramModel>,
    shortcuts: Option<Shortcuts>,
    paradigms: Option<Paradigms>,
    masker: Option<Masker>,
) -> Engine {
    // A `--mask` list flips the (otherwise off-by-default) masking gate on, so
    // the CLI actually offers the masked twins; without it the engine keeps
    // its default config.
    let mut engine = match &masker {
        Some(_) => Engine::with_config(
            opts_lexicon,
            EngineConfig {
                mask: true,
                ..EngineConfig::default()
            },
        ),
        None => Engine::new(opts_lexicon),
    };
    if let Some(lm) = lm {
        engine.set_context_model(Box::new(lm));
    }
    if let Some(sc) = shortcuts {
        engine.set_shortcuts(sc);
    }
    if let Some(p) = paradigms {
        engine.set_paradigms(p);
    }
    if let Some(m) = masker {
        engine.set_masker(m);
    }
    engine
}

fn parse_opts(args: Vec<&str>) -> Result<CommonOpts, String> {
    let mut lexicon_path: Option<String> = None;
    let mut extra_lexicon_paths: Vec<String> = Vec::new();
    let mut lm_path: Option<String> = None;
    let mut shortcuts_paths: Vec<String> = Vec::new();
    let mut paradigms_path: Option<String> = None;
    let mut mask_path: Option<String> = None;
    let mut limit = 5usize;
    let mut context = Context::default();
    let mut positional = Vec::new();
    let mut it = args.into_iter();
    while let Some(arg) = it.next() {
        match arg {
            "--lexicon" => {
                lexicon_path = Some(it.next().ok_or("--lexicon needs a path")?.to_string());
            }
            "--extra-lexicon" => {
                // Repeatable: fold an opt-in source (e.g. the gunzipped names
                // lexicon) into the base. Not forced on any platform.
                extra_lexicon_paths
                    .push(it.next().ok_or("--extra-lexicon needs a path")?.to_string());
            }
            "--lm" => {
                lm_path = Some(it.next().ok_or("--lm needs a path")?.to_string());
            }
            "--shortcuts" => {
                // Repeatable: the exact-match layer is one namespace, so the
                // conventional shorthand (data/shortcuts/ru.tsv) and the
                // transliteration terms (data/translit/ru-tech.tsv) load
                // together. Same-key lines stack as ordered expansions.
                shortcuts_paths.push(it.next().ok_or("--shortcuts needs a path")?.to_string());
            }
            "--paradigms" => {
                paradigms_path = Some(it.next().ok_or("--paradigms needs a path")?.to_string());
            }
            "--mask" => {
                mask_path = Some(it.next().ok_or("--mask needs a path")?.to_string());
            }
            "--limit" => {
                limit = it
                    .next()
                    .ok_or("--limit needs a number")?
                    .parse()
                    .map_err(|_| "--limit needs a number")?;
            }
            "--context" => {
                let words = it.next().ok_or("--context needs a string")?;
                context = Context::new(words.split_whitespace().map(String::from).collect());
            }
            other => positional.push(other.to_string()),
        }
    }
    let mut lexicon = match lexicon_path {
        Some(path) => {
            let tsv = std::fs::read_to_string(&path)
                .map_err(|e| format!("cannot read lexicon {path}: {e}"))?;
            Lexicon::from_tsv_str(&tsv).map_err(|e| e.to_string())?
        }
        None => Lexicon::demo(),
    };
    for path in &extra_lexicon_paths {
        let tsv = std::fs::read_to_string(path)
            .map_err(|e| format!("cannot read extra lexicon {path}: {e}"))?;
        lexicon
            .extend_from_tsv_str(&tsv)
            .map_err(|e| format!("{path}: {e}"))?;
    }
    let lm = match lm_path {
        Some(path) => {
            let tsv = std::fs::read_to_string(&path)
                .map_err(|e| format!("cannot read lm {path}: {e}"))?;
            Some(BigramModel::from_tsv_str(&tsv).map_err(|e| e.to_string())?)
        }
        None => None,
    };
    let shortcuts = if shortcuts_paths.is_empty() {
        None
    } else {
        // Concatenate all shortcut sources; from_tsv_str merges keys across
        // them (same-key lines stack).
        let mut tsv = String::new();
        for path in &shortcuts_paths {
            tsv.push_str(
                &std::fs::read_to_string(path)
                    .map_err(|e| format!("cannot read shortcuts {path}: {e}"))?,
            );
            tsv.push('\n');
        }
        Some(Shortcuts::from_tsv_str(&tsv).map_err(|e| e.to_string())?)
    };
    let paradigms = match paradigms_path {
        Some(path) => {
            let tsv = std::fs::read_to_string(&path)
                .map_err(|e| format!("cannot read paradigms {path}: {e}"))?;
            Some(Paradigms::from_tsv_str(&tsv))
        }
        None => None,
    };
    let masker = match mask_path {
        Some(path) => {
            let list = std::fs::read_to_string(&path)
                .map_err(|e| format!("cannot read mask list {path}: {e}"))?;
            Some(Masker::from_list_str(&list).map_err(|e| e.to_string())?)
        }
        None => None,
    };
    Ok(CommonOpts {
        lexicon,
        lm,
        shortcuts,
        paradigms,
        masker,
        limit,
        context,
        positional,
    })
}

fn cmd_suggest(args: Vec<&str>) -> ExitCode {
    let grouped = args.contains(&"--grouped");
    let args = args.into_iter().filter(|a| *a != "--grouped").collect();
    let opts = match parse_opts(args) {
        Ok(o) => o,
        Err(e) => return fail(&e),
    };
    let Some(input) = opts.positional.first() else {
        return fail("suggest needs an input word, e.g. `abbrev suggest првт`");
    };
    let engine = build_engine(
        opts.lexicon,
        opts.lm,
        opts.shortcuts,
        opts.paradigms,
        opts.masker,
    );
    let started = Instant::now();
    if grouped {
        // The two-level strip: one line per lemma, variants on "hold".
        let groups = engine.suggest_grouped(input, &opts.context, opts.limit);
        let elapsed = started.elapsed();
        for (i, g) in groups.iter().enumerate() {
            let variants = if g.variants.is_empty() {
                String::new()
            } else {
                format!("  | hold: {}", g.variants.join(" "))
            };
            println!(
                "{:>2}. {:<20} (лемма: {}, score: {:.2}){variants}",
                i + 1,
                g.best.form,
                g.lemma,
                g.best.score
            );
        }
        eprintln!("-- {} groups in {:?}", groups.len(), elapsed);
        return ExitCode::SUCCESS;
    }
    let suggestions = engine.suggest(input, &opts.context, opts.limit);
    let elapsed = started.elapsed();
    for (i, s) in suggestions.iter().enumerate() {
        println!(
            "{:>2}. {:<20} (лемма: {}, score: {:.2})",
            i + 1,
            s.form,
            s.lemma,
            s.score
        );
    }
    eprintln!("-- {} candidates in {:?}", suggestions.len(), elapsed);
    ExitCode::SUCCESS
}

fn cmd_repl(args: Vec<&str>) -> ExitCode {
    let opts = match parse_opts(args) {
        Ok(o) => o,
        Err(e) => return fail(&e),
    };
    let mut engine = build_engine(
        opts.lexicon,
        opts.lm,
        opts.shortcuts,
        opts.paradigms,
        opts.masker,
    );
    eprintln!("abbrev repl — введите сокращение; `!N` принимает вариант N; пустая строка — выход");
    let stdin = std::io::stdin();
    let mut last_input = String::new();
    let mut last: Vec<abbrev_core::Suggestion> = Vec::new();
    loop {
        eprint!("> ");
        let _ = std::io::stderr().flush();
        let mut line = String::new();
        if stdin.lock().read_line(&mut line).unwrap_or(0) == 0 {
            break;
        }
        let line = line.trim();
        if line.is_empty() {
            break;
        }
        if let Some(n) = line.strip_prefix('!').and_then(|n| n.parse::<usize>().ok()) {
            if let Some(s) = last.get(n.saturating_sub(1)) {
                engine.accept(&last_input, &s.form);
                eprintln!("принято: {} → {}", last_input, s.form);
            }
            continue;
        }
        last = engine.suggest(line, &opts.context, opts.limit);
        last_input = line.to_string();
        for (i, s) in last.iter().enumerate() {
            println!("{:>2}. {:<20} (score: {:.2})", i + 1, s.form, s.score);
        }
    }
    ExitCode::SUCCESS
}

/// Aggregate benchmark counters. Floats are summed and divided by `total`
/// at report time so every metric is comparable across runs (A/B for tune,
/// LM, etc.). This is the dashboard the project optimizes against.
#[derive(Default)]
struct Metrics {
    total: u32,
    top1: u32,
    top3: u32,
    /// Σ reciprocal rank of the expected form within the flat top-K.
    rr_sum: f64,
    /// Σ keystroke savings counted only on top-1 hits (misses save 0,
    /// because a miss means the user types the full word).
    ks_realized: f64,
    /// Σ keystroke savings over top-3 hits (oracle: best the strip can do).
    ks_oracle: f64,
    /// Σ score(top1) − score(top2); calibrates the autocorrect margin.
    margin_sum: f64,
    margin_n: u32,
    /// Grouped-strip metrics over the top-3 lemma groups.
    lemma_hit: u32,
    best_form_hit: u32,
    hold_success: u32,
}

/// Benchmark over `input<TAB>expected[<TAB>tag[<TAB>context]]` lines.
/// Reports accuracy (top-1, top-3), ranking quality (MRR), keystroke
/// savings, autocorrect margin, latency, and grouped-strip quality
/// (lemma/best-form/hold hit@3) — overall and per corruption-rule tag.
/// The optional 4th column is per-case left context.
/// `--errors path` dumps the failing cases for analysis.
fn cmd_bench(args: Vec<&str>) -> ExitCode {
    const FLAT_K: usize = 10;
    const GROUPS: usize = 3;
    let mut errors_path: Option<String> = None;
    let mut recency = false;
    let mut noise = 0usize;
    let mut noise_given = false;
    let mut rest: Vec<&str> = Vec::new();
    let mut it = args.into_iter();
    while let Some(arg) = it.next() {
        match arg {
            "--errors" => errors_path = it.next().map(String::from),
            // Recency slice: measure the session-cache lift (Parts 1+2) by
            // running each case cold vs warm. `--noise N` ages the prior.
            "--recency" => recency = true,
            "--noise" => match it.next().and_then(|v| v.parse().ok()) {
                Some(v) => {
                    noise = v;
                    noise_given = true;
                }
                None => return fail("--noise needs a number"),
            },
            other => rest.push(other),
        }
    }
    // `--noise` only means something in the recency slice; in the normal path
    // it would be silently ignored, so reject it instead of reporting the
    // wrong mode's metrics.
    if noise_given && !recency {
        return fail("--noise requires --recency");
    }
    let opts = match parse_opts(rest) {
        Ok(o) => o,
        Err(e) => return fail(&e),
    };
    let Some(path) = opts.positional.first().cloned() else {
        return fail("bench needs a cases file: `abbrev bench data/bench/basic.tsv`");
    };
    if recency {
        return run_recency(opts, &path, noise);
    }
    let cases = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(e) => return fail(&format!("cannot read {path}: {e}")),
    };
    let engine = build_engine(
        opts.lexicon,
        opts.lm,
        opts.shortcuts,
        opts.paradigms,
        opts.masker,
    );
    let mut m = Metrics::default();
    let mut by_tag: BTreeMap<String, Metrics> = BTreeMap::new();
    let mut latencies_us: Vec<u128> = Vec::new();
    let mut errors = String::new();
    for line in cases.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut fields = line.split('\t');
        let (Some(input), Some(expected)) = (fields.next(), fields.next()) else {
            return fail(&format!("bad bench line (need input\\texpected): {line}"));
        };
        let tag = fields.next().unwrap_or("untagged");
        let case_context = fields
            .next()
            .map(|words| Context::new(words.split_whitespace().map(String::from).collect()));
        let context = case_context.as_ref().unwrap_or(&opts.context);

        let started = Instant::now();
        let flat = engine.suggest(input, context, FLAT_K);
        let groups = engine.suggest_grouped(input, context, GROUPS);
        latencies_us.push(started.elapsed().as_micros());

        let savings = keystroke_savings(input, expected);
        let rank = flat.iter().position(|s| s.form == expected);
        let hit1 = rank == Some(0);
        let hit3 = rank.is_some_and(|r| r < GROUPS);
        // Grouped strip: did the right lemma surface, and was the form the
        // group's best or only reachable by holding?
        let best_form_hit = groups.iter().any(|g| g.best.form == expected);
        let lemma_hit = groups
            .iter()
            .any(|g| g.best.form == expected || g.variants.iter().any(|v| v == expected));

        let tag_m = by_tag.entry(tag.to_string()).or_default();
        for slot in [&mut m, tag_m] {
            slot.total += 1;
            if hit1 {
                slot.top1 += 1;
                slot.ks_realized += savings;
            }
            if hit3 {
                slot.top3 += 1;
                slot.ks_oracle += savings;
            }
            if let Some(r) = rank {
                slot.rr_sum += 1.0 / (r as f64 + 1.0);
            }
            if flat.len() >= 2 {
                slot.margin_sum += f64::from(flat[0].score - flat[1].score);
                slot.margin_n += 1;
            }
            if best_form_hit {
                slot.best_form_hit += 1;
            }
            if lemma_hit {
                slot.lemma_hit += 1;
                if !best_form_hit {
                    slot.hold_success += 1;
                }
            }
        }
        if !hit3 && errors_path.is_some() {
            let got: Vec<&str> = flat.iter().take(3).map(|s| s.form.as_str()).collect();
            errors.push_str(&format!("{input}\t{expected}\t{tag}\t{}\n", got.join("|")));
        }
    }
    if m.total == 0 {
        return fail("no cases found");
    }
    latencies_us.sort_unstable();
    let mean = latencies_us.iter().sum::<u128>() / latencies_us.len() as u128;
    let p95 = latencies_us[(latencies_us.len() * 95 / 100).min(latencies_us.len() - 1)];

    let pct = |x: u32, n: u32| 100.0 * f64::from(x) / f64::from(n.max(1));
    println!("cases:        {}", m.total);
    println!("top-1:        {:.1}%", pct(m.top1, m.total));
    println!("top-3:        {:.1}%", pct(m.top3, m.total));
    println!("MRR:          {:.3}", m.rr_sum / f64::from(m.total));
    println!(
        "keystrokes:   realized {:.1}%  oracle@3 {:.1}%",
        100.0 * m.ks_realized / f64::from(m.total),
        100.0 * m.ks_oracle / f64::from(m.total),
    );
    println!(
        "group@3:      lemma {:.1}%  best-form {:.1}%  hold-rescue {:.1}%",
        pct(m.lemma_hit, m.total),
        pct(m.best_form_hit, m.total),
        pct(m.hold_success, m.total),
    );
    println!(
        "margin t1-t2: {:.2}",
        m.margin_sum / f64::from(m.margin_n.max(1)),
    );
    println!("latency:      mean {mean} µs, p95 {p95} µs");
    if by_tag.len() > 1 {
        println!("per tag:");
        for (tag, t) in &by_tag {
            println!(
                "  {tag:<10} n={:<6} top-1 {:.1}%  top-3 {:.1}%  MRR {:.3}  ks {:.1}%",
                t.total,
                pct(t.top1, t.total),
                pct(t.top3, t.total),
                t.rr_sum / f64::from(t.total.max(1)),
                100.0 * t.ks_realized / f64::from(t.total.max(1)),
            );
        }
    }
    if let Some(path) = errors_path {
        if let Err(e) = std::fs::write(&path, errors) {
            return fail(&format!("cannot write {path}: {e}"));
        }
        eprintln!("top-3 misses dumped to {path}");
    }
    ExitCode::SUCCESS
}

/// Cold-vs-warm counters for the recency slice.
#[derive(Default)]
struct RecencyMetrics {
    total: u32,
    cold_top1: u32,
    cold_top3: u32,
    warm_top1: u32,
    warm_top3: u32,
}

/// Runs each `(input, expected, context)` case twice on the same engine:
/// **cold** (empty session) and **warm** (the session primed with
/// `note_word(expected)` followed by `noise` distractor words, so the prior has
/// aged `noise` words). The cold→warm delta is the recency lift — for an
/// in-lexicon `expected` it is the ranking boost (Part 1); for an OOV `expected`
/// cold is unreachable and warm measures retrieval (Part 2). The per-case
/// context (4th bench column / `--context`) is honored so the slice is measured
/// under the same ranking model as the normal bench. Deterministic: distractors
/// are drawn in lexicon order via a rolling cursor.
fn recency_eval(
    engine: &mut Engine,
    cases: &[(String, String, Context)],
    noise: usize,
    distractors: &[String],
) -> RecencyMetrics {
    const K: usize = 10;
    let mut m = RecencyMetrics::default();
    let mut cursor = 0usize;
    for (input, expected, ctx) in cases {
        m.total += 1;

        engine.reset_session();
        let cold = engine.suggest(input, ctx, K);
        let cold_rank = cold.iter().position(|s| &s.form == expected);
        if cold_rank == Some(0) {
            m.cold_top1 += 1;
        }
        if cold_rank.is_some_and(|r| r < 3) {
            m.cold_top3 += 1;
        }

        engine.reset_session();
        engine.note_word(expected);
        // Distractors are normalized (like the pool), so compare against the
        // normalized target key — a ё/case variant must still count as the
        // target and be skipped, not re-noted as noise.
        let target_norm = abbrev_core::alphabet::normalize(expected);
        let mut added = 0;
        // Age the prior by `noise` distractors (skip ones equal to the target).
        // `since_add` bounds a full no-progress pass: if a whole cycle of the
        // pool yields no usable distractor (every entry equals the target),
        // stop instead of looping forever.
        let mut since_add = 0;
        while added < noise && !distractors.is_empty() && since_add < distractors.len() {
            let d = &distractors[cursor % distractors.len()];
            cursor += 1;
            if d != &target_norm {
                engine.note_word(d);
                added += 1;
                since_add = 0;
            } else {
                since_add += 1;
            }
        }
        let warm = engine.suggest(input, ctx, K);
        let warm_rank = warm.iter().position(|s| &s.form == expected);
        if warm_rank == Some(0) {
            m.warm_top1 += 1;
        }
        if warm_rank.is_some_and(|r| r < 3) {
            m.warm_top3 += 1;
        }
    }
    m
}

/// `abbrev bench <cases> --recency [--noise N]`: reports the session-cache
/// lift (cold vs warm top-1/top-3) so `w_recency` can be tuned against a real
/// measurement. Works on any `abbrev gen` output.
fn run_recency(opts: CommonOpts, path: &str, noise: usize) -> ExitCode {
    let text = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) => return fail(&format!("cannot read {path}: {e}")),
    };
    let mut cases: Vec<(String, String, Context)> = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut f = line.split('\t');
        let (Some(input), Some(expected)) = (f.next(), f.next()) else {
            return fail(&format!("bad bench line (need input\\texpected): {line}"));
        };
        let _tag = f.next();
        // 4th column is the per-case left context (as in the normal bench);
        // fall back to the global --context so --lm is measured fairly.
        let context = f
            .next()
            .map(|words| Context::new(words.split_whitespace().map(String::from).collect()))
            .unwrap_or_else(|| opts.context.clone());
        cases.push((input.to_string(), expected.to_string(), context));
    }
    if cases.is_empty() {
        return fail("no cases found");
    }
    let mut engine = build_engine(
        opts.lexicon,
        opts.lm,
        opts.shortcuts,
        opts.paradigms,
        opts.masker,
    );
    // Distractor pool: normalized lexicon forms, collected before the mutable
    // borrow of the engine in `recency_eval`.
    let distractors: Vec<String> = engine
        .lexicon()
        .iter()
        .map(|(_, e)| abbrev_core::alphabet::normalize(&e.form))
        .collect();
    let m = recency_eval(&mut engine, &cases, noise, &distractors);

    let pct = |x: u32| 100.0 * f64::from(x) / f64::from(m.total.max(1));
    println!("recency slice (noise={noise}, cases={}):", m.total);
    println!(
        "  cold  top-1 {:.1}%  top-3 {:.1}%",
        pct(m.cold_top1),
        pct(m.cold_top3)
    );
    println!(
        "  warm  top-1 {:.1}%  top-3 {:.1}%",
        pct(m.warm_top1),
        pct(m.warm_top3)
    );
    println!(
        "  lift  top-1 {:+.1}pp  top-3 {:+.1}pp",
        pct(m.warm_top1) - pct(m.cold_top1),
        pct(m.warm_top3) - pct(m.cold_top3),
    );
    ExitCode::SUCCESS
}

/// Fraction of keystrokes saved by accepting `expected` after typing
/// `input`, in chars, clamped to [0, 1].
fn keystroke_savings(input: &str, expected: &str) -> f64 {
    let typed = input.chars().count() as f64;
    let full = expected.chars().count() as f64;
    if full <= 0.0 {
        return 0.0;
    }
    (1.0 - typed / full).clamp(0.0, 1.0)
}

fn fail(message: &str) -> ExitCode {
    eprintln!("error: {message}");
    ExitCode::FAILURE
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recency_eval_reports_warm_lift() {
        // Close frequencies so the boost is decisive at top-1 (the demo's
        // привет dominates приват on frequency alone).
        let lexicon = Lexicon::from_tsv_str("привет\tпривет\t150\nприват\tприват\t100\n").unwrap();
        let mut engine = Engine::new(lexicon);
        let cases = vec![("првт".to_string(), "приват".to_string(), Context::default())];
        let m = recency_eval(&mut engine, &cases, 0, &[]);
        // Cold: frequency picks привет, so приват is not top-1.
        assert_eq!(m.cold_top1, 0);
        // Warm: noting приват floats it to the top — the measured lift.
        assert_eq!(m.warm_top1, 1);
        assert_eq!(m.total, 1);
    }

    #[test]
    fn recency_eval_consumes_distractors_with_wraparound() {
        // noise (10) exceeds the distractor pool (2): the cursor must wrap
        // without panicking. The distractors are out-of-lexicon and don't
        // share приват's skeleton, so they don't compete — приват stays
        // reachable in top-3 both cold and warm.
        let lexicon = Lexicon::from_tsv_str("привет\tпривет\t150\nприват\tприват\t100\n").unwrap();
        let mut engine = Engine::new(lexicon);
        let cases = vec![("првт".to_string(), "приват".to_string(), Context::default())];
        let distractors = vec!["солнце".to_string(), "ветер".to_string()];
        let m = recency_eval(&mut engine, &cases, 10, &distractors);
        assert_eq!(m.total, 1);
        assert_eq!(m.cold_top3, 1);
        assert_eq!(m.warm_top3, 1);
    }

    #[test]
    fn recency_eval_terminates_when_all_distractors_are_the_target() {
        // Pathological pool: every distractor equals the target, so none is
        // usable. The no-progress guard must stop the loop instead of hanging.
        let lexicon = Lexicon::from_tsv_str("привет\tпривет\t150\nприват\tприват\t100\n").unwrap();
        let mut engine = Engine::new(lexicon);
        let cases = vec![("првт".to_string(), "приват".to_string(), Context::default())];
        let distractors = vec!["приват".to_string(), "приват".to_string()];
        let m = recency_eval(&mut engine, &cases, 5, &distractors);
        assert_eq!(m.total, 1); // completes
    }
}
