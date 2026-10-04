use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::time::Instant;

use russian_draughts_classic::{
    eval_param_name, read_tune_file, term_divisor, DataSplit, EvalParams, Evaluator, Game,
    PdnOutcome, Position, BASE_EVAL_TERM_COUNT, ENGINE_VERSION, EVAL_PARAM_COUNT, EVAL_TERM_COUNT,
    KING_PSQT_OFFSET, MAN_PSQT_OFFSET, PHASE_SCALE,
};

#[derive(Clone)]
struct Sample {
    base: [i16; BASE_EVAL_TERM_COUNT],
    position: Position,
    phase: u16,
    y: f64,
    split: DataSplit,
    weight: f64,
    flags: u8,
}

struct Options {
    corpus: PathBuf,
    base: PathBuf,
    output: Option<PathBuf>,
    epochs: usize,
    batch: usize,
    lr: f64,
    scale: Option<f64>,
    l2: f64,
    seed: u64,
    patience: usize,
    evaluate_test: bool,
    polish_passes: usize,
    polish_samples: usize,
}

fn main() {
    if let Err(err) = real_main() {
        eprintln!("classic_tune_eval: {err}");
        std::process::exit(2);
    }
}

fn real_main() -> Result<(), Box<dyn std::error::Error>> {
    let o = parse_args()?;
    if let Some(output) = o.output.as_ref() {
        create_parent(output)?;
    }
    let base_text = fs::read_to_string(&o.base)?;
    let base = EvalParams::parse_kv(&base_text).map_err(|e| format!("base eval: {e}"))?;
    let mut samples = Vec::new();
    let mut split_counts = [0usize; 3];
    if o.corpus.is_dir() {
        if o.evaluate_test {
            return Err("Corpus V2 FINAL cannot be opened by the training command".into());
        }
        load_v3_role(
            &o.corpus.join("train.rdtune3"),
            DataSplit::Train,
            0,
            &mut samples,
        )?;
        load_v3_role(
            &o.corpus.join("dev.rdtune3"),
            DataSplit::Validation,
            1,
            &mut samples,
        )?;
        for sample in &samples {
            split_counts[sample.split as usize] += 1;
        }
        // Uniform sample batches estimate the weighted objective without a
        // batch-dependent denominator. Mean train weight is normalized to one.
        let sum: f64 = samples
            .iter()
            .filter(|s| s.split == DataSplit::Train)
            .map(|s| s.weight)
            .sum();
        if sum > 0.0 {
            let factor = split_counts[0] as f64 / sum;
            for sample in samples.iter_mut().filter(|s| s.split == DataSplit::Train) {
                sample.weight *= factor;
            }
        }
        println!("RDTUNE3 weighted corpus training; final/reference/arena excluded");
    } else {
        let (info, records) = read_tune_file(&o.corpus)?;
        println!("Tapered eval autotuner v{ENGINE_VERSION}");
        println!("corpus  : {}", o.corpus.display());
        println!("records : {}", info.records);
        println!("terms   : {EVAL_TERM_COUNT} (parameters MG+EG: {EVAL_PARAM_COUNT})");

        let evaluator = Evaluator::default();
        for record in records {
            let y = match record.outcome {
                PdnOutcome::WhiteWin => 1.0,
                PdnOutcome::Draw => 0.5,
                PdnOutcome::BlackWin => 0.0,
                PdnOutcome::Unknown => continue,
            };
            let game = Game::new(record.position);
            let features = evaluator.base_features(&game);
            let mut base_features = [0i16; BASE_EVAL_TERM_COUNT];
            base_features.copy_from_slice(&features.values[..BASE_EVAL_TERM_COUNT]);
            split_counts[record.split as usize] += 1;
            samples.push(Sample {
                base: base_features,
                position: record.position,
                phase: features.phase,
                y,
                split: record.split,
                weight: 1.0,
                flags: 0,
            });
        }
    }
    println!(
        "split   : train={} validation={} test={}",
        split_counts[0], split_counts[1], split_counts[2]
    );
    if split_counts[0] == 0 || split_counts[1] == 0 {
        return Err("corpus needs non-empty train and validation splits".into());
    }

    let base_vec = base.to_vector();
    let mut w = [0.0f64; EVAL_PARAM_COUNT];
    for (dst, &src) in w.iter_mut().zip(base_vec.iter()) {
        *dst = src as f64;
    }

    let baseline_scores: Vec<f64> = samples
        .iter()
        .map(|s| exact_serialized_score(&base_vec, s) as f64)
        .collect();
    let scale = o
        .scale
        .unwrap_or_else(|| fit_scale(&samples, &baseline_scores));
    println!("logistic scale: {scale:.6}");
    let base_train = loss_serialized(&samples, base, scale, DataSplit::Train);
    let base_val = loss_serialized(&samples, base, scale, DataSplit::Validation);
    println!("baseline loss: train={base_train:.8} val={base_val:.8}");
    if o.evaluate_test {
        if split_counts[2] == 0 {
            return Err("corpus needs a non-empty test split for --evaluate-test".into());
        }
        let test = loss_serialized(&samples, base, scale, DataSplit::Test);
        println!("FROZEN HOLDOUT EVALUATION");
        println!("eval      : {}", o.base.display());
        println!("scale     : {scale:.6} (fit on train only)");
        println!("loss train: {base_train:.8}");
        println!("loss val  : {base_val:.8}");
        println!("loss test : {test:.8}");
        return Ok(());
    }

    let output = o
        .output
        .as_ref()
        .ok_or("--output is required unless --evaluate-test is used")?;

    let mut m = [0.0f64; EVAL_PARAM_COUNT];
    let mut v = [0.0f64; EVAL_PARAM_COUNT];
    let mut step = 0u64;
    let mut best_float = w;
    let mut best_params = base;
    let mut best_serialized_val = base_val;
    let mut best_float_val_seen = loss(&samples, &w, scale, DataSplit::Validation);
    let mut best_epoch = 0usize;
    let mut train_indices: Vec<usize> = samples
        .iter()
        .enumerate()
        .filter_map(|(i, s)| (s.split == DataSplit::Train).then_some(i))
        .collect();

    let mut epochs_without_improvement = 0usize;
    for epoch in 1..=o.epochs {
        let mut progress = Instant::now();
        eprintln!("Обучение: эпоха {epoch}/{}", o.epochs);
        shuffle_indices(&mut train_indices, o.seed ^ epoch as u64);
        let mut grad = [0.0f64; EVAL_PARAM_COUNT];
        let mut in_batch = 0usize;
        for (done, &sample_index) in train_indices.iter().enumerate() {
            if progress.elapsed().as_secs() >= 5 {
                eprintln!(
                    "Эпоха {epoch}: {done}/{} ({:.1}%)",
                    train_indices.len(),
                    100.0 * done as f64 / train_indices.len() as f64
                );
                progress = Instant::now();
            }
            let sample = &samples[sample_index];
            let z = dot_sample(&w, sample) / scale;
            let pred = sigmoid(z);
            let common = sample.weight * (pred - sample.y) / scale;
            add_gradient(&mut grad, sample, common);
            in_batch += 1;
            if in_batch == o.batch {
                apply_adam(
                    &mut w, &mut m, &mut v, &mut step, &grad, in_batch, o.lr, o.l2, &base_vec,
                );
                grad.fill(0.0);
                in_batch = 0;
            }
        }
        if in_batch != 0 {
            apply_adam(
                &mut w, &mut m, &mut v, &mut step, &grad, in_batch, o.lr, o.l2, &base_vec,
            );
        }
        enforce_bounds(&mut w);
        project_psqt(&mut w);

        let float_train = loss(&samples, &w, scale, DataSplit::Train);
        let float_val = loss(&samples, &w, scale, DataSplit::Validation);
        let serialized = rounded_params(&w);
        let serialized_val = loss_serialized(&samples, serialized, scale, DataSplit::Validation);
        println!(
            "epoch {epoch:3}: float_train={float_train:.8} float_val={float_val:.8} serialized_val={serialized_val:.8}"
        );
        let serialized_improved = serialized_val < best_serialized_val;
        if serialized_improved {
            best_serialized_val = serialized_val;
            best_float = w;
            best_params = serialized;
            best_epoch = epoch;
            fs::write(output, best_params.to_kv_string())?;
        }

        // Quantized i32 candidates can sit on a plateau for several epochs even
        // while the continuous optimizer is still moving toward a better integer
        // point. Stop only when neither representation improves.
        let float_improved = float_val < best_float_val_seen;
        if float_improved {
            best_float_val_seen = float_val;
        }
        if serialized_improved || float_improved {
            epochs_without_improvement = 0;
        } else {
            epochs_without_improvement += 1;
            if o.patience != 0 && epochs_without_improvement >= o.patience {
                println!(
                    "early stop: no float or serialized validation improvement for {} epochs",
                    o.patience
                );
                break;
            }
        }
    }

    let continuous_params = best_params;
    if o.polish_passes > 0 {
        best_params = integer_polish(&samples, best_params, &base_vec, scale, &o);
    }
    let best_float_train = loss(&samples, &best_float, scale, DataSplit::Train);
    let best_float_val = loss(&samples, &best_float, scale, DataSplit::Validation);
    let tuned_train = loss_serialized(&samples, best_params, scale, DataSplit::Train);
    let tuned_val = loss_serialized(&samples, best_params, scale, DataSplit::Validation);
    fs::write(output, best_params.to_kv_string())?;
    fs::write(
        output.with_extension("continuous.txt"),
        continuous_params.to_kv_string(),
    )?;
    write_diagnostics(
        &samples,
        base,
        best_params,
        scale,
        &output.with_extension("diagnostics.csv"),
    )?;

    println!("best epoch: {best_epoch}");
    println!("float loss at selected epoch: train={best_float_train:.8} val={best_float_val:.8}");
    println!("engine-exact serialized loss: train={tuned_train:.8} val={tuned_val:.8}");
    println!("output    : {}", output.display());
    println!("parameters changed (base -> tuned):");
    let tuned_vec = best_params.to_vector();
    let mut changed = 0usize;
    for (i, (&before, &after)) in base_vec.iter().zip(tuned_vec.iter()).enumerate() {
        if before != after {
            println!("  {:28} {:>6} -> {:>6}", eval_param_name(i), before, after);
            changed += 1;
        }
    }
    println!("changed parameters: {changed}/{EVAL_PARAM_COUNT}");
    println!("parameter_report: name,base,float,serialized,delta");
    for i in 0..EVAL_PARAM_COUNT {
        println!(
            "{},{},{:.9},{},{}",
            eval_param_name(i),
            base_vec[i],
            best_float[i],
            tuned_vec[i],
            tuned_vec[i] - base_vec[i]
        );
    }
    Ok(())
}

fn penalty(params: EvalParams, base: &[i32; EVAL_PARAM_COUNT], l2: f64) -> f64 {
    params
        .to_vector()
        .iter()
        .zip(base)
        .map(|(&w, &b)| 0.5 * l2 * ((w - b) as f64).powi(2))
        .sum()
}
fn perturb(params: EvalParams, index: usize, delta: i32) -> EvalParams {
    let mut v = params.to_vector().map(|x| x as f64);
    v[index] += delta as f64;
    enforce_bounds(&mut v);
    project_psqt(&mut v);
    rounded_params(&v)
}
fn integer_polish(
    samples: &[Sample],
    initial: EvalParams,
    base: &[i32; EVAL_PARAM_COUNT],
    scale: f64,
    o: &Options,
) -> EvalParams {
    let mut indices: Vec<_> = samples
        .iter()
        .enumerate()
        .filter_map(|(i, s)| (s.split == DataSplit::Train).then_some(i))
        .collect();
    shuffle_indices(&mut indices, o.seed ^ 0x504f4c495348);
    indices.truncate(o.polish_samples.min(indices.len()));
    let subset: Vec<_> = indices.iter().map(|&i| samples[i].clone()).collect();
    let mut current = initial;
    let mut selected = initial;
    let mut best_dev = loss_serialized(samples, selected, scale, DataSplit::Validation);
    let mut full_loss = loss_serialized(samples, current, scale, DataSplit::Train);
    let mut full_objective = full_loss + penalty(current, base, o.l2);
    for pass in 1..=o.polish_passes {
        let mut proposal = current;
        let mut objective = loss_serialized(&subset, proposal, scale, DataSplit::Train)
            + penalty(proposal, base, o.l2);
        let mut last = Instant::now();
        eprintln!(
            "Integer polish {pass}/{}: TRAIN subset={}",
            o.polish_passes,
            subset.len()
        );
        for i in 0..EVAL_PARAM_COUNT {
            if locked(i) {
                continue;
            }
            let origin = proposal;
            for delta in [-2, -1, 1, 2] {
                let candidate = perturb(origin, i, delta);
                if candidate.to_vector() == origin.to_vector() {
                    continue;
                }
                let score = loss_serialized(&subset, candidate, scale, DataSplit::Train)
                    + penalty(candidate, base, o.l2);
                if score + 1e-12 < objective {
                    objective = score;
                    proposal = candidate;
                }
            }
            if last.elapsed().as_secs() >= 5 {
                eprintln!("Integer polish {pass}: {}/{EVAL_PARAM_COUNT}", i + 1);
                last = Instant::now();
            }
        }
        // Subset only proposes. Full TRAIN accepts/rejects the entire sweep.
        let proposed_loss = loss_serialized(samples, proposal, scale, DataSplit::Train);
        let proposed_objective = proposed_loss + penalty(proposal, base, o.l2);
        if proposed_loss + 1e-12 >= full_loss || proposed_objective + 1e-12 >= full_objective {
            println!("polish pass {pass}: no improvement in both full TRAIN loss and regularized objective; stopping");
            break;
        }
        current = proposal;
        full_loss = proposed_loss;
        full_objective = proposed_objective;
        let dev = loss_serialized(samples, current, scale, DataSplit::Validation);
        println!("polish pass {pass}: train={full_loss:.8} val={dev:.8}");
        if dev < best_dev {
            selected = current;
            best_dev = dev;
        } else {
            println!("polish: DEV did not improve; keep previous candidate and stop");
            break;
        }
    }
    selected
}

#[derive(Default)]
struct Diagnostic {
    n: usize,
    weight: f64,
    base_loss: f64,
    tuned_loss: f64,
    prediction: f64,
    outcome: f64,
    brier: f64,
}
fn write_diagnostics(
    samples: &[Sample],
    base: EvalParams,
    tuned: EvalParams,
    scale: f64,
    path: &std::path::Path,
) -> std::io::Result<()> {
    use std::collections::BTreeMap;
    let mut groups: BTreeMap<(u8, String), Diagnostic> = BTreeMap::new();
    let bv = base.to_vector();
    let tv = tuned.to_vector();
    let mut progress = Instant::now();
    for (i, s) in samples
        .iter()
        .enumerate()
        .filter(|(_, s)| s.split != DataSplit::Test)
    {
        let bp = sigmoid(exact_serialized_score(&bv, s) as f64 / scale).clamp(1e-12, 1.0 - 1e-12);
        let tp = sigmoid(exact_serialized_score(&tv, s) as f64 / scale).clamp(1e-12, 1.0 - 1e-12);
        let pieces = s.position.occupied().count_ones();
        let kings = s.position.kings.count_ones();
        let material = if kings == 0 {
            "men"
        } else if kings * 2 >= pieces {
            "king-heavy"
        } else {
            "mixed"
        };
        let phase = if s.phase >= 192 {
            "high"
        } else if s.phase >= 64 {
            "middle"
        } else {
            "low"
        };
        for group in [
            "all".to_string(),
            format!("material/{material}"),
            format!("phase/{phase}"),
            format!(
                "tactical/{}",
                if s.flags & 1 != 0 { "capture" } else { "quiet" }
            ),
            format!("promotion/{}", s.flags & 2 != 0),
            format!("calibration/{}", (tp * 10.0) as usize),
        ] {
            let d = groups.entry((s.split as u8, group)).or_default();
            d.n += 1;
            d.weight += s.weight;
            d.base_loss += -s.weight * (s.y * bp.ln() + (1.0 - s.y) * (1.0 - bp).ln());
            d.tuned_loss += -s.weight * (s.y * tp.ln() + (1.0 - s.y) * (1.0 - tp).ln());
            d.prediction += s.weight * tp;
            d.outcome += s.weight * s.y;
            d.brier += s.weight * (tp - s.y).powi(2);
        }
        if progress.elapsed().as_secs() >= 5 {
            eprintln!("Диагностика: {i}/{}", samples.len());
            progress = Instant::now();
        }
    }
    let mut file = std::io::BufWriter::new(fs::File::create(path)?);
    writeln!(file,"split,class,records,weight,base_loss,candidate_loss,delta,mean_prediction,mean_result,brier")?;
    for ((split, group), d) in groups {
        writeln!(
            file,
            "{},{},{},{:.6},{:.9},{:.9},{:.9},{:.9},{:.9},{:.9}",
            if split == 0 { "train" } else { "dev" },
            group,
            d.n,
            d.weight,
            d.base_loss / d.weight,
            d.tuned_loss / d.weight,
            (d.tuned_loss - d.base_loss) / d.weight,
            d.prediction / d.weight,
            d.outcome / d.weight,
            d.brier / d.weight
        )?;
    }
    file.flush()
}

fn parse_args() -> Result<Options, Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let help = args.iter().any(|a| a == "-h" || a == "--help");
    if help || args.is_empty() {
        println!(
            "usage: classic_tune_eval --corpus FILE_OR_V2_DIRECTORY --base FILE --output FILE \
  [--epochs 30] [--batch 4096] [--lr 0.15] [--scale auto|N] [--l2 0.00002] [--seed N] [--patience 8]
       classic_tune_eval --corpus FILE --base FROZEN_EVAL --evaluate-test [--scale auto|N]"
        );
        std::process::exit(if help { 0 } else { 2 });
    }
    let mut corpus = None;
    let mut base = None;
    let mut output = None;
    let mut epochs = 30usize;
    let mut batch = 4096usize;
    let mut lr = 0.15f64;
    let mut scale = None;
    let mut l2 = 2e-5f64;
    let mut seed = 0x2026_0925_71E0_0002u64;
    let mut patience = 8usize;
    let mut evaluate_test = false;
    let mut polish_passes = 0usize;
    let mut polish_samples = 8192usize;
    let mut i = 0;
    while i < args.len() {
        let key = &args[i];
        let value = |i: usize| -> Result<&str, Box<dyn std::error::Error>> {
            args.get(i + 1)
                .map(String::as_str)
                .ok_or_else(|| format!("{key} requires value").into())
        };
        match key.as_str() {
            "--polish-passes" => {
                polish_passes = value(i)?.parse()?;
                i += 2;
            }
            "--polish-samples" => {
                polish_samples = value(i)?.parse()?;
                i += 2;
            }
            "--corpus" => {
                corpus = Some(PathBuf::from(value(i)?));
                i += 2;
            }
            "--base" => {
                base = Some(PathBuf::from(value(i)?));
                i += 2;
            }
            "--output" => {
                output = Some(PathBuf::from(value(i)?));
                i += 2;
            }
            "--epochs" => {
                epochs = value(i)?.parse()?;
                i += 2;
            }
            "--batch" => {
                batch = value(i)?.parse()?;
                i += 2;
            }
            "--lr" => {
                lr = value(i)?.parse()?;
                i += 2;
            }
            "--l2" => {
                l2 = value(i)?.parse()?;
                i += 2;
            }
            "--seed" => {
                seed = parse_u64(value(i)?)?;
                i += 2;
            }
            "--patience" => {
                patience = value(i)?.parse()?;
                i += 2;
            }
            "--scale" => {
                let text = value(i)?;
                scale = if text == "auto" {
                    None
                } else {
                    Some(text.parse()?)
                };
                i += 2;
            }
            "--evaluate-test" => {
                evaluate_test = true;
                i += 1;
            }
            _ => return Err(format!("unknown option {key}").into()),
        }
    }
    if polish_samples == 0
        || !lr.is_finite()
        || !l2.is_finite()
        || scale.is_some_and(|v: f64| !v.is_finite() || v <= 0.0)
    {
        return Err("invalid finite settings".into());
    }
    if !evaluate_test && (epochs == 0 || batch == 0 || lr <= 0.0 || l2 < 0.0) {
        return Err("invalid tuning hyperparameters".into());
    }
    if !evaluate_test && output.is_none() {
        return Err("--output is required for tuning".into());
    }
    Ok(Options {
        corpus: corpus.ok_or("--corpus is required")?,
        base: base.ok_or("--base is required")?,
        output,
        epochs,
        batch,
        lr,
        scale,
        l2,
        seed,
        patience,
        evaluate_test,
        polish_passes,
        polish_samples,
    })
}

#[inline]
fn phase_factors(sample: &Sample) -> (f64, f64) {
    let mg = sample.phase as f64 / PHASE_SCALE as f64;
    (mg, 1.0 - mg)
}

#[inline]
fn dot_sample(w: &[f64; EVAL_PARAM_COUNT], sample: &Sample) -> f64 {
    let (mgf, egf) = phase_factors(sample);
    let mut sum = 0.0f64;
    for term in 0..BASE_EVAL_TERM_COUNT {
        let f = sample.base[term] as f64 / term_divisor(term) as f64;
        if f != 0.0 {
            sum += f * (w[2 * term] * mgf + w[2 * term + 1] * egf);
        }
    }
    let pos = sample.position;
    let white_kings = pos.white & pos.kings;
    let black_kings = pos.black & pos.kings;
    add_psqt_dot(
        &mut sum,
        w,
        pos.white & !pos.kings,
        MAN_PSQT_OFFSET,
        1.0,
        mgf,
        egf,
        false,
    );
    add_psqt_dot(
        &mut sum,
        w,
        pos.black & !pos.kings,
        MAN_PSQT_OFFSET,
        -1.0,
        mgf,
        egf,
        true,
    );
    add_psqt_dot(
        &mut sum,
        w,
        white_kings,
        KING_PSQT_OFFSET,
        1.0,
        mgf,
        egf,
        false,
    );
    add_psqt_dot(
        &mut sum,
        w,
        black_kings,
        KING_PSQT_OFFSET,
        -1.0,
        mgf,
        egf,
        true,
    );
    sum
}

// Numerical hot helper: keeping scalar phase/sign arguments explicit makes the
// MG/EG arithmetic auditable; bundling them only to satisfy Clippy would not
// simplify the call sites.
#[allow(clippy::too_many_arguments)]
#[inline]
fn add_psqt_dot(
    sum: &mut f64,
    w: &[f64; EVAL_PARAM_COUNT],
    mut pieces: u32,
    offset: usize,
    sign: f64,
    mgf: f64,
    egf: f64,
    rotate: bool,
) {
    while pieces != 0 {
        let sq = pop_lsb(&mut pieces) as usize;
        let bucket = if rotate { 31 - sq } else { sq };
        let term = offset + bucket;
        *sum += sign * (w[2 * term] * mgf + w[2 * term + 1] * egf);
    }
}

#[inline]
fn add_gradient(grad: &mut [f64; EVAL_PARAM_COUNT], sample: &Sample, common: f64) {
    let (mgf, egf) = phase_factors(sample);
    for term in 0..BASE_EVAL_TERM_COUNT {
        let f = sample.base[term] as f64 / term_divisor(term) as f64;
        if f != 0.0 {
            grad[2 * term] += common * f * mgf;
            grad[2 * term + 1] += common * f * egf;
        }
    }
    let pos = sample.position;
    add_psqt_gradient(
        grad,
        pos.white & !pos.kings,
        MAN_PSQT_OFFSET,
        common,
        mgf,
        egf,
        false,
    );
    add_psqt_gradient(
        grad,
        pos.black & !pos.kings,
        MAN_PSQT_OFFSET,
        -common,
        mgf,
        egf,
        true,
    );
    add_psqt_gradient(
        grad,
        pos.white & pos.kings,
        KING_PSQT_OFFSET,
        common,
        mgf,
        egf,
        false,
    );
    add_psqt_gradient(
        grad,
        pos.black & pos.kings,
        KING_PSQT_OFFSET,
        -common,
        mgf,
        egf,
        true,
    );
}

#[inline]
fn add_psqt_gradient(
    grad: &mut [f64; EVAL_PARAM_COUNT],
    mut pieces: u32,
    offset: usize,
    common: f64,
    mgf: f64,
    egf: f64,
    rotate: bool,
) {
    while pieces != 0 {
        let sq = pop_lsb(&mut pieces) as usize;
        let bucket = if rotate { 31 - sq } else { sq };
        let term = offset + bucket;
        grad[2 * term] += common * mgf;
        grad[2 * term + 1] += common * egf;
    }
}

#[inline]
fn exact_serialized_score(w: &[i32; EVAL_PARAM_COUNT], sample: &Sample) -> i32 {
    let phase = sample.phase as i64;
    let eg_phase = PHASE_SCALE as i64 - phase;
    let mut mg = 0i64;
    let mut eg = 0i64;

    for term in 0..BASE_EVAL_TERM_COUNT {
        let feature = sample.base[term] as i64;
        if feature == 0 {
            continue;
        }
        let divisor = term_divisor(term) as i64;
        mg += feature * w[2 * term] as i64 / divisor;
        eg += feature * w[2 * term + 1] as i64 / divisor;
    }

    let pos = sample.position;
    add_psqt_exact(
        &mut mg,
        &mut eg,
        w,
        pos.white & !pos.kings,
        MAN_PSQT_OFFSET,
        1,
        false,
    );
    add_psqt_exact(
        &mut mg,
        &mut eg,
        w,
        pos.black & !pos.kings,
        MAN_PSQT_OFFSET,
        -1,
        true,
    );
    add_psqt_exact(
        &mut mg,
        &mut eg,
        w,
        pos.white & pos.kings,
        KING_PSQT_OFFSET,
        1,
        false,
    );
    add_psqt_exact(
        &mut mg,
        &mut eg,
        w,
        pos.black & pos.kings,
        KING_PSQT_OFFSET,
        -1,
        true,
    );

    div_round_signed(mg * phase + eg * eg_phase, PHASE_SCALE as i64) as i32
}

#[inline]
fn add_psqt_exact(
    mg: &mut i64,
    eg: &mut i64,
    w: &[i32; EVAL_PARAM_COUNT],
    mut pieces: u32,
    offset: usize,
    sign: i64,
    rotate: bool,
) {
    while pieces != 0 {
        let sq = pop_lsb(&mut pieces) as usize;
        let bucket = if rotate { 31 - sq } else { sq };
        let term = offset + bucket;
        *mg += sign * w[2 * term] as i64;
        *eg += sign * w[2 * term + 1] as i64;
    }
}

#[inline]
fn div_round_signed(value: i64, divisor: i64) -> i64 {
    if value >= 0 {
        (value + divisor / 2) / divisor
    } else {
        (value - divisor / 2) / divisor
    }
}

fn loss_serialized(samples: &[Sample], params: EvalParams, scale: f64, split: DataSplit) -> f64 {
    let w = params.to_vector();
    let mut sum = 0.0;
    let mut n = 0.0f64;
    for sample in samples.iter().filter(|sample| sample.split == split) {
        let score = exact_serialized_score(&w, sample) as f64;
        let p = sigmoid(score / scale).clamp(1e-12, 1.0 - 1e-12);
        sum += -sample.weight * (sample.y * p.ln() + (1.0 - sample.y) * (1.0 - p).ln());
        n += sample.weight;
    }
    if n == 0.0 {
        f64::NAN
    } else {
        sum / n
    }
}

fn fit_scale(samples: &[Sample], baseline_scores: &[f64]) -> f64 {
    let mut lo = 20.0f64.ln();
    let mut hi = 1000.0f64.ln();
    let phi = (5.0f64.sqrt() - 1.0) / 2.0;
    let mut x1 = hi - phi * (hi - lo);
    let mut x2 = lo + phi * (hi - lo);
    let mut f1 = loss_from_scores(samples, baseline_scores, x1.exp(), DataSplit::Train);
    let mut f2 = loss_from_scores(samples, baseline_scores, x2.exp(), DataSplit::Train);
    for _ in 0..40 {
        if f1 < f2 {
            hi = x2;
            x2 = x1;
            f2 = f1;
            x1 = hi - phi * (hi - lo);
            f1 = loss_from_scores(samples, baseline_scores, x1.exp(), DataSplit::Train);
        } else {
            lo = x1;
            x1 = x2;
            f1 = f2;
            x2 = lo + phi * (hi - lo);
            f2 = loss_from_scores(samples, baseline_scores, x2.exp(), DataSplit::Train);
        }
    }
    ((lo + hi) * 0.5).exp()
}

fn loss_from_scores(samples: &[Sample], scores: &[f64], scale: f64, split: DataSplit) -> f64 {
    let mut sum = 0.0;
    let mut n = 0.0f64;
    for (s, &score) in samples.iter().zip(scores).filter(|(s, _)| s.split == split) {
        let p = sigmoid(score / scale).clamp(1e-12, 1.0 - 1e-12);
        sum += -s.weight * (s.y * p.ln() + (1.0 - s.y) * (1.0 - p).ln());
        n += s.weight;
    }
    if n == 0.0 {
        f64::NAN
    } else {
        sum / n
    }
}

fn loss(samples: &[Sample], w: &[f64; EVAL_PARAM_COUNT], scale: f64, split: DataSplit) -> f64 {
    let mut sum = 0.0;
    let mut n = 0.0f64;
    for s in samples.iter().filter(|s| s.split == split) {
        let p = sigmoid(dot_sample(w, s) / scale).clamp(1e-12, 1.0 - 1e-12);
        sum += -s.weight * (s.y * p.ln() + (1.0 - s.y) * (1.0 - p).ln());
        n += s.weight;
    }
    if n == 0.0 {
        f64::NAN
    } else {
        sum / n
    }
}

#[allow(clippy::too_many_arguments)]
fn apply_adam(
    w: &mut [f64; EVAL_PARAM_COUNT],
    m: &mut [f64; EVAL_PARAM_COUNT],
    v: &mut [f64; EVAL_PARAM_COUNT],
    step: &mut u64,
    grad: &[f64; EVAL_PARAM_COUNT],
    batch: usize,
    lr: f64,
    l2: f64,
    base: &[i32; EVAL_PARAM_COUNT],
) {
    *step += 1;
    let beta1 = 0.9f64;
    let beta2 = 0.999f64;
    let t = *step as i32;
    for i in 0..EVAL_PARAM_COUNT {
        if locked(i) {
            w[i] = base[i] as f64;
            continue;
        }
        let g = grad[i] / batch as f64 + l2 * (w[i] - base[i] as f64);
        m[i] = beta1 * m[i] + (1.0 - beta1) * g;
        v[i] = beta2 * v[i] + (1.0 - beta2) * g * g;
        let mh = m[i] / (1.0 - beta1.powi(t));
        let vh = v[i] / (1.0 - beta2.powi(t));
        w[i] -= lr * mh / (vh.sqrt() + 1e-8);
    }
    enforce_bounds(w);
    project_psqt(w);
}

fn locked(index: usize) -> bool {
    let term = index / 2;
    // Keep centi-man scale identifiable and impossible advancement ranks fixed.
    matches!(term, 0 | 2 | 9)
}

fn project_psqt(w: &mut [f64; EVAL_PARAM_COUNT]) {
    // Project directly onto the intersection of the zero-sum identifiability
    // hyperplane and the PSQT box bounds. A simple subtract-mean followed by
    // clamp can break zero-sum again when a value touches a bound.
    for phase in 0..2 {
        for rank in 0..8 {
            project_zero_sum_group(w, MAN_PSQT_OFFSET + rank * 4, 4, phase);
        }
        project_zero_sum_group(w, KING_PSQT_OFFSET, 32, phase);
    }
}

fn project_zero_sum_group(
    w: &mut [f64; EVAL_PARAM_COUNT],
    term_start: usize,
    term_count: usize,
    phase: usize,
) {
    const LO: f64 = -120.0;
    const HI: f64 = 120.0;

    // x_i(lambda) = clamp(w_i - lambda, LO, HI). The sum is continuous and
    // monotonically decreasing, so bisection finds the bounded zero-sum
    // projection without heap allocation.
    let mut lambda_lo = f64::INFINITY;
    let mut lambda_hi = f64::NEG_INFINITY;
    for offset in 0..term_count {
        let index = 2 * (term_start + offset) + phase;
        lambda_lo = lambda_lo.min(w[index] - HI);
        lambda_hi = lambda_hi.max(w[index] - LO);
    }

    for _ in 0..64 {
        let lambda = (lambda_lo + lambda_hi) * 0.5;
        let mut sum = 0.0;
        for offset in 0..term_count {
            let index = 2 * (term_start + offset) + phase;
            sum += (w[index] - lambda).clamp(LO, HI);
        }
        if sum > 0.0 {
            lambda_lo = lambda;
        } else {
            lambda_hi = lambda;
        }
    }

    let lambda = (lambda_lo + lambda_hi) * 0.5;
    for offset in 0..term_count {
        let index = 2 * (term_start + offset) + phase;
        w[index] = (w[index] - lambda).clamp(LO, HI);
    }
}

fn enforce_bounds(w: &mut [f64; EVAL_PARAM_COUNT]) {
    for phase in 0..2 {
        w[phase] = 100.0; // man value
        w[2 * 2 + phase] = 0.0;
        w[2 * 9 + phase] = 0.0;
        w[2 + phase] = w[2 + phase].clamp(120.0, 650.0);
    }
    for (i, value) in w.iter_mut().enumerate() {
        if locked(i) || i / 2 == 1 {
            continue;
        }
        let term = i / 2;
        let (lo, hi) = if term >= MAN_PSQT_OFFSET {
            (-120.0, 120.0)
        } else if term == 19 {
            (-60.0, 60.0)
        } else if (3..=8).contains(&term) {
            (-60.0, 220.0)
        } else {
            (-160.0, 200.0)
        };
        *value = (*value).clamp(lo, hi);
    }
}

fn rounded_params(w: &[f64; EVAL_PARAM_COUNT]) -> EvalParams {
    let mut v = [0i32; EVAL_PARAM_COUNT];
    for (dst, &src) in v.iter_mut().zip(w.iter()) {
        *dst = src.round() as i32;
    }

    // Keep the exact integer representation identifiable as well. Float PSQT
    // normalization alone is insufficient because independent rounding can make
    // a rank/global sum non-zero again.
    for phase in 0..2 {
        for rank in 0..8 {
            rebalance_integer_group(&mut v, w, MAN_PSQT_OFFSET + rank * 4, 4, phase);
        }
        rebalance_integer_group(&mut v, w, KING_PSQT_OFFSET, 32, phase);
    }

    // Fixed scale/impossible ranks are enforced after integer rebalancing.
    v[0] = 100;
    v[1] = 100;
    for phase in 0..2 {
        v[2 * 2 + phase] = 0;
        v[2 * 9 + phase] = 0;
    }
    EvalParams::from_vector(v)
}

fn rebalance_integer_group(
    v: &mut [i32; EVAL_PARAM_COUNT],
    w: &[f64; EVAL_PARAM_COUNT],
    term_start: usize,
    term_count: usize,
    phase: usize,
) {
    let mut residual: i32 = (0..term_count)
        .map(|offset| v[2 * (term_start + offset) + phase])
        .sum();

    while residual > 0 {
        let mut best = None::<(usize, f64)>;
        for offset in 0..term_count {
            let index = 2 * (term_start + offset) + phase;
            if v[index] <= -120 {
                continue;
            }
            // Prefer the integer that was rounded furthest upward.
            let error = v[index] as f64 - w[index];
            if best.map_or(true, |(_, best_error)| error > best_error) {
                best = Some((index, error));
            }
        }
        let index = best
            .expect("PSQT integer normalization has a decrement candidate")
            .0;
        v[index] -= 1;
        residual -= 1;
    }

    while residual < 0 {
        let mut best = None::<(usize, f64)>;
        for offset in 0..term_count {
            let index = 2 * (term_start + offset) + phase;
            if v[index] >= 120 {
                continue;
            }
            // Prefer the integer that was rounded furthest downward.
            let error = v[index] as f64 - w[index];
            if best.map_or(true, |(_, best_error)| error < best_error) {
                best = Some((index, error));
            }
        }
        let index = best
            .expect("PSQT integer normalization has an increment candidate")
            .0;
        v[index] += 1;
        residual += 1;
    }
}

fn create_parent(path: &std::path::Path) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)?;
        }
    }
    Ok(())
}

fn parse_u64(text: &str) -> Result<u64, Box<dyn std::error::Error>> {
    Ok(if let Some(hex) = text.strip_prefix("0x") {
        u64::from_str_radix(hex, 16)?
    } else {
        text.parse()?
    })
}

fn shuffle_indices(values: &mut [usize], seed: u64) {
    let mut rng = SplitMix64(seed);
    for i in (1..values.len()).rev() {
        let j = (rng.next() % (i as u64 + 1)) as usize;
        values.swap(i, j);
    }
}

struct SplitMix64(u64);
impl SplitMix64 {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }
}

#[inline(always)]
fn sigmoid(x: f64) -> f64 {
    if x >= 0.0 {
        1.0 / (1.0 + (-x).exp())
    } else {
        let e = x.exp();
        e / (1.0 + e)
    }
}

#[inline(always)]
fn pop_lsb(bb: &mut u32) -> u8 {
    let sq = bb.trailing_zeros() as u8;
    *bb &= *bb - 1;
    sq
}

/// RDTUNE3 STATIC reader. The Python entry point checks SHA-256 manifests before
/// invoking the tuner. This reader additionally validates framing and records.
fn load_v3_role(
    path: &std::path::Path,
    split: DataSplit,
    role: u8,
    samples: &mut Vec<Sample>,
) -> Result<(), Box<dyn std::error::Error>> {
    use std::io::Read;
    let file = std::fs::File::open(path)?;
    let bytes = file.metadata()?.len();
    let mut input = std::io::BufReader::new(file);
    let mut header = [0u8; 28];
    input.read_exact(&mut header)?;
    let count = u64::from_le_bytes(header[16..24].try_into()?);
    if &header[..8] != b"RDTUNE3\0"
        || u32::from_le_bytes(header[8..12].try_into()?) != 1
        || u32::from_le_bytes(header[12..16].try_into()?) != 246
        || u16::from_le_bytes(header[24..26].try_into()?) != EVAL_TERM_COUNT as u16
        || header[26] != role
        || header[27] != 0
        || count > 50_000_000
        || bytes != 28 + count * 246
    {
        return Err("invalid RDTUNE3 header/length/role".into());
    }
    let mut progress = Instant::now();
    eprintln!("Загрузка {}: {count} записей", path.display());
    for index in 0..count {
        if progress.elapsed().as_secs() >= 5 {
            eprintln!("Загрузка {}: {index}/{count}", path.display());
            progress = Instant::now();
        }
        let mut r = [0u8; 246];
        input.read_exact(&mut r)?;
        let position = Position {
            white: u32::from_le_bytes(r[32..36].try_into()?),
            black: u32::from_le_bytes(r[36..40].try_into()?),
            kings: u32::from_le_bytes(r[40..44].try_into()?),
            side_to_move: match r[44] {
                0 => russian_draughts_classic::Side::White,
                1 => russian_draughts_classic::Side::Black,
                _ => return Err("invalid side".into()),
            },
        };
        russian_draughts_classic::api::validate_position(position)?;
        let phase = u16::from_le_bytes(r[48..50].try_into()?);
        let weight = f64::from_le_bytes(r[54..62].try_into()?);
        if r[45] > 2
            || phase > 256
            || r[50] > 3
            || r[53] != 0
            || r[51] as u32 != position.occupied().count_ones()
            || r[52] as u32 != position.kings.count_ones()
            || !weight.is_finite()
            || weight <= 0.0
            || weight > 1.0
        {
            return Err("invalid RDTUNE3 sample".into());
        }
        let mut base = [0i16; BASE_EVAL_TERM_COUNT];
        for (i, value) in base.iter_mut().enumerate() {
            *value = i16::from_le_bytes(r[62 + 2 * i..64 + 2 * i].try_into()?);
        }
        samples.push(Sample {
            base,
            position,
            phase,
            y: r[45] as f64 / 2.0,
            split,
            weight,
            flags: r[50],
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use russian_draughts_classic::Side;

    #[test]
    fn polish_improves_train_but_dev_can_veto() {
        let position = Position {
            white: 1,
            black: 1 << 31,
            kings: 1,
            side_to_move: Side::White,
        };
        let features = Evaluator::default().features(&Game::new(position));
        let mut base_features = [0; BASE_EVAL_TERM_COUNT];
        base_features.copy_from_slice(&features.values[..BASE_EVAL_TERM_COUNT]);
        let train = Sample {
            base: base_features,
            position,
            phase: features.phase,
            y: 1.0,
            split: DataSplit::Train,
            weight: 1.0,
            flags: 0,
        };
        let dev = Sample {
            split: DataSplit::Validation,
            ..train.clone()
        };
        let o = Options {
            corpus: PathBuf::new(),
            base: PathBuf::new(),
            output: None,
            epochs: 1,
            batch: 1,
            lr: 0.15,
            scale: None,
            l2: 0.0,
            seed: 7,
            patience: 8,
            evaluate_test: false,
            polish_passes: 1,
            polish_samples: 8,
        };
        let initial = EvalParams::default();
        let samples = [train.clone(), dev.clone()];
        let tuned = integer_polish(&samples, initial, &initial.to_vector(), 600.0, &o);
        assert!(
            loss_serialized(&samples, tuned, 600.0, DataSplit::Train)
                < loss_serialized(&samples, initial, 600.0, DataSplit::Train)
        );
        let veto = [train, Sample { y: 0.0, ..dev }];
        assert_eq!(
            integer_polish(&veto, initial, &initial.to_vector(), 600.0, &o).to_vector(),
            initial.to_vector()
        );
    }
    #[test]
    fn polish_neighbors_preserve_bounds_anchors_and_psqt_sums() {
        let mut w = EvalParams::default().to_vector().map(|x| x as f64);
        enforce_bounds(&mut w);
        project_psqt(&mut w);
        let base = rounded_params(&w);
        for i in 0..EVAL_PARAM_COUNT {
            for delta in [-2, -1, 1, 2] {
                let v = perturb(base, i, delta).to_vector();
                assert_eq!(v[0], 100);
                assert_eq!(v[1], 100);
                for phase in 0..2 {
                    for rank in 0..8 {
                        assert_eq!(
                            (0..4)
                                .map(|f| v[2 * (MAN_PSQT_OFFSET + rank * 4 + f) + phase])
                                .sum::<i32>(),
                            0
                        );
                    }
                    assert_eq!(
                        (0..32)
                            .map(|f| v[2 * (KING_PSQT_OFFSET + f) + phase])
                            .sum::<i32>(),
                        0
                    );
                }
                for value in &v[2 * MAN_PSQT_OFFSET..] {
                    assert!((-120..=120).contains(value));
                }
            }
        }
    }
    #[test]
    fn continuous_psqt_projection_preserves_bounds_and_zero_sum() {
        let mut w = [0.0f64; EVAL_PARAM_COUNT];
        for phase in 0..2 {
            for sq in 0..32 {
                w[2 * (MAN_PSQT_OFFSET + sq) + phase] = if sq & 1 == 0 { 300.0 } else { -170.0 };
                w[2 * (KING_PSQT_OFFSET + sq) + phase] = if sq % 3 == 0 { 400.0 } else { -95.0 };
            }
        }
        project_psqt(&mut w);
        for phase in 0..2 {
            for rank in 0..8 {
                let mut sum = 0.0;
                for file in 0..4 {
                    let value = w[2 * (MAN_PSQT_OFFSET + rank * 4 + file) + phase];
                    assert!((-120.0..=120.0).contains(&value));
                    sum += value;
                }
                assert!(
                    sum.abs() < 1e-9,
                    "man PSQT rank {rank} phase {phase}: sum={sum}"
                );
            }
            let mut king_sum = 0.0;
            for sq in 0..32 {
                let value = w[2 * (KING_PSQT_OFFSET + sq) + phase];
                assert!((-120.0..=120.0).contains(&value));
                king_sum += value;
            }
            assert!(
                king_sum.abs() < 1e-8,
                "king PSQT phase {phase}: sum={king_sum}"
            );
        }
    }

    #[test]
    fn integer_psqt_normalization_survives_rounding() {
        let mut w = [0.0f64; EVAL_PARAM_COUNT];
        let base = EvalParams::default().to_vector();
        for (dst, &src) in w.iter_mut().zip(base.iter()) {
            *dst = src as f64;
        }
        for phase in 0..2 {
            for sq in 0..32 {
                let man = 2 * (MAN_PSQT_OFFSET + sq) + phase;
                let king = 2 * (KING_PSQT_OFFSET + sq) + phase;
                w[man] = ((sq * 17 + phase * 11) % 13) as f64 * 0.37 - 2.1;
                w[king] = ((sq * 19 + phase * 7) % 17) as f64 * 0.29 - 2.3;
            }
        }
        project_psqt(&mut w);
        let v = rounded_params(&w).to_vector();
        for phase in 0..2 {
            for rank in 0..8 {
                let sum: i32 = (0..4)
                    .map(|file| v[2 * (MAN_PSQT_OFFSET + rank * 4 + file) + phase])
                    .sum();
                assert_eq!(sum, 0, "man PSQT rank {rank} phase {phase}");
            }
            let king_sum: i32 = (0..32)
                .map(|sq| v[2 * (KING_PSQT_OFFSET + sq) + phase])
                .sum();
            assert_eq!(king_sum, 0, "king PSQT phase {phase}");
        }
    }

    #[test]
    fn exact_serialized_score_matches_engine_arithmetic() {
        let mut params = EvalParams::default();
        for term in 0..EVAL_TERM_COUNT {
            let mg = ((term * 13) % 23) as i32 - 7;
            let eg = ((term * 17) % 29) as i32 - 9;
            *params.term_mut(term) = russian_draughts_classic::PhaseScore::new(mg, eg);
        }
        // Use non-multiple values here because term 13 is the one whose
        // product is truncated by /12 before phase interpolation.
        *params.term_mut(13) = russian_draughts_classic::PhaseScore::new(11, -7);
        let evaluator = Evaluator::new(params);
        let feature_extractor = Evaluator::default();
        let weights = params.to_vector();
        let mut rng = 0xD1B5_4A32_D192_ED03u64;

        for _ in 0..2_000 {
            let position = random_position(&mut rng);
            let game = Game::new(position);
            let features = feature_extractor.base_features(&game);
            let mut base = [0i16; BASE_EVAL_TERM_COUNT];
            base.copy_from_slice(&features.values[..BASE_EVAL_TERM_COUNT]);
            let sample = Sample {
                base,
                position,
                phase: features.phase,
                y: 0.5,
                split: DataSplit::Train,
                weight: 1.0,
                flags: 0,
            };
            assert_eq!(
                exact_serialized_score(&weights, &sample),
                evaluator.evaluate(&game).total_white,
                "position={position:?}"
            );
        }
    }

    fn random_position(state: &mut u64) -> Position {
        let mut white = next_u32(state);
        while white.count_ones() > 12 {
            white &= white - 1;
        }
        let mut black = next_u32(state) & !white;
        while black.count_ones() > 12 {
            black &= black - 1;
        }
        if white | black == 0 {
            white = 1;
        }
        Position {
            white,
            black,
            kings: next_u32(state) & (white | black),
            side_to_move: if next_u32(state) & 1 == 0 {
                Side::White
            } else {
                Side::Black
            },
        }
    }

    fn next_u32(state: &mut u64) -> u32 {
        let mut x = *state;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        *state = x;
        (x >> 16) as u32
    }
    #[test]
    fn weighted_loss_is_invariant_to_splitting_one_observation() {
        let position = Position::initial();
        let features = Evaluator::default().base_features(&Game::new(position));
        let mut base = [0i16; BASE_EVAL_TERM_COUNT];
        base.copy_from_slice(&features.values[..BASE_EVAL_TERM_COUNT]);
        let a = Sample {
            base,
            position,
            phase: features.phase,
            y: 1.0,
            split: DataSplit::Train,
            weight: 1.0,
            flags: 0,
        };
        let b = Sample {
            y: 0.0,
            weight: 3.0,
            ..a.clone()
        };
        let params = EvalParams::default();
        let before = loss_serialized(&[a.clone(), b.clone()], params, 100.0, DataSplit::Train);
        let half = Sample { weight: 0.5, ..a };
        let after = loss_serialized(&[half.clone(), half, b], params, 100.0, DataSplit::Train);
        assert!((before - after).abs() < 1e-12);
    }
}
