//! Developer CLI: the fastest feedback loop for engine work.
//!
//! ```text
//! abbrev suggest првт [--lexicon path.tsv] [--limit 5] [--context "слова до"] [--grouped]
//! abbrev repl [--lexicon path.tsv]
//! abbrev bench data/bench/basic.tsv [--lexicon path.tsv] [--errors fails.tsv]
//! abbrev gen --lexicon path.tsv --count 20000 --seed 42 -o cases.tsv
//! ```

mod generate;

use std::collections::BTreeMap;
use std::io::{BufRead, Write as _};
use std::process::ExitCode;
use std::time::Instant;

use abbrev_core::{BigramModel, Context, Engine, Lexicon};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut args = args.iter().map(String::as_str);
    match args.next() {
        Some("suggest") => cmd_suggest(args.collect()),
        Some("repl") => cmd_repl(args.collect()),
        Some("bench") => cmd_bench(args.collect()),
        Some("gen") => generate::cmd_gen(args.collect()),
        _ => {
            eprintln!("usage: abbrev <suggest|repl|bench|gen> [args]  (see crates/abbrev-cli)");
            ExitCode::FAILURE
        }
    }
}

struct CommonOpts {
    lexicon: Lexicon,
    lm: Option<BigramModel>,
    limit: usize,
    context: Context,
    positional: Vec<String>,
}

/// Engine over the options' lexicon, with the bigram LM plugged in when
/// `--lm` was given.
fn build_engine(lexicon: Lexicon, lm: Option<BigramModel>) -> Engine {
    let mut engine = Engine::new(lexicon);
    if let Some(lm) = lm {
        engine.set_context_model(Box::new(lm));
    }
    engine
}

fn parse_opts(args: Vec<&str>) -> Result<CommonOpts, String> {
    let mut lexicon_path: Option<String> = None;
    let mut lm_path: Option<String> = None;
    let mut limit = 5usize;
    let mut context = Context::default();
    let mut positional = Vec::new();
    let mut it = args.into_iter();
    while let Some(arg) = it.next() {
        match arg {
            "--lexicon" => {
                lexicon_path = Some(it.next().ok_or("--lexicon needs a path")?.to_string());
            }
            "--lm" => {
                lm_path = Some(it.next().ok_or("--lm needs a path")?.to_string());
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
    let lexicon = match lexicon_path {
        Some(path) => {
            let tsv = std::fs::read_to_string(&path)
                .map_err(|e| format!("cannot read lexicon {path}: {e}"))?;
            Lexicon::from_tsv_str(&tsv).map_err(|e| e.to_string())?
        }
        None => Lexicon::demo(),
    };
    let lm = match lm_path {
        Some(path) => {
            let tsv = std::fs::read_to_string(&path)
                .map_err(|e| format!("cannot read lm {path}: {e}"))?;
            Some(BigramModel::from_tsv_str(&tsv).map_err(|e| e.to_string())?)
        }
        None => None,
    };
    Ok(CommonOpts {
        lexicon,
        lm,
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
    let engine = build_engine(opts.lexicon, opts.lm);
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
    let mut engine = build_engine(opts.lexicon, opts.lm);
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

/// Benchmark over `input<TAB>expected[<TAB>tag[<TAB>context]]` lines:
/// top-1 accuracy, top-3 recall, latency — overall and per
/// corruption-rule tag. The optional 4th column is left context
/// (space-separated previous words) fed to the engine per case.
/// `--errors path` dumps the failing cases for analysis.
fn cmd_bench(args: Vec<&str>) -> ExitCode {
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
    let engine = build_engine(opts.lexicon, opts.lm);
    let (mut total, mut top1, mut top3) = (0u32, 0u32, 0u32);
    let mut by_tag: BTreeMap<String, (u32, u32, u32)> = BTreeMap::new();
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
        let suggestions = engine.suggest(input, context, 3);
        latencies_us.push(started.elapsed().as_micros());
        total += 1;
        let hit1 = suggestions.first().is_some_and(|s| s.form == expected);
        let hit3 = suggestions.iter().any(|s| s.form == expected);
        let entry = by_tag.entry(tag.to_string()).or_insert((0, 0, 0));
        entry.0 += 1;
        if hit1 {
            top1 += 1;
            entry.1 += 1;
        }
        if hit3 {
            top3 += 1;
            entry.2 += 1;
        } else if errors_path.is_some() {
            let got: Vec<&str> = suggestions.iter().map(|s| s.form.as_str()).collect();
            errors.push_str(&format!("{input}\t{expected}\t{tag}\t{}\n", got.join("|")));
        }
    }
    if total == 0 {
        return fail("no cases found");
    }
    latencies_us.sort_unstable();
    let mean = latencies_us.iter().sum::<u128>() / latencies_us.len() as u128;
    let p95 = latencies_us[(latencies_us.len() * 95 / 100).min(latencies_us.len() - 1)];
    let pct = |hits: u32, n: u32| 100.0 * f64::from(hits) / f64::from(n.max(1));
    println!("cases:      {total}");
    println!("top-1:      {:.1}%", pct(top1, total));
    println!("top-3:      {:.1}%", pct(top3, total));
    println!("latency:    mean {mean} µs, p95 {p95} µs");
    if by_tag.len() > 1 {
        println!("per tag:");
        for (tag, (n, t1, t3)) in &by_tag {
            println!(
                "  {tag:<10} n={n:<6} top-1 {:.1}%  top-3 {:.1}%",
                pct(*t1, *n),
                pct(*t3, *n)
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

fn fail(message: &str) -> ExitCode {
    eprintln!("error: {message}");
    ExitCode::FAILURE
}
