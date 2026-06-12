//! Developer CLI: the fastest feedback loop for engine work.
//!
//! ```text
//! abbrev suggest првт [--lexicon path.tsv] [--limit 5] [--context "слова до"]
//! abbrev repl [--lexicon path.tsv]
//! abbrev bench data/bench/basic.tsv [--lexicon path.tsv]
//! ```

use std::io::{BufRead, Write as _};
use std::process::ExitCode;
use std::time::Instant;

use abbrev_core::{Context, Engine, Lexicon};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut args = args.iter().map(String::as_str);
    match args.next() {
        Some("suggest") => cmd_suggest(args.collect()),
        Some("repl") => cmd_repl(args.collect()),
        Some("bench") => cmd_bench(args.collect()),
        _ => {
            eprintln!("usage: abbrev <suggest|repl|bench> [args]  (see crates/abbrev-cli)");
            ExitCode::FAILURE
        }
    }
}

struct CommonOpts {
    lexicon: Lexicon,
    limit: usize,
    context: Context,
    positional: Vec<String>,
}

fn parse_opts(args: Vec<&str>) -> Result<CommonOpts, String> {
    let mut lexicon_path: Option<String> = None;
    let mut limit = 5usize;
    let mut context = Context::default();
    let mut positional = Vec::new();
    let mut it = args.into_iter();
    while let Some(arg) = it.next() {
        match arg {
            "--lexicon" => {
                lexicon_path = Some(it.next().ok_or("--lexicon needs a path")?.to_string());
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
    Ok(CommonOpts {
        lexicon,
        limit,
        context,
        positional,
    })
}

fn cmd_suggest(args: Vec<&str>) -> ExitCode {
    let opts = match parse_opts(args) {
        Ok(o) => o,
        Err(e) => return fail(&e),
    };
    let Some(input) = opts.positional.first() else {
        return fail("suggest needs an input word, e.g. `abbrev suggest првт`");
    };
    let engine = Engine::new(opts.lexicon);
    let started = Instant::now();
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
    let mut engine = Engine::new(opts.lexicon);
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

/// Benchmark over `input<TAB>expected` lines: top-1 accuracy, top-3 recall,
/// mean and p95 latency — the non-negotiable metrics of the project.
fn cmd_bench(args: Vec<&str>) -> ExitCode {
    let opts = match parse_opts(args) {
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
    let engine = Engine::new(opts.lexicon);
    let (mut total, mut top1, mut top3) = (0u32, 0u32, 0u32);
    let mut latencies_us: Vec<u128> = Vec::new();
    for line in cases.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((input, expected)) = line.split_once('\t') else {
            return fail(&format!("bad bench line (need input\\texpected): {line}"));
        };
        let started = Instant::now();
        let suggestions = engine.suggest(input, &opts.context, 3);
        latencies_us.push(started.elapsed().as_micros());
        total += 1;
        if suggestions.first().is_some_and(|s| s.form == expected) {
            top1 += 1;
        }
        if suggestions.iter().any(|s| s.form == expected) {
            top3 += 1;
        }
    }
    if total == 0 {
        return fail("no cases found");
    }
    latencies_us.sort_unstable();
    let mean = latencies_us.iter().sum::<u128>() / latencies_us.len() as u128;
    let p95 = latencies_us[(latencies_us.len() * 95 / 100).min(latencies_us.len() - 1)];
    println!("cases:      {total}");
    println!(
        "top-1:      {:.1}%",
        100.0 * f64::from(top1) / f64::from(total)
    );
    println!(
        "top-3:      {:.1}%",
        100.0 * f64::from(top3) / f64::from(total)
    );
    println!("latency:    mean {mean} µs, p95 {p95} µs");
    ExitCode::SUCCESS
}

fn fail(message: &str) -> ExitCode {
    eprintln!("error: {message}");
    ExitCode::FAILURE
}
