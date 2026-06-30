//! `abbrev tune` — random search over the ranking [`Weights`] on a held-out
//! benchmark, with hard constraints so a higher headline number can't hide
//! a regression:
//!
//! * search maximizes `top1 + 0.3·top3 + 0.1·MRR` on the train set;
//! * a candidate is rejected if **any** corruption tag's top-3 drops more
//!   than 0.5pp below baseline (no trading one tag for another);
//! * the winner is reported on a separate `--valid` set when given, so the
//!   gain is not just generator overfit;
//! * weights are only printed as "ADOPT" when the validation objective
//!   beats baseline by a real margin — small wins are noise.
//!
//! Weights are not written into the source automatically: the command
//! prints them, and a human decides whether to bake them into
//! `rank::Weights::default` and re-verify the acceptance set.

use std::collections::{BTreeMap, HashMap};
use std::process::ExitCode;

use abbrev_core::alphabet::{normalize, skeleton};
use abbrev_core::{BigramModel, Context, Engine, Lexicon, Shortcuts, Weights};

/// xorshift64 — deterministic, dependency-free.
struct Rng(u64);
impl Rng {
    fn new(seed: u64) -> Self {
        Self(seed.max(1))
    }
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
    fn unit(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }
    /// Multiplicative jitter in `[1-amount, 1+amount]`.
    fn jitter(&mut self, amount: f64) -> f32 {
        (1.0 + (self.unit() * 2.0 - 1.0) * amount) as f32
    }
}

struct Case {
    input: String,
    expected: String,
    tag: String,
    context: Context,
}

#[derive(Default, Clone)]
struct Eval {
    n: u32,
    top1: u32,
    top3: u32,
    rr: f64,
    by_tag_top3: BTreeMap<String, (u32, u32)>, // (hits, n)
}

impl Eval {
    fn objective(&self) -> f64 {
        let n = f64::from(self.n.max(1));
        let top1 = f64::from(self.top1) / n;
        let top3 = f64::from(self.top3) / n;
        let mrr = self.rr / n;
        top1 + 0.3 * top3 + 0.1 * mrr
    }
}

fn parse_cases(text: &str) -> Vec<Case> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .filter_map(|l| {
            let mut f = l.split('\t');
            let input = f.next()?.to_string();
            let expected = f.next()?.to_string();
            let tag = f.next().unwrap_or("untagged").to_string();
            let context = Context::new(
                f.next()
                    .map(|w| w.split_whitespace().map(String::from).collect())
                    .unwrap_or_default(),
            );
            Some(Case {
                input,
                expected,
                tag,
                context,
            })
        })
        .collect()
}

fn evaluate(engine: &Engine, cases: &[Case]) -> Eval {
    let mut e = Eval::default();
    for case in cases {
        let sugg = engine.suggest(&case.input, &case.context, 10);
        let rank = sugg.iter().position(|s| s.form == case.expected);
        e.n += 1;
        let slot = e.by_tag_top3.entry(case.tag.clone()).or_default();
        slot.1 += 1;
        if rank == Some(0) {
            e.top1 += 1;
        }
        if rank.is_some_and(|r| r < 3) {
            e.top3 += 1;
            slot.0 += 1;
        }
        if let Some(r) = rank {
            e.rr += 1.0 / (r as f64 + 1.0);
        }
    }
    e
}

/// Per-tag top-3 must not drop more than this below baseline.
const TAG_TOLERANCE: f64 = 0.005;
/// Minimum validation-objective gain to recommend adopting the weights.
const ADOPT_MARGIN: f64 = 0.002;

fn passes_tag_constraint(base: &Eval, cand: &Eval) -> bool {
    cand.by_tag_top3.iter().all(|(tag, &(hits, n))| {
        let cand_rate = f64::from(hits) / f64::from(n.max(1));
        let base_rate = base
            .by_tag_top3
            .get(tag)
            .map(|&(h, m)| f64::from(h) / f64::from(m.max(1)))
            .unwrap_or(0.0);
        cand_rate >= base_rate - TAG_TOLERANCE
    })
}

fn jittered(base: &Weights, rng: &mut Rng, amount: f64) -> Weights {
    Weights {
        skeleton: (base.skeleton * rng.jitter(amount)).max(0.0),
        suffix: (base.suffix * rng.jitter(amount)).max(0.0),
        prefix: (base.prefix * rng.jitter(amount)).max(0.0),
        edit: (base.edit * rng.jitter(amount)).max(0.0),
        freq: (base.freq * rng.jitter(amount)).max(0.0),
        context: (base.context * rng.jitter(amount)).max(0.0),
        user: (base.user * rng.jitter(amount)).max(0.0),
        morph: (base.morph * rng.jitter(amount)).max(0.0),
        // Pinned, not jittered: tune cases never call `note_word`, so
        // `recency_prior` is 0 for every candidate and this weight has no
        // effect on the objective. Jittering it would let random search bake
        // an arbitrary, unvalidated multiplier into the adopted weights
        // (carried along by steps accepted on the *other* weights). Tune it
        // only once the benchmark exercises recency (the `abbrev gen` slice).
        recency: base.recency,
    }
}

pub fn cmd_tune(args: Vec<String>) -> ExitCode {
    let mut train_path: Option<String> = None;
    let mut valid_path: Option<String> = None;
    let mut lexicon_path: Option<String> = None;
    let mut lm_path: Option<String> = None;
    let mut shortcuts_path: Option<String> = None;
    let mut iters = 300usize;
    let mut seed = 1u64;
    let mut recency = false;
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--train" => train_path = it.next().cloned(),
            "--valid" => valid_path = it.next().cloned(),
            "--lexicon" => lexicon_path = it.next().cloned(),
            "--lm" => lm_path = it.next().cloned(),
            "--shortcuts" => shortcuts_path = it.next().cloned(),
            // Sweep w_recency instead of random-searching all weights — recency
            // is gated by the session cache, so it needs its own harness.
            "--recency" => recency = true,
            "--iters" => match it.next().and_then(|v| v.parse().ok()) {
                Some(v) => iters = v,
                None => return fail("--iters needs a number"),
            },
            "--seed" => match it.next().and_then(|v| v.parse().ok()) {
                Some(v) => seed = v,
                None => return fail("--seed needs a number"),
            },
            other => return fail(&format!("tune: unknown argument {other}")),
        }
    }
    let Some(train_path) = train_path else {
        return fail("tune needs --train <cases.tsv>");
    };

    let mut engine = match load_engine(&lexicon_path, &lm_path, &shortcuts_path) {
        Ok(e) => e,
        Err(e) => return fail(&e),
    };
    let train = match std::fs::read_to_string(&train_path) {
        Ok(t) => parse_cases(&t),
        Err(e) => return fail(&format!("cannot read {train_path}: {e}")),
    };
    if train.is_empty() {
        return fail("no training cases");
    }
    if recency {
        return recency_sweep(&mut engine, &train);
    }
    let valid = match &valid_path {
        Some(p) => match std::fs::read_to_string(p) {
            Ok(t) => parse_cases(&t),
            Err(e) => return fail(&format!("cannot read {p}: {e}")),
        },
        None => Vec::new(),
    };

    let baseline = Weights::default();
    engine.set_weights(baseline);
    let base_eval = evaluate(&engine, &train);

    let mut rng = Rng::new(seed);
    let mut best = baseline;
    let mut best_obj = base_eval.objective();
    let mut accepted = 0u32;
    // Anneal the jitter so early iters explore, later ones refine.
    for i in 0..iters {
        let amount = 0.5 * (1.0 - i as f64 / iters as f64).max(0.15);
        let cand = jittered(&best, &mut rng, amount);
        engine.set_weights(cand);
        let eval = evaluate(&engine, &train);
        if eval.objective() > best_obj && passes_tag_constraint(&base_eval, &eval) {
            best = cand;
            best_obj = eval.objective();
            accepted += 1;
        }
    }

    println!("train cases: {}", train.len());
    print_eval("baseline (train)", &base_eval);
    engine.set_weights(best);
    print_eval("tuned    (train)", &evaluate(&engine, &train));
    println!("accepted steps: {accepted}/{iters}");

    let (report, adopt) = if !valid.is_empty() {
        engine.set_weights(baseline);
        let bv = evaluate(&engine, &valid);
        engine.set_weights(best);
        let tv = evaluate(&engine, &valid);
        print_eval("baseline (valid)", &bv);
        print_eval("tuned    (valid)", &tv);
        ("valid", tv.objective() - bv.objective() > ADOPT_MARGIN)
    } else {
        (
            "train (no --valid; overfit risk)",
            best_obj - base_eval.objective() > ADOPT_MARGIN,
        )
    };

    println!(
        "\nweights: skeleton={:.3} suffix={:.3} prefix={:.3} edit={:.3} freq={:.3} context={:.3} user={:.3} morph={:.3} recency={:.3}",
        best.skeleton,
        best.suffix,
        best.prefix,
        best.edit,
        best.freq,
        best.context,
        best.user,
        best.morph,
        best.recency
    );
    if adopt {
        println!("verdict: ADOPT — {report} objective improves beyond the margin.");
    } else {
        println!("verdict: KEEP BASELINE — gain on {report} is within noise.");
    }
    ExitCode::SUCCESS
}

/// Sweeps `w_recency` over a grid, reporting two top-1 rates per value:
///
/// * **positive** — note `expected`, then suggest: how often recency floats the
///   just-used word to the top (the lift we want);
/// * **adversarial** — note a *competitor* (another word with the same input
///   skeleton), then suggest: how often `expected` still wins despite a
///   *different* recent word competing (the over-boost we must avoid).
///
/// Unlike the other weights, recency is gated by the cache (`recency_prior` is 0
/// otherwise), so it has zero cost on the plain benchmark and a naive
/// maximize-positive search would push it to infinity. The adversarial column is
/// the brake: it falls as the weight rises. The knee — high positive while
/// adversarial stays near its `w=0` value — is the weight to adopt.
fn recency_sweep(engine: &mut Engine, cases: &[Case]) -> ExitCode {
    // Top-2 normalized forms per skeleton, by frequency: a competitor for a case
    // is the most frequent same-skeleton form that isn't the expected answer.
    let mut by_skel: HashMap<String, Vec<(String, f32)>> = HashMap::new();
    for (_, e) in engine.lexicon().iter() {
        let norm = normalize(&e.form);
        let entry = by_skel.entry(skeleton(&norm)).or_default();
        entry.push((norm, e.freq));
        entry.sort_by(|a, b| b.1.total_cmp(&a.1));
        entry.truncate(2);
    }
    let competitor = |input: &str, expected_norm: &str| -> Option<String> {
        by_skel
            .get(&skeleton(&normalize(input)))?
            .iter()
            .map(|(f, _)| f)
            .find(|f| *f != expected_norm)
            .cloned()
    };

    // Real usage reuses a just-typed word far more often than it immediately
    // switches to a same-skeleton competitor, so the operating point weights
    // positive over adversarial (assume ~4:1) rather than treating them equally
    // (min()) — which would always pick w=0 and discard the whole signal.
    const REUSE_SHARE: f64 = 0.8;
    const GRID: [f32; 8] = [0.0, 0.5, 1.0, 1.5, 2.0, 3.0, 4.0, 6.0];
    println!(
        "recency sweep over {} cases (note expected vs note competitor)",
        cases.len()
    );
    println!("  w_recency   positive top-1   adversarial top-1   blend(4:1)");
    let mut best: Option<(f32, f64)> = None;
    for &w in &GRID {
        engine.set_weights(Weights {
            recency: w,
            ..Weights::default()
        });

        let (mut pos, mut pos_n) = (0u32, 0u32);
        let (mut adv, mut adv_n) = (0u32, 0u32);
        for case in cases {
            // Normalized only for the competitor lookup (the pool is normalized);
            // the hit check compares *surface* forms, like the rest of the bench.
            let expected_norm = normalize(&case.expected);

            engine.reset_session();
            engine.note_word(&case.expected);
            if top1_matches(engine, &case.input, &case.context, &case.expected) {
                pos += 1;
            }
            pos_n += 1;

            if let Some(comp) = competitor(&case.input, &expected_norm) {
                engine.reset_session();
                engine.note_word(&comp);
                if top1_matches(engine, &case.input, &case.context, &case.expected) {
                    adv += 1;
                }
                adv_n += 1;
            }
        }
        let pos_rate = 100.0 * f64::from(pos) / f64::from(pos_n.max(1));
        let adv_rate = 100.0 * f64::from(adv) / f64::from(adv_n.max(1));
        let blend = REUSE_SHARE * pos_rate + (1.0 - REUSE_SHARE) * adv_rate;
        println!("  {w:>7.1}     {pos_rate:>10.1}%       {adv_rate:>12.1}%     {blend:>7.1}%");
        if best.is_none_or(|(_, b)| blend > b) {
            best = Some((w, blend));
        }
    }
    engine.reset_session();
    if let Some((w, b)) = best {
        println!(
            "\nUnder an assumed 4:1 reuse:switch mix, the blend peaks at w_recency = {w:.1} \
             (blended top-1 {b:.1}%).\nThe default 1.0 is a conservative point on this curve; \
             raise it toward the knee only if real logs show reuse dominates."
        );
    }
    ExitCode::SUCCESS
}

/// True when the engine's top-1 form (normalized) equals `expected_norm`.
fn top1_matches(engine: &Engine, input: &str, context: &Context, expected_norm: &str) -> bool {
    engine
        .suggest(input, context, 1)
        .first()
        .is_some_and(|s| normalize(&s.form) == expected_norm)
}

fn print_eval(label: &str, e: &Eval) {
    let n = f64::from(e.n.max(1));
    println!(
        "{label}: top-1 {:.1}%  top-3 {:.1}%  MRR {:.3}  obj {:.4}",
        100.0 * f64::from(e.top1) / n,
        100.0 * f64::from(e.top3) / n,
        e.rr / n,
        e.objective(),
    );
}

fn load_engine(
    lexicon_path: &Option<String>,
    lm_path: &Option<String>,
    shortcuts_path: &Option<String>,
) -> Result<Engine, String> {
    let lexicon = match lexicon_path {
        Some(p) => Lexicon::from_tsv_str(
            &std::fs::read_to_string(p).map_err(|e| format!("cannot read {p}: {e}"))?,
        )
        .map_err(|e| e.to_string())?,
        None => Lexicon::demo(),
    };
    let mut engine = Engine::new(lexicon);
    if let Some(p) = lm_path {
        let tsv = std::fs::read_to_string(p).map_err(|e| format!("cannot read {p}: {e}"))?;
        engine.set_context_model(Box::new(
            BigramModel::from_tsv_str(&tsv).map_err(|e| e.to_string())?,
        ));
    }
    if let Some(p) = shortcuts_path {
        let tsv = std::fs::read_to_string(p).map_err(|e| format!("cannot read {p}: {e}"))?;
        engine.set_shortcuts(Shortcuts::from_tsv_str(&tsv).map_err(|e| e.to_string())?);
    }
    Ok(engine)
}

fn fail(message: &str) -> ExitCode {
    eprintln!("error: {message}");
    ExitCode::FAILURE
}
