//! The native differential gate: `--smoke` (per push), `--full` (the versioned
//! matrix, the set rule, topology and mismatch artifacts), `--cranelift` (the
//! Cranelift parity gate) and `--cranelift-times` (emission time per backend),
//! `--replay` and `--reduce` of a stored artifact.
use hirsel_cli_args::{
    CommonOption, CommonOptions, Shard,
    lexopt::{Arg::*, Parser, ValueExt},
    parse_or_exit,
};
use hirsel_conformance::native::{self, Case, Class, artifact};
use hirsel_driver::native::{
    Configuration, Outcome, TargetSpec, Toolchain, full_matrix, smoke_matrix, topology_matrix,
};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

const USAGE: &str = "usage: native --smoke [--filter TEXT]... | --sanitize [--filter TEXT]... | --cranelift [--clif] [--aarch64] [--filter TEXT]... [--shard I/N] | --cranelift-times [--filter TEXT]... | --full [--bless] [--filter TEXT]... [--shard I/N] [--engine heap|llvm|diagnostic|default|system] [--seed N] [--exhaustive] | --hash-sweep [--filter TEXT]... | --replay DIR | --reduce DIR\n\
                     options: [--root PATH] [--jobs N] [--out DIR] [--hash-seed N]";

fn main() -> ExitCode {
    match run() {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(message) => {
            eprintln!("native: {message}");
            ExitCode::from(2)
        }
    }
}

/// The cases `--filter` and `--shard` select: every case whose ID contains
/// any given text (all of them without one) and that the shard holds.
#[derive(Default)]
struct Filter(Vec<String>, Option<Shard>);

impl Filter {
    fn keeps(&self, id: &str) -> bool {
        (self.0.is_empty() || self.0.iter().any(|text| id.contains(text.as_str())))
            && self.1.as_ref().is_none_or(|shard| shard.holds(id))
    }

    /// No text selects: the run covers every case of its shard.
    fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

fn toolchain(matrix: &[Configuration]) -> Result<Toolchain, String> {
    Toolchain::for_matrix(&native::runtime_cache()?, matrix).map_err(|e| e.to_string())
}

/// The case IDs a list file names, with their sources.
fn listed(root: &Path, list: &str) -> Result<Vec<Case>, String> {
    let text = std::fs::read_to_string(root.join(list)).map_err(|e| format!("{list}: {e}"))?;
    text.lines()
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(|id| {
            Ok(Case {
                id: id.to_string(),
                path: Some(root.join(id)),
                source: std::fs::read_to_string(root.join(id)).map_err(|e| format!("{id}: {e}"))?,
            })
        })
        .collect()
}

/// Every case must agree under every configuration. A campaign that checks
/// nothing fails: no cases, or a case run under no configuration.
fn all_agree(what: &str, results: &[native::CaseResult]) -> bool {
    if results.is_empty() {
        eprintln!("{what}: selected no cases");
        return false;
    }
    let mut ok = true;
    for result in results {
        if let Some(reason) = &result.snapshot {
            eprintln!("{what}: {}: {reason}", result.id);
            ok = false;
        } else if result.outcomes.is_empty() {
            eprintln!("{what}: {}: ran under no configuration", result.id);
            ok = false;
        }
        for (name, _, outcome) in &result.outcomes {
            match outcome {
                Outcome::Agree => {}
                Outcome::Mismatch(actual) => {
                    eprintln!(
                        "{what}: {} under {name} observed\n{}",
                        result.id,
                        actual.render()
                    );
                    ok = false;
                }
                Outcome::Unavailable(error) => {
                    eprintln!("{what}: {} under {name}: {error}", result.id);
                    ok = false;
                }
            }
        }
    }
    ok
}

fn run() -> Result<bool, String> {
    let mut mode = None;
    // The host is shared: four cases at a time unless told otherwise.
    let mut jobs = 4;
    let mut out = None;
    let mut filter = Filter::default();
    let mut bless = false;
    let mut clif = false;
    let mut aarch64 = false;
    let mut exhaustive = false;
    let mut seed = std::env::var("PROPTEST_RNG_SEED")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let mut engine = None;
    let mut common = CommonOptions::default();
    let help = parse_or_exit((|| {
        let mut args = Parser::from_env();
        while let Some(arg) = args.next()? {
            if let Some(option) = CommonOption::from_arg(&arg) {
                common.parse(option, &mut args)?;
                continue;
            }
            match arg {
                Long(
                    name @ ("smoke" | "full" | "sanitize" | "hash-sweep" | "cranelift"
                    | "cranelift-times"),
                ) => {
                    mode = Some((format!("--{name}"), PathBuf::new()));
                }
                Long(name @ ("replay" | "reduce")) => {
                    let name = format!("--{name}");
                    mode = Some((name, args.value()?.into()));
                }
                Long("bless") => bless = true,
                Long("clif") => clif = true,
                Long("aarch64") => aarch64 = true,
                Long("exhaustive") => exhaustive = true,
                Long("seed") => seed = args.value()?.parse()?,
                Long("out") => out = Some(PathBuf::from(args.value()?)),
                Long("filter") => filter.0.push(args.value()?.string()?),
                Long("engine") => {
                    use hirsel_driver::native::Runtime;
                    let (class, runtime) = match args.value()?.string()?.as_str() {
                        "heap" => (Class::Heap, None),
                        "llvm" => (Class::Llvm, None),
                        "diagnostic" => (Class::Llvm, Some(Runtime::Diagnostic)),
                        "default" => (Class::Llvm, Some(Runtime::Default)),
                        "system" => (Class::Llvm, Some(Runtime::System)),
                        _ => return Err("--engine must be heap, llvm, or a runtime archive: diagnostic, default or system".into()),
                    };
                    engine = Some(Selected { class, runtime });
                }
                Long("jobs") => jobs = args.value()?.parse()?,
                Long("help") | Short('h') => return Ok(true),
                _ => return Err(arg.unexpected()),
            }
        }
        Ok(false)
    })());
    if help {
        println!("{USAGE}");
        return Ok(true);
    }
    let root = common
        .root
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../.."));
    let hash_seed = common.hash_seed.unwrap_or(0);
    let hash_seed_explicit = common.hash_seed.is_some();
    filter.1 = common.shard;
    let (mode, dir) = mode.ok_or(USAGE)?;
    if engine.is_some() && mode != "--full" {
        return Err("--engine requires --full".into());
    }
    if filter.1.is_some() && mode != "--full" && mode != "--cranelift" {
        return Err("--shard requires --full or --cranelift".into());
    }
    match mode.as_str() {
        "--smoke" => {
            let matrix = seeded(smoke_matrix(), hash_seed);
            let mut cases = if filter.is_empty() {
                let mut cases = listed(&root, "native-tests/regressions/smoke.txt")?;
                cases.extend(native::generate::cases(8));
                cases
            } else {
                native::cranelift::cases(&root)?
            };
            cases.retain(|c| filter.keeps(&c.id));
            if cases.is_empty() {
                return Err(format!(
                    "native smoke: --filter {:?} selected no cases",
                    filter.0
                ));
            }
            let results = native::run_cases(
                &cases,
                &matrix,
                &toolchain(&matrix)?,
                &native::scratch("smoke")?,
                jobs,
            );
            let mut ok = all_agree("smoke", &results);
            // Real Io: each case is recorded on real Io and replayed from its
            // completion tape on simulated Io.
            let toolchain = toolchain(&matrix)?;
            let replayed = native::run_cases_with(
                &cases,
                &native::scratch("smoke-replay")?,
                jobs,
                |case, dir| {
                    native::replay_source_with_config(
                        &case.id,
                        &case.source,
                        case.path.as_deref(),
                        &toolchain,
                        dir,
                        matrix[0].options.launch.clone(),
                    )
                },
            );
            ok &= all_agree("real Io replay", &replayed);
            // A handful of cases as Cranelift executables.
            let cranelift = [native::cranelift::cranelift(matrix[0].options.clone())];
            let compiled = native::run_cases(
                &listed(&root, native::cranelift::SMOKE)?,
                &cranelift,
                &toolchain,
                &native::scratch("smoke-cranelift")?,
                jobs,
            );
            ok &= all_agree("cranelift", &compiled);
            println!(
                "native smoke: {} cases under {} configurations and real Io record and replay, \
                 {} as Cranelift executables{}",
                results.len(),
                matrix.len(),
                compiled.len(),
                if ok { " agree" } else { ": FAILED" }
            );
            Ok(ok)
        }
        "--cranelift" => {
            let target = if aarch64 {
                TargetSpec::aarch64_unknown_linux_gnu()
            } else {
                TargetSpec::x86_64_unknown_linux_gnu()
            };
            cranelift(&root, jobs, &filter, hash_seed, clif, target)
        }
        "--cranelift-times" => cranelift_times(&root, jobs, &filter),
        "--sanitize" => {
            // The smoke and memcheck cases as optimized and unoptimized x86-64
            // executables on the system-allocator archive and on the size-class
            // archive with client requests under Valgrind memcheck: emitted code
            // and the runtime together.
            let matrix = sanitize_matrix(hash_seed);
            let cases = if filter.is_empty() {
                let mut cases = listed(&root, "native-tests/regressions/smoke.txt")?;
                cases.extend(listed(&root, "native-tests/regressions/memcheck.txt")?);
                cases.extend(native::generate::cases(8));
                cases
            } else {
                let mut cases = native::baseline(&root)?;
                cases.retain(|c| filter.keeps(&c.id));
                cases
            };
            let toolchain = toolchain(&matrix)?.under_memcheck();
            let results = native::run_cases(
                &cases,
                &matrix,
                &toolchain,
                &native::scratch("sanitize")?,
                jobs,
            );
            let ok = all_agree("memcheck", &results);
            println!(
                "native memcheck: {} cases under {} configurations{}",
                results.len(),
                matrix.len(),
                if ok {
                    " agree with no error"
                } else {
                    ": FAILED"
                }
            );
            Ok(ok)
        }
        "--full" => full(
            &root,
            jobs,
            out,
            &filter,
            bless,
            Coverage {
                hash_seed,
                seed,
                exhaustive,
            },
            engine,
        ),
        "--hash-sweep" => hash_sweep(&root, jobs, &filter, hash_seed),
        "--replay" | "--reduce" => {
            let stored = artifact::read(&dir)?;
            let configuration = artifact::configuration(&stored.configuration)
                .ok_or_else(|| format!("unknown configuration {}", stored.configuration))?;
            if hash_seed_explicit && hash_seed != configuration.options.launch.hash_seed {
                return Err("--hash-seed differs from the stored artifact configuration".into());
            }
            let toolchain = toolchain(std::slice::from_ref(&configuration))?;
            let scratch = native::scratch("replay")?;
            let reduce = mode == "--reduce";
            hirsel_engine::with_compiler_stack(move || {
                let probe = artifact::NativeProbe {
                    configuration,
                    toolchain: &toolchain,
                    dir: &scratch,
                    warm: None,
                    source_path: Some(dir.join("source.hir")),
                };
                artifact::replay(&stored, &probe)?;
                if reduce {
                    print!("{}", artifact::reduce(&stored.source, &probe)?);
                } else {
                    println!("replayed: the same expected and actual observations");
                }
                Ok(true)
            })
        }
        _ => unreachable!(),
    }
}

/// `--cranelift`: the parity gate over the selected cases (all without a filter).
/// With `clif`, each selected case's Cranelift IR is printed first. The parity list is
/// `target`'s: x86-64's file, or AArch64's, run under qemu.
fn cranelift(
    root: &Path,
    jobs: usize,
    filter: &Filter,
    hash_seed: u64,
    clif: bool,
    target: TargetSpec,
) -> Result<bool, String> {
    use native::cranelift;
    let list = cranelift::parity_list(&target);
    let matrix = seeded(cranelift::matrix(target), hash_seed);
    let mut cases = cranelift::cases(root)?;
    cases.retain(|c| filter.keeps(&c.id));
    if clif {
        for case in &cases {
            let case = case.clone();
            let text = hirsel_engine::with_compiler_stack(move || cranelift::clif(&case))
                .unwrap_or_else(|e| format!("unavailable: {e}"));
            println!("{text}");
        }
    }
    let text = std::fs::read_to_string(root.join(list)).map_err(|e| format!("{list}: {e}"))?;
    let listed = cranelift::parse(&text)?;
    let results = native::run_cases(
        &cases,
        &matrix,
        &toolchain(&matrix)?,
        &native::scratch("cranelift")?,
        jobs,
    );
    let report = cranelift::judge(&results, &listed);
    for (id, reason) in &report.unlisted {
        eprintln!("cranelift: {id}: {reason}");
        // The whole failure, for the lane that debugs it.
        let outcome = results
            .iter()
            .filter(|r| r.id == *id)
            .flat_map(|r| &r.outcomes)
            .find(|(_, class, _)| *class == Class::Cranelift);
        match outcome {
            Some((_, _, Outcome::Unavailable(error))) => eprintln!("  {error}"),
            Some((_, _, Outcome::Mismatch(actual))) => eprintln!("  observed\n{}", actual.render()),
            _ => {}
        }
    }
    for id in &report.stale {
        eprintln!("cranelift: {id} passes; remove its row from {list}");
    }
    // Cases outside the gate, and why LLVM does not pass them.
    for result in results.iter().filter(|r| cranelift::verdict(r).is_none()) {
        let why = result.snapshot.clone().unwrap_or_else(|| {
            result
                .outcomes
                .iter()
                .find(|(_, class, _)| *class == Class::Llvm)
                .and_then(|(_, _, outcome)| native::exclusion(outcome))
                .map_or_else(
                    || "no LLVM run".into(),
                    |e| format!("{}: {}", e.class, e.reason),
                )
        });
        eprintln!("cranelift: outside the gate: {}: {why}", result.id);
    }
    // Rows of cases this run did not reach stay as they are.
    let ran: std::collections::BTreeSet<_> = results.iter().map(|r| r.id.as_str()).collect();
    if !filter.is_empty() || filter.1.is_some() {
        println!("the parity rows of the selected cases:");
    }
    let mut rendered = cranelift::render(&results);
    for (id, reason) in listed.iter().filter(|(id, _)| !ran.contains(id.as_str())) {
        rendered.push_str(&format!("{id}\t{reason}\n"));
    }
    print!("{rendered}");
    let ok = report.ok() && !results.is_empty();
    println!(
        "cranelift parity: {} of {} cases LLVM passes agree as Cranelift executables ({} selected){}",
        report.passing,
        report.gated,
        results.len(),
        if ok { "" } else { ": FAILED" }
    );
    Ok(ok)
}

/// `--cranelift-times`: emission wall time per case and backend, and the totals.
fn cranelift_times(root: &Path, jobs: usize, filter: &Filter) -> Result<bool, String> {
    use native::cranelift::{self, BACKENDS};
    let mut cases = cranelift::cases(root)?;
    cases.retain(|c| filter.keeps(&c.id));
    let times = std::sync::Mutex::new(Vec::new());
    native::run_cases_with(
        &cases,
        &native::scratch("cranelift-times")?,
        jobs,
        |case, _| {
            let timed = cranelift::compile_times(case);
            times.lock().unwrap().push((case.id.clone(), timed));
            native::CaseResult {
                id: case.id.clone(),
                snapshot: None,
                outcomes: Vec::new(),
                reference: None,
                wall: Default::default(),
            }
        },
    );
    let mut times = times.into_inner().unwrap();
    times.sort_by(|a, b| a.0.cmp(&b.0));
    let names: Vec<_> = BACKENDS
        .iter()
        .map(|(name, ..)| format!("{name}_ms"))
        .collect();
    // Two spaces apart: test logs drop tabs.
    println!("case  {}", names.join("  "));
    let mut totals = [std::time::Duration::ZERO; 3];
    let mut timed = 0;
    for (id, result) in &times {
        match result {
            Ok(walls) => {
                timed += 1;
                let row: Vec<_> = walls
                    .iter()
                    .zip(&mut totals)
                    .map(|(wall, total)| {
                        *total += *wall;
                        format!("{:.2}", wall.as_secs_f64() * 1e3)
                    })
                    .collect();
                println!("{id}  {}", row.join("  "));
            }
            Err(error) => println!("{id}  unavailable: {}", error.replace(['\t', '\n'], " ")),
        }
    }
    let total = |i: usize| totals[i].as_secs_f64();
    println!(
        "emission over {timed} cases: cranelift {:.2} s, llvm O0 {:.2} s ({:.1}x), llvm O2 {:.2} s ({:.1}x)",
        total(0),
        total(1),
        total(1) / total(0).max(1e-9),
        total(2),
        total(2) / total(0).max(1e-9),
    );
    Ok(timed > 0)
}

/// `--engine`: one engine's configurations, or native code on one runtime
/// archive.
#[derive(Clone, Copy)]
struct Selected {
    class: Class,
    runtime: Option<hirsel_driver::native::Runtime>,
}

impl Selected {
    fn keeps(self, c: &Configuration) -> bool {
        Class::of(c) == self.class
            && self.runtime.is_none_or(|r| {
                matches!(c.engine, hirsel_driver::native::Engine::Native { runtime, .. } if runtime == r)
            })
    }
}

struct Coverage {
    hash_seed: u64,
    seed: u64,
    exhaustive: bool,
}

fn full(
    root: &Path,
    jobs: usize,
    out: Option<PathBuf>,
    filter: &Filter,
    bless: bool,
    coverage: Coverage,
    engine: Option<Selected>,
) -> Result<bool, String> {
    let Coverage {
        hash_seed,
        seed,
        exhaustive,
    } = coverage;
    if bless && !exhaustive {
        return Err("--bless requires --exhaustive".into());
    }
    if bless && hash_seed != 0 {
        return Err("--bless requires the baseline hash seed".into());
    }
    if bless && filter.1.is_some() {
        return Err("--bless needs the whole baseline, without --shard".into());
    }
    let out = match out {
        Some(out) => {
            std::fs::create_dir_all(&out).map_err(|e| format!("{}: {e}", out.display()))?;
            out
        }
        None => native::scratch("report")?,
    };
    let matrix: Vec<_> = seeded(full_matrix(), hash_seed)
        .into_iter()
        .filter(|c| engine.is_none_or(|e| e.keeps(c)))
        .collect();
    let toolchain = toolchain(&matrix)?;
    let selection = if exhaustive || !filter.is_empty() {
        None
    } else {
        Some(native::diet::Selection::load(root)?)
    };
    let exclusions = native::read_exclusions(root)?;
    let mut cases = native::baseline(root)?;
    let baseline_before = cases.len();
    let complete = filter.is_empty() && engine.is_none();
    // Only the whole run must find every campaign non-empty; a shard may hold
    // no topology or generated case.
    let whole = complete && filter.1.is_none();
    cases.retain(|c| {
        filter.keeps(&c.id)
            && (!filter.is_empty()
                || selection
                    .as_ref()
                    .is_none_or(|s| s.selected.contains(&c.id)))
    });
    let results = native::run_cases_with(&cases, &native::scratch("full")?, jobs, |case, dir| {
        let selected =
            native::diet::configurations(&matrix, &case.id, seed, exhaustive, &exclusions);
        case.run(&selected, &toolchain, dir)
    });
    let write = |name: &str, text: &str| {
        std::fs::write(out.join(name), text).map_err(|e| format!("{name}: {e}"))
    };
    write("native-matrix.tsv", &native::render_matrix(&results))?;
    write("case-walls.tsv", &native::render_walls(&results))?;
    if bless {
        if !complete {
            return Err(
                "--bless needs the whole baseline and both engines, without --filter or --engine"
                    .into(),
            );
        }
        std::fs::write(
            root.join(native::EXCLUSIONS),
            native::render_exclusions(&results),
        )
        .map_err(|e| format!("{}: {e}", native::EXCLUSIONS))?;
    }
    // A shard judges the rows of its own cases, including rows naming a case
    // or configuration no longer in the matrix.
    let mut exclusions = exclusions;
    exclusions.retain(|(id, _), _| filter.keeps(id));
    let classes: Vec<_> = engine.map_or_else(|| vec![Class::Heap, Class::Llvm], |e| vec![e.class]);
    let report = native::judge_classes(&results, &exclusions, matrix.len(), complete, &classes);
    // Every mismatch leaves an artifact, which must replay at once, from a
    // warmed engine, with the same observations; and each mismatching case's
    // first artifact is reduced, the reduction keeping the same discrepancy.
    struct Stored {
        dir: PathBuf,
        name: String,
        first: bool,
    }
    let mut stored = Vec::new();
    for (result, case) in results.iter().zip(&cases) {
        let mut first = true;
        for (name, _, outcome) in &result.outcomes {
            let (Outcome::Mismatch(actual), Some(expected)) = (outcome, &result.reference) else {
                continue;
            };
            let slug: String = format!("{}--{name}", case.id)
                .chars()
                .map(|c| {
                    if c.is_ascii_alphanumeric() || c == '-' {
                        c
                    } else {
                        '_'
                    }
                })
                .collect();
            // Written under one path and replayed from another.
            let dir = out.join("mismatch").join(&slug);
            let written = out.join("mismatch").join(format!("{slug}.written"));
            let _ = std::fs::remove_dir_all(&dir);
            artifact::write(&written, &case.id, &case.source, name, expected, actual)?;
            if let Some(path) = &case.path {
                hirsel_engine::bundle_program(path, &case.source, &written)
                    .map_err(|e| e.to_string())?;
            }
            std::fs::rename(&written, &dir).map_err(|e| format!("{}: {e}", dir.display()))?;
            stored.push(Stored {
                dir,
                name: name.clone(),
                first,
            });
            first = false;
        }
    }
    let next = std::sync::atomic::AtomicUsize::new(0);
    let failures = std::sync::Mutex::new(Vec::new());
    std::thread::scope(|scope| {
        for job in 0..jobs.max(1) {
            let (next, failures, stored, toolchain) = (&next, &failures, &stored, &toolchain);
            scope.spawn(move || {
                let scratch = native::scratch(&format!("replay-{job}")).expect("scratch");
                hirsel_engine::with_compiler_stack(move || {
                    loop {
                        let index = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        let Some(item) = stored.get(index) else { break };
                        let verdict = (|| {
                            let artifact = artifact::read(&item.dir)?;
                            let configuration = artifact::configuration(&item.name)
                                .ok_or("an unnamed configuration")?;
                            let probe = artifact::NativeProbe {
                                configuration,
                                toolchain,
                                dir: &scratch,
                                source_path: Some(item.dir.join("source.hir")),
                                warm: Some("fn warm(x: u64) -> u64 { x }\ntype Warm { a: u64 }\n"),
                            };
                            artifact::replay(&artifact, &probe)?;
                            if item.first {
                                let reduced = artifact::reduce(&artifact.source, &probe)?;
                                std::fs::write(item.dir.join("reduced.hir"), reduced)
                                    .map_err(|e| e.to_string())?;
                            }
                            Ok::<_, String>(())
                        })();
                        if let Err(e) = verdict {
                            failures
                                .lock()
                                .unwrap()
                                .push(format!("{}: {e}", item.dir.display()));
                        }
                    }
                });
            });
        }
    });
    let failures = failures.into_inner().unwrap();
    for failure in &failures {
        eprintln!("native: artifact does not replay or reduce: {failure}");
    }
    let mut ok = failures.is_empty();
    let replayed = stored.len() - failures.len();
    let reduced = stored.iter().filter(|s| s.first).count();
    // Topology (PROG-5): the listed multi-unit cases agree as one object, one
    // object per unit and twenty seeded random partitions.
    let topology: Vec<_> = seeded(topology_matrix(20), hash_seed)
        .into_iter()
        .filter(|c| engine.is_none_or(|e| e.keeps(c)))
        .collect();
    let mut partitioned = listed(root, "native-tests/topology/cases.txt")?;
    partitioned.retain(|c| filter.keeps(&c.id));
    let topology_results = native::run_cases(
        &partitioned,
        &topology,
        &toolchain,
        &native::scratch("topology")?,
        jobs,
    );
    write(
        "topology-matrix.tsv",
        &native::render_matrix(&topology_results),
    )?;
    if whole || !(partitioned.is_empty() || topology.is_empty()) {
        ok &= all_agree("topology", &topology_results);
    }
    // Generated successes and directed failures agree under every
    // configuration of the matrix.
    let mut generated = native::generate::cases(64);
    generated.retain(|c| filter.keeps(&c.id));
    let generated_results = native::run_cases_with(
        &generated,
        &native::scratch("generated")?,
        jobs,
        |case, dir| {
            let selected = native::diet::configurations(
                &matrix,
                &case.id,
                seed,
                exhaustive,
                &native::Exclusions::new(),
            );
            case.run(&selected, &toolchain, dir)
        },
    );
    write(
        "generated-matrix.tsv",
        &native::render_matrix(&generated_results),
    )?;
    if whole || !generated.is_empty() {
        ok &= all_agree("generated", &generated_results);
    }
    // A restriction may empty a campaign, but not the whole run; the
    // unrestricted run checks every campaign (all_agree refuses an empty one).
    if matrix.is_empty() || whole && cases.is_empty() {
        eprintln!("native: the matrix selected no cases or no configurations");
        ok = false;
    }
    if !complete && cases.is_empty() && partitioned.is_empty() && generated.is_empty() {
        eprintln!("native: --filter {:?} selected no cases", filter.0);
        ok = false;
    }
    let mut json = report.json();
    json["seed"] = seed.into();
    json["exhaustive"] = exhaustive.into();
    json["baseline_before_selection"] = baseline_before.into();
    json["configuration_attempts"] = results
        .iter()
        .chain(&generated_results)
        .chain(&topology_results)
        .map(|r| r.outcomes.len())
        .sum::<usize>()
        .into();
    json["generated_cases"] = generated_results.len().into();
    json["mismatch_artifacts_replayed"] = replayed.into();
    json["mismatching_cases_reduced"] = reduced.into();
    json["topology_cases"] = topology_results.len().into();
    json["topology_partitions"] = topology.len().into();
    json["report"] = out.display().to_string().into();
    if let Some(shard) = &filter.1 {
        json["shard"] = format!("{}/{}", shard.index, shard.count).into();
    }
    println!("{json:#}");
    for line in report.unlisted.iter().chain(&report.stale) {
        eprintln!("native: {line}");
    }
    Ok(ok && report.ok())
}

fn seeded(mut matrix: Vec<Configuration>, hash_seed: u64) -> Vec<Configuration> {
    for config in &mut matrix {
        config.options.launch.hash_seed = hash_seed;
    }
    matrix
}
/// One shipped-archive optimized whole-program configuration per target over the LLVM passing corpus. Existing
/// named unsupported cases are outside this sweep; every selected case must agree.
fn hash_sweep(root: &Path, jobs: usize, filter: &Filter, hash_seed: u64) -> Result<bool, String> {
    let matrix = hash_sweep_matrix(hash_seed);
    let exclusions = native::read_exclusions(root)?;
    let mut cases = native::baseline(root)?;
    cases.retain(|c| {
        filter.keeps(&c.id) && !native::listed_under(&exclusions, &c.id, native::Class::Llvm)
    });
    if cases.is_empty() {
        return Err("hash sweep selected no passing cases".into());
    }
    let results = native::run_cases(
        &cases,
        &matrix,
        &toolchain(&matrix)?,
        &native::scratch("hash-sweep")?,
        jobs,
    );
    let ok = all_agree("hash sweep", &results);
    println!(
        "native hash sweep: seed {hash_seed}, {} passing cases under {} configurations{}",
        results.len(),
        matrix.len(),
        if ok { " agree" } else { ": FAILED" }
    );
    Ok(ok)
}

/// Memcheck's executables: x86-64 (the only target Valgrind runs here), the
/// default passes as one object at O0 and O3. On the system-allocator archive,
/// where each fixed-size block is its own libc allocation, memcheck sees a use
/// after free, a double free or a leak of one, with regions and without. On the
/// memcheck archive, the shipped size-class allocator whose client requests
/// describe its slab blocks, region reservations and arena blocks, it sees a use
/// of a freed slab slot or a reset token, a read past a reservation and a use
/// after a region or arena exits, with regions, as `hirsel run --native` ships.
fn sanitize_matrix(hash_seed: u64) -> Vec<Configuration> {
    use hirsel_driver::native::{Engine, OptLevel, Options, Partition, Runtime};
    let mut matrix = Vec::new();
    for (runtime, regions) in [
        (Runtime::System, true),
        (Runtime::System, false),
        (Runtime::Memcheck, true),
    ] {
        for opt in [OptLevel::None, OptLevel::Speed] {
            matrix.push(Configuration {
                options: Options {
                    regions,
                    ..Options::new(TargetSpec::x86_64_unknown_linux_gnu())
                },
                engine: Engine::Native {
                    opt,
                    partition: Partition::Whole,
                    runtime,
                },
            });
        }
    }
    seeded(matrix, hash_seed)
}

fn hash_sweep_matrix(hash_seed: u64) -> Vec<Configuration> {
    use hirsel_driver::native::{Engine, OptLevel, Partition, Runtime};
    seeded(
        full_matrix()
            .into_iter()
            .filter(|c| {
                c.options.flat_walk
                    && matches!(
                        c.engine,
                        Engine::Native {
                            opt: OptLevel::Speed,
                            partition: Partition::Whole,
                            runtime: Runtime::Default
                        }
                    )
            })
            .collect(),
        hash_seed,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn memcheck_runs_whole_x86_executables_on_the_system_and_memcheck_archives() {
        use hirsel_driver::native::{Engine, OptLevel, Partition, Runtime};
        let matrix = sanitize_matrix(0);
        let names: Vec<_> = matrix.iter().map(Configuration::name).collect();
        assert_eq!(names.len(), 6, "{names:?}");
        for (runtime, regions, opt) in [
            (Runtime::System, true, OptLevel::None),
            (Runtime::System, true, OptLevel::Speed),
            (Runtime::System, false, OptLevel::None),
            (Runtime::System, false, OptLevel::Speed),
            (Runtime::Memcheck, true, OptLevel::None),
            (Runtime::Memcheck, true, OptLevel::Speed),
        ] {
            assert!(
                matrix.iter().any(|c| {
                    let o = &c.options;
                    o.target == TargetSpec::x86_64_unknown_linux_gnu()
                        && o.regions == regions
                        && o.elide
                        && o.reuse
                        && o.flat_walk
                        && c.engine
                            == Engine::Native {
                                opt,
                                partition: Partition::Whole,
                                runtime,
                            }
                }),
                "{runtime:?} regions {regions} {opt:?} missing from {names:?}"
            );
        }
    }

    #[test]
    fn a_repeated_filter_keeps_a_case_any_text_selects() {
        let filter = Filter(vec!["SEM-2".into(), "MEM-4".into()], None);
        assert!(filter.keeps("conformance/SEM/SEM-2-a.hir"));
        assert!(filter.keeps("conformance/MEM/MEM-4-b.hir"));
        assert!(!filter.keeps("conformance/OWN/OWN-7-c.hir"));
        assert!(Filter::default().keeps("conformance/OWN/OWN-7-c.hir"));
    }

    #[test]
    fn shards_partition_the_cases_a_filter_selects() {
        let ids: Vec<String> = (0..200)
            .map(|i| format!("conformance/SEM/SEM-{i}.hir"))
            .collect();
        let selected = |filter: &Filter| ids.iter().filter(|id| filter.keeps(id)).count();
        let shards: Vec<_> = (1..=4)
            .map(|i| Filter(Vec::new(), Shard::parse(&format!("{i}/4"))))
            .collect();
        assert_eq!(shards.iter().map(selected).sum::<usize>(), ids.len());
        for id in &ids {
            assert_eq!(shards.iter().filter(|s| s.keeps(id)).count(), 1, "{id}");
        }
        assert!(shards.iter().all(|s| selected(s) > 0 && s.is_empty()));
        let narrowed = Filter(vec!["SEM-1".into()], Shard::parse("1/4"));
        assert!(
            ids.iter()
                .filter(|id| narrowed.keeps(id))
                .all(|id| id.contains("SEM-1"))
        );
    }

    #[test]
    fn a_campaign_over_no_cases_or_no_configurations_fails() {
        let result = |outcomes| native::CaseResult {
            id: "case.hir".into(),
            snapshot: None,
            outcomes,
            reference: None,
            wall: Default::default(),
        };
        assert!(!all_agree("empty", &[]));
        assert!(!all_agree("unconfigured", &[result(Vec::new())]));
        // The control: one case agreeing under one configuration passes.
        let agreed = vec![("x86_64/heap".into(), Class::Heap, Outcome::Agree)];
        assert!(all_agree("one", &[result(agreed)]));
    }

    #[test]
    fn second_hash_sweep_runs_one_shipped_archive_configuration_per_target() {
        let matrix = hash_sweep_matrix(1);
        assert_eq!(matrix.len(), 2);
        assert_ne!(matrix[0].options.target, matrix[1].options.target);
        for c in matrix {
            assert!(c.options.regions && c.options.elide && c.options.reuse && c.options.flat_walk);
            assert_eq!(
                c.options.launch,
                hirsel_driver::native::LaunchInputs {
                    hash_seed: 1,
                    ..Default::default()
                }
            );
            assert!(matches!(
                c.engine,
                hirsel_driver::native::Engine::Native {
                    opt: hirsel_driver::native::OptLevel::Speed,
                    partition: hirsel_driver::native::Partition::Whole,
                    runtime: hirsel_driver::native::Runtime::Default
                }
            ));
        }
    }
}
