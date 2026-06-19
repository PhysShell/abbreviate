//! Developer CLI: the fastest feedback loop for engine work.
//!
//! ```text
//! abbrev suggest првт [--lexicon path.tsv] [--limit 5] [--context "слова до"] [--grouped] [--paradigms hold.tsv]
//! abbrev repl [--lexicon path.tsv]
//! abbrev bench data/bench/basic.tsv [--lexicon path.tsv] [--errors fails.tsv]
//! abbrev gen --lexicon path.tsv --count 20000 --seed 42 -o cases.tsv
//! ```

mod generate;
mod snapshot;
mod tune;

use std::collections::BTreeMap;
use std::io::{BufRead, Write as _};
use std::process::ExitCode;
use std::time::Instant;

use abbrev_core::{BigramModel, Context, Engine, Lexicon, Paradigms, Shortcuts};

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
) -> Engine {
    let mut engine = Engine::new(opts_lexicon);
    if let Some(lm) = lm {
        engine.set_context_model(Box::new(lm));
    }
    if let Some(sc) = shortcuts {
        engine.set_shortcuts(sc);
    }
    if let Some(p) = paradigms {
        engine.set_paradigms(p);
    }
    engine
}

fn parse_opts(args: Vec<&str>) -> Result<CommonOpts, String> {
    let mut lexicon_path: Option<String> = None;
    let mut extra_lexicon_paths: Vec<String> = Vec::new();
    let mut lm_path: Option<String> = None;
    let mut shortcuts_paths: Vec<String> = Vec::new();
    let mut paradigms_path: Option<String> = None;
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
    Ok(CommonOpts {
        lexicon,
        lm,
        shortcuts,
        paradigms,
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
    let engine = build_engine(opts.lexicon, opts.lm, opts.shortcuts, opts.paradigms);
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
    let mut engine = build_engine(opts.lexicon, opts.lm, opts.shortcuts, opts.paradigms);
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
    let mut rest: Vec<&str> = Vec::new();
    let mut it = args.into_iter();
    while let Some(arg) = it.next() {
        if arg == "--errors" {
            errors_path = it.next().map(String::from);
        } else {
            rest.push(arg);
        }
    }
    let opts = match parse_opts(rest) {
        Ok(o) => o,
        Err(e) => return fail(&e),
    };
    let Some(path) = opts.positional.first() else {
        return fail("bench needs a cases file: `abbrev bench data/bench/basic.tsv`");
    };
    let cases = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) => return fail(&format!("cannot read {path}: {e}")),
    };
    let engine = build_engine(opts.lexicon, opts.lm, opts.shortcuts, opts.paradigms);
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
