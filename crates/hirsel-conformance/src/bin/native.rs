//! The native differential gate: `--smoke` (per push), `--full` (the versioned
//! matrix, the set rule, topology and mismatch artifacts), `--cranelift` (the
//! Cranelift parity gate), `--cranelift-times` (emission time per backend),
//! `--trace-roots` (S-14: every executable baseline case's `Shared` root trace in the
//! interpreter, LLVM and Cranelift, which must be equal), `--replay` and `--reduce` of a
//! stored artifact.

hirsel_guard::install!();

#[path = "native/isolate.rs"]
mod isolate;

use hirsel_cli_args::{
    CommonOption, CommonOptions, Shard,
    lexopt::{Arg::*, Parser, ValueExt},
    parse_or_exit,
};
use hirsel_conformance::native::{self, Case, Class, artifact};
use hirsel_driver::native::{
    Configuration, Engine, OptLevel, Options, Outcome, Partition, Runtime, TargetSpec, Toolchain,
    full_matrix, smoke_matrix, topology_matrix,
};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

const USAGE: &str = "usage: native --std-compile [--filter MODULE] [--shard I/N] | --corpus [--seed N] [--shard I/N] | --bugs | --smoke [--filter TEXT]... | --sanitize [--filter TEXT]... | --cranelift [--clif] [--aarch64] [--filter TEXT]... [--shard I/N] | --cranelift-times [--filter TEXT]... | --trace-roots [--filter TEXT]... [--shard I/N] | --migrate [--filter TEXT]... [--shard I/N] | --full [--bless] [--filter TEXT]... [--shard I/N] [--engine heap|llvm|diagnostic|default|system] [--seed N] [--exhaustive] | --hash-sweep [--filter TEXT]... | --replay DIR | --reduce DIR\n\
                     options: [--case ID]... [--root PATH] [--jobs N] [--out DIR] [--hash-seed N]";

fn main() -> ExitCode {
    if let Some(status) = hirsel_conformance::native::diet::observation_worker() {
        return status;
    }

    let started = std::time::Instant::now();
    let result = hirsel_engine::with_compiler_stack(run);
    isolate::report(started.elapsed(), matches!(result, Ok(true)));
    match result {
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
struct Filter(Vec<String>, Option<Shard>, Vec<String>);

impl Filter {
    fn keeps(&self, id: &str) -> bool {
        self.matches(id) && self.1.as_ref().is_none_or(|shard| shard.holds(id))
    }

    /// Whether the text selects `id`, whichever shard holds it.
    fn matches(&self, id: &str) -> bool {
        if let Some(selected) = isolate::selected() {
            return id == selected;
        }
        self.is_empty()
            || self.0.iter().any(|text| id.contains(text.as_str()))
            || self.2.iter().any(|case| id == case)
    }

    /// A run that selected no case fails unless it is a shard and the text
    /// selects some of `ids` that other shards hold: then it passes with zero
    /// cases.
    fn empty_fails<'a>(&self, mut ids: impl Iterator<Item = &'a str>) -> bool {
        self.1.is_none() || !ids.any(|id| self.matches(id))
    }

    /// No text selects: the run covers every case of its shard.
    fn is_empty(&self) -> bool {
        self.0.is_empty() && self.2.is_empty()
    }
}

fn toolchain(matrix: &[Configuration]) -> Result<Toolchain, String> {
    Toolchain::for_matrix(&native::runtime_cache()?, matrix).map_err(|e| e.to_string())
}

/// `toolchain` launching every executable through the glibc 2.28 floor
/// release's own loader (`HIRSEL_GLIBC_FLOOR`): the smoke programs prove they
/// load and run at the floor on both targets.
fn floor_toolchain(matrix: &[Configuration]) -> Result<Toolchain, String> {
    let mut toolchain = toolchain(matrix)?;
    toolchain.glibc_floor = Some(
        std::env::var_os("HIRSEL_GLIBC_FLOOR")
            .map(PathBuf::from)
            .ok_or("HIRSEL_GLIBC_FLOOR must name the glibc floor runtime")?,
    );
    Ok(toolchain)
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

/// Exact generated IDs stay replayable after the rotating window moves.
fn generated_cases(seed: u64, count: u64, filter: &Filter) -> Vec<Case> {
    eprintln!("native: generator seed {seed}; reproduce with --seed {seed}");
    let mut cases = native::generate::cases(seed, count);
    for case in filter
        .0
        .iter()
        .chain(&filter.2)
        .filter_map(|id| native::generate::from_id(id))
    {
        if !cases.iter().any(|existing| existing.id == case.id) {
            cases.push(case);
        }
    }
    cases
}

/// Report how many programs ran as images in runners and why any fell back to a linked
/// executable. A program that calls C or runs under memcheck runs as an executable by design;
/// any other fallback (an lld error, a refused image) fails the campaign.
fn runner_counts(campaign: &str) -> bool {
    let counts = hirsel_driver::native::counts();
    let fallbacks: Vec<String> = counts
        .fallbacks
        .iter()
        .map(|(reason, count)| format!("{reason} {count}"))
        .collect();
    println!(
        "{campaign}: {} runs as images in runners ({} images cached), fallbacks to executables: {}",
        counts.images,
        counts.cached,
        if fallbacks.is_empty() {
            "none".to_string()
        } else {
            fallbacks.join(", ")
        }
    );
    let unexpected: Vec<&String> = counts
        .fallbacks
        .keys()
        .filter(|reason| !matches!(reason.as_str(), "foreign" | "memcheck"))
        .collect();
    for reason in &unexpected {
        println!(
            "{campaign}: FAILED: a fallback other than by design: {}",
            counts.reasons[*reason]
        );
    }
    unexpected.is_empty()
}

/// An explicit smoke selection exercises the same programs on both backends.
/// Unfiltered smoke retains its smaller fixed Cranelift control set.
fn smoke_cranelift_cases(
    root: &Path,
    filter: &Filter,
    selected: &[Case],
) -> Result<Vec<Case>, String> {
    if filter.is_empty() {
        let mut cases = listed(root, native::cranelift::SMOKE)?;
        cases.extend(
            selected
                .iter()
                .filter(|case| case.id.starts_with("generated/"))
                .cloned(),
        );
        Ok(cases)
    } else {
        Ok(selected
            .iter()
            .filter(|case| filter.keeps(&case.id))
            .cloned()
            .collect())
    }
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
                    if let Some(reference) = &result.reference {
                        eprintln!("{what}: {} expected\n{}", result.id, reference.render());
                    }
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

/// `--jobs`, else the host's worker limit (`HIRSEL_JOBS`, default 2).
fn worker_count(explicit: Option<usize>) -> usize {
    explicit.unwrap_or_else(hirsel_guard::jobs)
}

/// `--trace-roots` (S-14): every executable baseline case (the root-anchor examples
/// `DROP-2-root-*` among them) with each engine tracing every live `Shared` root's count at
/// every scheduling point.
/// The interpreter's trace is the reference; optimized LLVM and Cranelift executables on
/// the diagnostic archive agree only when their traces (and observations) equal it.
fn trace_roots(root: &Path, jobs: usize, filter: &Filter, hash_seed: u64) -> Result<bool, String> {
    let matrix = seeded(hirsel_driver::native::trace_roots_matrix(), hash_seed);
    let all = native::baseline(root)?;
    let cases: Vec<Case> = all
        .iter()
        .filter(|c| filter.keeps(&c.id))
        .cloned()
        .collect();
    if cases.is_empty() {
        if filter.empty_fails(all.iter().map(|c| c.id.as_str())) {
            return Err(format!(
                "native trace-roots: --filter {:?} selected no cases",
                filter.0
            ));
        }
        println!("native trace-roots: this shard holds none of the selected cases");
        return Ok(true);
    }
    let toolchain = toolchain(&matrix)?;
    let results = native::run_cases_with(
        &cases,
        &native::scratch("trace-roots")?,
        jobs,
        |case, dir| case.run_traced(&matrix, &toolchain, dir),
    );
    let exclusions = native::read_exclusions(root)?;
    let ok = trace_agree("trace-roots", &results, &exclusions);
    println!(
        "native trace-roots: {} cases under {} configurations{}",
        results.len(),
        matrix.len(),
        if ok {
            " trace the same roots"
        } else {
            ": FAILED"
        }
    );
    Ok(ok)
}

/// `--migrate` (design §9 W1): every executable baseline case as optimized LLVM and
/// Cranelift executables on the optimized runtime archive, each run on several compute
/// workers with every task moving to another worker at every suspension. A configuration
/// agrees only when its observation equals the interpreter's, as without migration.
fn migrate(root: &Path, jobs: usize, filter: &Filter, hash_seed: u64) -> Result<bool, String> {
    let matrix = seeded(hirsel_driver::native::migrate_matrix(), hash_seed);
    let all = native::baseline(root)?;
    let cases: Vec<Case> = all
        .iter()
        .filter(|c| filter.keeps(&c.id))
        .cloned()
        .collect();
    if cases.is_empty() {
        if filter.empty_fails(all.iter().map(|c| c.id.as_str())) {
            return Err(format!(
                "native migrate: --filter {:?} selected no cases",
                filter.0
            ));
        }
        println!("native migrate: this shard holds none of the selected cases");
        return Ok(true);
    }
    let toolchain = toolchain(&matrix)?;
    let results = native::run_cases_with(
        &cases,
        &native::scratch("migrate")?,
        jobs,
        |case, dir| case.run_migrated(&matrix, &toolchain, dir),
    );
    let exclusions = native::read_exclusions(root)?;
    let (results, unjudged) = before_launch(results, &exclusions);
    let ok = results.is_empty() || trace_agree("migrate", &results, &exclusions);
    println!(
        "native migrate: {} cases under {} configurations{}; {unjudged} configurations fail \
         before their executables start",
        results.len(),
        matrix.len(),
        if ok { " agree under migration" } else { ": FAILED" }
    );
    Ok(ok)
}

/// Migration is the launched executable's (`HIRSEL_RT_MIGRATE`): what fails before it starts
/// (the reference snapshot, or a stage before launch) fails the same without it, so it is
/// reported and left unjudged, unless an exclusion row lists it. The results left, and how
/// many configurations were left unjudged.
fn before_launch(
    results: Vec<native::CaseResult>,
    exclusions: &native::Exclusions,
) -> (Vec<native::CaseResult>, usize) {
    let mut unjudged = 0;
    let mut left = Vec::new();
    for mut result in results {
        let listed = |configuration: &str| {
            exclusions.contains_key(&(result.id.clone(), configuration.to_string()))
        };
        if let Some(reason) = &result.snapshot
            && !listed(native::SNAPSHOT)
        {
            eprintln!("migrate: {}: unjudged, its snapshot fails: {reason}", result.id);
            unjudged += 1;
            continue;
        }
        let id = result.id.clone();
        result.outcomes.retain(|(name, _, outcome)| match outcome {
            Outcome::Unavailable(error)
                if error.stage != hirsel_driver::native::StageName::Launch
                    && !exclusions.contains_key(&(id.clone(), name.clone())) =>
            {
                eprintln!(
                    "migrate: {id} under {name}: unjudged, fails at {}: {}",
                    error.stage.name(),
                    error.message.lines().next().unwrap_or_default()
                );
                unjudged += 1;
                false
            }
            _ => true,
        });
        if result.snapshot.is_some() || !result.outcomes.is_empty() {
            left.push(result);
        }
    }
    (left, unjudged)
}

/// `--trace-roots`' judgement: every case traces the interpreter's roots under every
/// configuration, unless `native-tests/regressions/exclusions.tsv` lists exactly why it does
/// not, as the parity set's LLVM control is judged: the case still runs, its reason must
/// equal the row's, and a row whose case now agrees is stale. A disagreeing case also prints
/// a line led by its path, which names it to the gate.
fn trace_agree(
    label: &str,
    results: &[native::CaseResult],
    exclusions: &native::Exclusions,
) -> bool {
    if results.is_empty() {
        eprintln!("{label}: selected no cases");
        return false;
    }
    let mut ok = true;
    for result in results {
        let mut agrees = true;
        if result.snapshot.is_none() && result.outcomes.is_empty() {
            eprintln!("{label}: {}: ran under no configuration", result.id);
            agrees = false;
        }
        let found: std::collections::BTreeMap<String, native::Exclusion> = result
            .exclusions(&[Class::Llvm, Class::Cranelift])
            .into_iter()
            .collect();
        let listed =
            |configuration: &str| exclusions.get(&(result.id.clone(), configuration.to_string()));
        // As in the parity set, Cranelift is held to an LLVM control that ran: when the
        // LLVM configuration is unavailable and its exact row says so, the case's Cranelift
        // configurations are judged unavailable with it, needing no rows of their own.
        let control_excluded = result.outcomes.iter().any(|(name, class, outcome)| {
            *class == Class::Llvm
                && matches!(outcome, Outcome::Unavailable(_))
                && found.get(name).is_some_and(|f| listed(name) == Some(f))
        });
        let ran = std::iter::once((native::SNAPSHOT, None)).chain(
            result
                .outcomes
                .iter()
                .map(|(name, class, _)| (name.as_str(), Some(*class))),
        );
        for (configuration, class) in ran {
            let listed = listed(configuration);
            if control_excluded && class == Some(Class::Cranelift) && listed.is_none() {
                if found.contains_key(configuration) {
                    eprintln!(
                        "{label}: {} under {configuration}: unavailable with its excluded LLVM control",
                        result.id
                    );
                }
                continue;
            }
            match (found.get(configuration), listed) {
                (None, None) => {}
                (None, Some(_)) => {
                    eprintln!(
                        "{label}: {} under {configuration}: now agrees; delete its row in {}",
                        result.id,
                        native::EXCLUSIONS
                    );
                    agrees = false;
                }
                (Some(found), Some(listed)) if found == listed => eprintln!(
                    "{label}: {} under {configuration}: excluded by {}: {} ({})",
                    result.id,
                    native::EXCLUSIONS,
                    found.reason,
                    found.class
                ),
                (Some(found), listed) => {
                    agrees = false;
                    match result
                        .outcomes
                        .iter()
                        .find(|(name, _, _)| name == configuration)
                    {
                        Some((_, _, Outcome::Mismatch(actual))) => {
                            eprintln!(
                                "{label}: {} under {configuration} observed\n{}",
                                result.id,
                                actual.render()
                            );
                            if let Some(reference) = &result.reference {
                                eprintln!(
                                    "{label}: {} expected\n{}",
                                    result.id,
                                    reference.render()
                                );
                            }
                        }
                        _ => eprintln!(
                            "{label}: {} under {configuration}: {} ({}){}",
                            result.id,
                            found.reason,
                            found.class,
                            match listed {
                                Some(l) => format!("; listed as {} ({})", l.reason, l.class),
                                None => String::new(),
                            }
                        ),
                    }
                }
            }
        }
        if !agrees {
            eprintln!("{}: {label}: disagrees", result.id);
            ok = false;
        }
    }
    ok
}

fn run() -> Result<bool, String> {
    let mut mode = None;
    let mut jobs = None;
    let mut out = None;
    let mut filter = Filter::default();
    let mut bless = false;
    let mut clif = false;
    let mut aarch64 = false;
    let mut exhaustive = false;
    let mut seed = std::env::var("PROPTEST_RNG_SEED")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or_else(|| {
            u64::from_le_bytes(hirsel_driver::COMPILER_IDENTITY[..8].try_into().unwrap())
        });
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
                    | "cranelift-times" | "trace-roots" | "migrate" | "std-compile" | "corpus" | "bugs"),
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
                Long("case") => filter.2.push(args.value()?.string()?),
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
                Long("jobs") => jobs = Some(args.value()?.parse()?),
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
    let jobs = worker_count(jobs);
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
    if filter.1.is_some()
        && !matches!(
            mode.as_str(),
            "--full" | "--cranelift" | "--trace-roots" | "--migrate" | "--std-compile" | "--corpus"
        )
    {
        return Err(
            "--shard requires --full, --cranelift, --trace-roots, --migrate, --std-compile or --corpus"
                .into(),
        );
    }
    match mode.as_str() {
        "--bugs" => {
            let mut cases = native::generate::bug_cases();
            cases.retain(|c| filter.keeps(&c.id));
            if cases.is_empty() {
                return Err("bug generator: selected no combinations".into());
            }
            let pending =
                native::corpus::pending_rows(&root, native::generate::BUG_PENDING, &cases)?;
            let matrix = native::cranelift::matrix(TargetSpec::x86_64_unknown_linux_gnu());
            let tools = floor_toolchain(&matrix)?;
            let scratch = native::scratch("bugs")?;
            let results = native::run_cases(&cases, &matrix, &tools, &scratch, jobs);
            let report = native::judge_classes(
                &results,
                &pending,
                2,
                false,
                &[Class::Cranelift, Class::Llvm],
            );
            for error in report.unlisted.iter().chain(&report.stale) {
                eprintln!("native bug generator: {error}");
            }
            // Each new failure kind gets one line-minimized witness.
            let mut minimized = std::collections::BTreeSet::new();
            for (case, result) in cases.iter().zip(&results) {
                for (configuration, failure) in result.exclusions(&[Class::Llvm, Class::Cranelift])
                {
                    if pending.contains_key(&(case.id.clone(), configuration.clone()))
                        || !minimized.insert(format!("{}: {}", failure.class, failure.reason))
                    {
                        continue;
                    }
                    let original = (configuration.clone(), failure.clone());
                    let source = native::artifact::minimize(&case.source, |source| {
                        native::run_source(
                            &case.id,
                            source,
                            &matrix,
                            &tools,
                            &scratch.join("reduce"),
                        )
                        .exclusions(&[Class::Llvm, Class::Cranelift])
                        .contains(&original)
                    });
                    println!(
                        "{}",
                        serde_json::json!({"failing": case.id, "configuration": configuration, "class": failure.class, "reason": failure.reason, "minimized": source})
                    );
                }
            }
            println!(
                "native bug generator findings:\n{}",
                native::corpus::rows(&results)
            );
            Ok(report.ok())
        }
        "--corpus" => {
            let all = native::corpus::cases(&root)?;
            let omitted = native::corpus::omissions(
                &std::fs::read_to_string(root.join(native::corpus::OMIT))
                    .map_err(|e| e.to_string())?,
                &all,
            )?;
            let pending = native::corpus::pending_rows(&root, native::corpus::PENDING, &all)?;
            for (id, _) in pending.keys() {
                if omitted.contains_key(id) {
                    return Err(format!("{id}: pending cases must be executed, not omitted"));
                }
            }
            let rules = hirsel_conformance::spec_rules(&root)?;
            let selected: Vec<_> = all.into_iter().filter(|c| filter.keeps(&c.id)).collect();
            if selected.is_empty() {
                return Err("native corpus: selected no cases".into());
            }
            let selected_count = selected.len();
            let mut skipped = 0;
            let cases: Vec<_> = selected
                .into_iter()
                .filter(|c| {
                    if let Some(reason) = omitted.get(&c.id) {
                        println!("native corpus exclusion: {}: {reason}", c.id);
                        skipped += 1;
                        false
                    } else {
                        true
                    }
                })
                .collect();
            let matrix = native::cranelift::matrix(TargetSpec::x86_64_unknown_linux_gnu());
            let tools = floor_toolchain(&matrix)?;
            let scratch = native::scratch("corpus")?;
            let results = native::run_cases_with(&cases, &scratch, jobs, |case, dir| {
                native::corpus::run_case(case, seed, &tools, dir, &rules, &pending)
            });
            let report = native::judge_classes(
                &results,
                &pending,
                2,
                false,
                &[Class::Cranelift, Class::Llvm],
            );
            for error in report.unlisted.iter().chain(&report.stale) {
                eprintln!("native corpus: {error}");
            }
            let findings = native::corpus::rows(&results);
            if let Some(out) = out {
                std::fs::create_dir_all(&out).map_err(|e| e.to_string())?;
                std::fs::write(out.join("corpus-findings.tsv"), &findings)
                    .map_err(|e| e.to_string())?;
            }
            println!("native corpus findings:\n{findings}");
            println!(
                "{}",
                serde_json::json!({"selected":selected_count,"built":results.len(),"excluded":skipped,"llvm_slice":seed % 16,"unlisted":report.unlisted.len(),"stale":report.stale.len(),"shard":filter.1.map(|s| (s.index,s.count))})
            );
            Ok(report.ok())
        }
        "--std-compile" => {
            let started = std::time::Instant::now();
            let mut surface =
                hirsel_driver::native::StdSurface::load(&root.join("stdlib/std@1/hirsel.json"))?;
            for name in &surface.compile_time {
                eprintln!(
                    "std compile: {name}: comptime generator: native generated body for NativeCoverageSample {{ count: u64, name: String, flag: bool }}"
                );
            }
            let modules: Vec<_> = surface
                .modules
                .keys()
                .filter(|m| {
                    (filter.0.is_empty() || filter.0.iter().any(|text| text == *m))
                        && filter.1.as_ref().is_none_or(|shard| shard.holds(m))
                })
                .cloned()
                .collect();
            if modules.is_empty() {
                return Err("std compile: selected no modules".into());
            }
            let mut ok = true;
            // Modules share one engine, so a broken surface repeats its error.
            let mut first_failure: Option<(String, String)> = None;
            for module in modules {
                let before = std::time::Instant::now();
                let functions = surface.modules[&module].len();
                // A panicking stage names its module; the shared engine may be
                // inconsistent after it, so later modules still report.
                let compiled = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    surface.compile(&module)
                }))
                .unwrap_or_else(|panic| {
                    Err(format!(
                        "a native stage panicked: {}",
                        panic
                            .downcast_ref::<String>()
                            .map(String::as_str)
                            .or_else(|| panic.downcast_ref::<&str>().copied())
                            .unwrap_or("(no message)")
                    ))
                });
                match compiled {
                    Ok(units) => println!(
                        "std compile: {module}: {functions} public functions, {units} units, both backends, {:.3}s",
                        before.elapsed().as_secs_f64()
                    ),
                    Err(error) => {
                        match &first_failure {
                            Some((first, text)) if *text == error => {
                                eprintln!("std compile: {module}: the same failure as {first}")
                            }
                            _ => eprintln!("std compile: {module}: {error}"),
                        }
                        first_failure.get_or_insert((module.clone(), error));
                        ok = false;
                    }
                }
            }
            println!("std compile: {:.3}s", started.elapsed().as_secs_f64());
            Ok(ok)
        }
        "--smoke" => {
            // Both targets, every launch at the glibc floor (floor_toolchain).
            let mut matrix = smoke_matrix();
            matrix.push(Configuration {
                options: Options::new(TargetSpec::aarch64_unknown_linux_gnu()),
                engine: Engine::Native {
                    opt: OptLevel::Speed,
                    partition: Partition::Whole,
                    runtime: Runtime::Default,
                },
            });
            let matrix = seeded(matrix, hash_seed);
            let mut cases = if filter.is_empty() {
                let mut cases = listed(&root, "native-tests/regressions/smoke.txt")?;
                cases.extend(generated_cases(seed, 8, &filter));
                cases
            } else {
                let mut cases = native::cranelift::cases(&root)?;
                cases.extend(generated_cases(seed, 64, &filter));
                cases
            };
            cases.retain(|c| filter.keeps(&c.id));
            if cases.is_empty() {
                return Err(format!(
                    "native smoke: --filter {:?} selected no cases",
                    filter.0
                ));
            }
            if !filter.is_empty()
                && let Some(ok) = isolate::cases(&cases, jobs)?
            {
                return Ok(ok);
            }
            let results = native::run_cases(
                &cases,
                &matrix,
                &floor_toolchain(&matrix)?,
                &native::scratch("smoke")?,
                jobs,
            );
            let mut ok = all_agree("smoke", &results);
            // Real Io: each case is recorded on real Io and replayed from its
            // completion tape on simulated Io.
            let toolchain = floor_toolchain(&matrix)?;
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
                &smoke_cranelift_cases(&root, &filter, &cases)?,
                &cranelift,
                &toolchain,
                &native::scratch("smoke-cranelift")?,
                jobs,
            );
            ok &= all_agree("cranelift", &compiled);
            // The runner oracle: every case's native configurations, as linked executables and
            // as images in their runners, byte for byte, with cross-replayed tapes on x86-64.
            let oracle = native::run_cases_with(
                &cases,
                &native::scratch("smoke-oracle")?,
                jobs,
                |case, dir| {
                    native::oracle_source(
                        &case.id,
                        &case.source,
                        case.path.as_deref(),
                        &matrix,
                        &toolchain,
                        dir,
                    )
                },
            );
            ok &= all_agree("runner oracle", &oracle);
            ok &= runner_counts("native smoke");
            println!(
                "native smoke: {} cases under {} configurations and real Io record and replay, \
                 {} as Cranelift executables, runner and executable byte for byte{}",
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
            cranelift(&root, jobs, &filter, hash_seed, clif, target, seed)
        }
        "--cranelift-times" => cranelift_times(&root, jobs, &filter),
        "--trace-roots" => trace_roots(&root, jobs, &filter, hash_seed),
        "--migrate" => migrate(&root, jobs, &filter, hash_seed),
        "--sanitize" => {
            // The smoke and memcheck cases as optimized and unoptimized x86-64
            // executables on the system-allocator archive and on the size-class
            // archive with client requests under Valgrind memcheck: emitted code
            // and the runtime together.
            let matrix = sanitize_matrix(hash_seed);
            let cases = if filter.is_empty() {
                let mut cases = listed(&root, "native-tests/regressions/smoke.txt")?;
                cases.extend(listed(&root, "native-tests/regressions/memcheck.txt")?);
                cases.extend(generated_cases(seed, 8, &filter));
                cases
            } else {
                let mut cases = native::baseline(&root)?;
                cases.retain(|c| filter.keeps(&c.id));
                cases
            };
            let toolchain = toolchain(&matrix)?
                .under_memcheck()
                .map_err(|e| e.to_string())?;
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
    seed: u64,
) -> Result<bool, String> {
    use native::cranelift;
    let list = cranelift::parity_list(&target);
    let matrix = seeded(cranelift::matrix(target), hash_seed);
    let mut cases = cranelift::cases(root)?;
    cases.extend(generated_cases(seed, 64, filter));
    let empty_fails = filter.empty_fails(cases.iter().map(|c| c.id.as_str()));
    cases.retain(|c| filter.keeps(&c.id));
    if let Some(ok) = isolate::cases(&cases, jobs)? {
        return Ok(ok);
    }
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
    let llvm_exclusions = native::read_exclusions(root)?;
    let results = native::run_cases(
        &cases,
        &matrix,
        &toolchain(&matrix)?,
        &native::scratch("cranelift")?,
        jobs,
    );
    let report = cranelift::judge(&results, &listed, &llvm_exclusions);
    for failure in &report.llvm.unlisted {
        eprintln!(
            "cranelift: LLVM control: {failure}; requires a matching reason in {}",
            native::EXCLUSIONS
        );
    }
    for stale in &report.llvm.stale {
        eprintln!("cranelift: LLVM control: {stale} in {}", native::EXCLUSIONS);
    }
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
    // Name each failed LLVM control, including failures with matching exclusions.
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
        eprintln!("cranelift: LLVM control: {}: {why}", result.id);
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
    let ok = report.ok() && !(results.is_empty() && empty_fails);
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
    let mut generated = generated_cases(seed, 64, &filter);
    let mut partitioned = listed(root, "native-tests/topology/cases.txt")?;
    let empty_fails = filter.empty_fails(
        cases
            .iter()
            .chain(&generated)
            .chain(&partitioned)
            .map(|c| c.id.as_str()),
    );
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
    generated.retain(|c| filter.keeps(&c.id));
    let generated_results = native::run_cases_with(
        &generated,
        &native::scratch("generated")?,
        jobs,
        |case, dir| {
            let mut selected = native::diet::configurations(
                &matrix,
                &case.id,
                seed,
                exhaustive,
                &native::Exclusions::new(),
            );
            if engine.is_none() {
                selected.push(native::cranelift::cranelift(Options::new(
                    TargetSpec::x86_64_unknown_linux_gnu(),
                )));
            }
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
    // The runner oracle on both targets: each case's shipped-archive O3 program as a linked
    // executable and as an image in its runner, byte for byte, with cross-replayed tapes.
    let mut oracle_matrix: Vec<Configuration> = Vec::new();
    for configuration in &matrix {
        let shipped = matches!(
            configuration.engine,
            Engine::Native {
                opt: OptLevel::Speed,
                partition: Partition::Whole,
                runtime: Runtime::Default,
            }
        );
        if shipped
            && !oracle_matrix
                .iter()
                .any(|c| c.options.target == configuration.options.target)
        {
            oracle_matrix.push(configuration.clone());
        }
    }
    if !oracle_matrix.is_empty() {
        let oracle =
            native::run_cases_with(&cases, &native::scratch("oracle")?, jobs, |case, dir| {
                native::oracle_source(
                    &case.id,
                    &case.source,
                    case.path.as_deref(),
                    &oracle_matrix,
                    &toolchain,
                    dir,
                )
            });
        if whole || !cases.is_empty() {
            let agree = all_agree("runner oracle", &oracle);
            if agree {
                let targets: Vec<&str> = oracle_matrix
                    .iter()
                    .map(|c| c.options.target.triple.as_str())
                    .collect();
                println!(
                    "native full: runner oracle: {} cases on {}, runner and executable byte for byte agree",
                    oracle.len(),
                    targets.join(" and ")
                );
            }
            ok &= agree;
        }
    }
    ok &= runner_counts("native full");
    // A restriction may empty a campaign, but not the whole run; the
    // unrestricted run checks every campaign (all_agree refuses an empty one).
    if matrix.is_empty() || whole && cases.is_empty() {
        eprintln!("native: the matrix selected no cases or no configurations");
        ok = false;
    }
    if !complete
        && cases.is_empty()
        && partitioned.is_empty()
        && generated.is_empty()
        && empty_fails
    {
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
    fn filtered_smoke_uses_the_callers_programs_for_cranelift() {
        let cases = ["new-program.hir", "unrelated.hir", "generated/success-0"].map(|id| Case {
            id: id.into(),
            source: String::new(),
            path: None,
        });
        let filter = Filter(vec!["new-program.hir".into()], None, Vec::new());
        // The explicit case need not occur in the fixed smoke list. No list is
        // read, and unrelated programs must not be compiled on this backend.
        let selected = smoke_cranelift_cases(Path::new("no-smoke-list"), &filter, &cases).unwrap();
        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].id, "new-program.hir");

        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
        let mut fixed = listed(&root, native::cranelift::SMOKE).unwrap();
        fixed.push(cases[2].clone());
        let default = smoke_cranelift_cases(&root, &Filter::default(), &cases).unwrap();
        assert!(!default.is_empty());
        assert_eq!(
            default.iter().map(|c| &c.id).collect::<Vec<_>>(),
            fixed.iter().map(|c| &c.id).collect::<Vec<_>>()
        );
    }

    #[test]
    fn exact_gate_cases_do_not_expand_into_a_generated_window() {
        let filter = Filter(Vec::new(), None, vec!["generated/success-28".into()]);
        assert!(!filter.is_empty());
        assert!(filter.keeps("generated/success-28"));
        assert!(!filter.keeps("generated/success-2823603195720935296"));
        let cases = generated_cases(13590946479063591582, 64, &filter);
        let ids: Vec<_> = cases
            .iter()
            .filter(|case| filter.keeps(&case.id))
            .map(|case| case.id.as_str())
            .collect();
        assert_eq!(ids, ["generated/success-28"]);
    }

    #[test]
    fn a_repeated_filter_keeps_a_case_any_text_selects() {
        let filter = Filter(vec!["SEM-2".into(), "MEM-4".into()], None, Vec::new());
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
            .map(|i| Filter(Vec::new(), Shard::parse(&format!("{i}/4")), Vec::new()))
            .collect();
        assert_eq!(shards.iter().map(selected).sum::<usize>(), ids.len());
        for id in &ids {
            assert_eq!(shards.iter().filter(|s| s.keeps(id)).count(), 1, "{id}");
        }
        assert!(shards.iter().all(|s| selected(s) > 0 && s.is_empty()));
        let narrowed = Filter(vec!["SEM-1".into()], Shard::parse("1/4"), Vec::new());
        assert!(
            ids.iter()
                .filter(|id| narrowed.keeps(id))
                .all(|id| id.contains("SEM-1"))
        );
    }

    #[test]
    fn an_empty_shard_of_a_matching_filter_passes() {
        let ids = ["conformance/SEM/SEM-1-a.hir", "conformance/OWN/OWN-7-c.hir"];
        let one = |filter: Vec<&str>, shard: Option<&str>| {
            Filter(
                filter.into_iter().map(Into::into).collect(),
                shard.and_then(Shard::parse),
                Vec::new(),
            )
        };
        let holder = (1..=8)
            .find(|i| Shard::parse(&format!("{i}/8")).unwrap().holds(ids[0]))
            .unwrap();
        let other = format!("{}/8", holder % 8 + 1);
        // A shard that holds none of the cases the filter selects elsewhere.
        let empty = one(vec!["SEM-1"], Some(&other));
        assert!(!ids.iter().any(|id| empty.keeps(id)));
        assert!(!empty.empty_fails(ids.into_iter()));
        // A filter that selects nothing in any shard still fails, sharded or not.
        assert!(one(vec!["NOPE"], Some(&other)).empty_fails(ids.into_iter()));
        assert!(one(vec!["NOPE"], None).empty_fails(ids.into_iter()));
        // An unsharded run that selected nothing fails.
        assert!(one(vec!["SEM-1"], None).empty_fails(ids.into_iter()));
        // An unfiltered shard of a nonempty corpus may be empty.
        assert!(!one(Vec::new(), Some(&other)).empty_fails(ids.into_iter()));
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
