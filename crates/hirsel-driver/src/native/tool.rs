//! `hirsel run --native`: the checked program, optimized, run as an image in the host's
//! runner (or, when it calls C, as an executable).
use super::observe::SiteTable;
use super::program_cache::{LaunchFacts, ProgramCache};
use super::{
    NativeError, OptLevel, Options, Partition, Sites, StageName, TargetSpec, Toolchain, build,
};
use hirsel_engine::{RunOutcome, ToolError};
use std::io::Write;
use std::path::Path;

/// Output destinations for a user launch. Inheriting the two OS streams preserves
/// the program's cross-stream write order; custom writers receive each stream.
pub enum Output<'a> {
    Inherit {
        json: bool,
    },
    Streams {
        stdout: &'a mut dyn Write,
        stderr: &'a mut dyn Write,
    },
}

/// Check `path` exactly as `run` does (the same first static diagnostics),
/// then build it for x86-64 Linux with every optimization, link it against
/// the in-tree runtime and launch it under the empty-grant record, as a launch
/// without a record. The executable's output is written to `stdout` and its
/// failure chain to `stderr`.
///
/// The outer error is a source or tool error of the checker; the inner one is
/// a native tool outcome (a stage does not support the program yet), which is
/// never a verdict about the source.
pub fn run_path_native(
    path: &Path,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> Result<Result<RunOutcome, NativeError>, ToolError> {
    run_path_native_with_config(
        path,
        crate::HostLaunch::empty(),
        None,
        Output::Streams { stdout, stderr },
    )
}

/// `run_path_native` with a host launch and PS-12 evidence; the proof report goes to
/// `stderr` before the program runs. Record and replay pass to the executable as
/// `--hirsel-record` and `--hirsel-replay`; the driver binds the run first, so a
/// recording of other code refuses before building.
pub fn run_path_native_with_config(
    path: &Path,
    launch: crate::HostLaunch<'_>,
    evidence: Option<&super::Evidence>,
    output: Output<'_>,
) -> Result<Result<RunOutcome, NativeError>, ToolError> {
    run_path_native_backend(
        path,
        launch,
        evidence,
        super::Backend::Llvm,
        output,
        hirsel_engine::Verify::On,
    )
}

/// `run_path_native_with_config` with `backend`: LLVM builds at O3, Cranelift
/// unoptimized, for a fast debug loop.
pub fn run_path_native_backend(
    path: &Path,
    launch: crate::HostLaunch<'_>,
    evidence: Option<&super::Evidence>,
    backend: super::Backend,
    output: Output<'_>,
    verify: hirsel_engine::Verify,
) -> Result<Result<RunOutcome, NativeError>, ToolError> {
    let (mut host_stdout, mut host_stderr) = (std::io::stdout(), std::io::stderr());
    let (inherit, json, stdout, stderr): (bool, bool, &mut dyn Write, &mut dyn Write) = match output
    {
        Output::Inherit { json } => (true, json, &mut host_stdout, &mut host_stderr),
        Output::Streams { stdout, stderr } => (false, false, stdout, stderr),
    };
    // An unchanged program's live launch without a recording runs its cached image.
    let cache = match launch {
        crate::HostLaunch::Live { record: None, .. } if evidence.is_none() => ProgramCache::open(),
        _ => None,
    };
    let program = cache.as_ref().and_then(|_| {
        crate::COMPILER
            .with_verify(verify)
            .program_identity(path)
            .ok()
            .flatten()
            .map(|identity| super::program_cache::hex(&identity.0))
    });
    if let (
        Some(cache),
        Some(program),
        crate::HostLaunch::Live {
            config,
            revocations,
            ..
        },
    ) = (&cache, &program, launch)
    {
        let cached = cached(
            cache,
            program,
            config,
            revocations,
            backend,
            inherit,
            json,
            &mut *stdout,
            &mut *stderr,
        )?;
        hirsel_intern::timings::count(match cached {
            Some(_) => hirsel_intern::timings::Count::ProgramHits,
            None => hirsel_intern::timings::Count::ProgramMisses,
        });
        if let Some(outcome) = cached {
            return Ok(outcome);
        }
    }
    let mut loaded = crate::COMPILER.with_verify(verify).load_program(path)?;
    loaded.check_for_execution()?;
    if matches!(
        launch,
        crate::HostLaunch::Replay(_)
            | crate::HostLaunch::Live {
                record: Some(_),
                ..
            }
    ) {
        loaded.refuse_unreplayable()?;
    }
    // The driver binds only: the executable admits the run, once (design.md).
    let host = match launch {
        crate::HostLaunch::Live {
            config,
            record,
            revocations,
        } => {
            let live = crate::HostLaunch::Live {
                config,
                record: None,
                revocations,
            };
            let crate::BoundHostRun::Live(bound) = crate::bind_launch(&mut loaded, live)? else {
                unreachable!("a live launch binds live")
            };
            // The executable records; the level and the destination must agree here first.
            hirsel_rt::host_launch::recording_level(bound.run(), record.is_some())
                .map_err(|e| ToolError::Runtime(format!("launch: {e}")))?;
            (
                bound.into_run(),
                super::link::Tape::Live {
                    record,
                    revocations,
                },
            )
        }
        launch @ crate::HostLaunch::Replay(tape) => {
            let crate::BoundHostRun::Replay { run, .. } = crate::bind_launch(&mut loaded, launch)?
            else {
                unreachable!("a replay binds a recording")
            };
            (run, super::link::Tape::Replay(tape))
        }
        crate::HostLaunch::Callee { .. } => {
            return Err(ToolError::Runtime(
                "launch: a runner host runs its callees interpreted".into(),
            ));
        }
    };
    // What a later launch of this program binds against, to store with its image.
    let store = match (cache, program) {
        (Some(cache), Some(program)) => LaunchFacts::of(&mut loaded).ok().map(|facts| Store {
            cache,
            program,
            facts,
        }),
        _ => None,
    };
    let entry = loaded.entry;
    let mut engine = loaded.engine;
    let query = |engine: &hirsel_engine::QueryEngine, error| ToolError::Query {
        error,
        phase: engine.unavailable,
        sources: engine.sources.clone(),
    };
    let input = match engine.export_native_input() {
        Ok(input) => input,
        Err(error) => return Err(query(&engine, error)),
    };
    let validated = match engine.import_native_input(input) {
        Ok(validated) => validated,
        Err(error) => {
            return Ok(Err(NativeError::new(
                StageName::Manifest,
                format!("the native snapshot: {error:?}"),
            )));
        }
    };
    let proofs = match evidence
        .map(|e| super::Proofs::assess(&mut engine, &entry, &validated, e))
        .transpose()
    {
        Ok(proofs) => proofs,
        Err(error) => return Ok(Err(error)),
    };
    let observed = run(
        &validated,
        &entry,
        (&host.0, host.1),
        proofs.as_ref(),
        backend,
        store.as_ref(),
        inherit,
        stdout,
        stderr,
    );
    finish(observed, &Sites::new(validated.snapshot()), json, stderr)
}

/// A launch's observation as `run --native` reports it: each failure on `stderr`,
/// then the outcome.
fn finish(
    observed: Result<super::link::Observed, NativeError>,
    sites: &Sites<'_>,
    json: bool,
    stderr: &mut dyn Write,
) -> Result<Result<RunOutcome, NativeError>, ToolError> {
    let observation = match observed {
        Ok(super::link::Observed::Ran(observation, waits)) => Ok((observation, waits)),
        // The executable admitted the run and refused it: the same refusal the interpreter's
        // admission reports.
        Ok(super::link::Observed::Refused(reason)) => {
            return Err(match hirsel_run_schema::RunRefusal::parse(&reason) {
                Some(refusal) => ToolError::Launch(refusal),
                None => ToolError::Runtime(format!("launch: {reason}")),
            });
        }
        Ok(super::link::Observed::Stopped(outcome)) => return Ok(Ok(outcome)),
        Err(error) => Err(error),
    };
    Ok(observation.and_then(|(observation, waits)| {
        let io = |e: std::io::Error| NativeError::new(StageName::Launch, e.to_string());
        for failure in &observation.failures {
            writeln!(stderr, "{}", sites.report(failure, json)?).map_err(io)?;
        }
        observation.outcome(sites, waits)
    }))
}

/// The program cache entries a fresh build of a run fills.
struct Store {
    cache: ProgramCache,
    program: String,
    facts: LaunchFacts,
}

fn facts_entry(program: &str) -> String {
    format!("f-{program}.json")
}

fn sites_entry(program: &str) -> String {
    format!("s-{program}.json")
}

/// The image entry of `program` linked for `run` and `runner` with `toolchain`.
fn image_entry(
    program: &str,
    backend: super::Backend,
    run: &hirsel_run_config::ValidatedRun,
    toolchain: &Toolchain,
    runner: &super::runner::Runner,
    target: &TargetSpec,
) -> Result<String, NativeError> {
    let toolchain = super::program_cache::toolchain_identity(toolchain, &target.triple)?;
    let fuel = run.budgets().is_some_and(|b| b.fuel.is_some());
    let key = super::program_cache::key(
        "hirsel.program-cache.image.v1",
        [
            program.as_bytes(),
            backend.name().as_bytes(),
            target.triple.as_bytes(),
            &[u8::from(fuel)],
            run.canonical(),
            &runner.build_id,
            &toolchain.0,
        ],
    );
    Ok(format!("i-{key}.so"))
}

/// Run `program`'s cached image for a live launch of `config`, or `None` when the cache
/// cannot serve it, before anything runs: no facts, sites or image for this program, run
/// and runner, or a toolchain that runs executables.
#[allow(clippy::too_many_arguments)]
fn cached(
    cache: &ProgramCache,
    program: &str,
    config: crate::LaunchConfig<'_>,
    revocations: Option<&Path>,
    backend: super::Backend,
    inherit: bool,
    json: bool,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> Result<Option<Result<RunOutcome, NativeError>>, ToolError> {
    let Some(facts) = cache
        .read(&facts_entry(program))
        .and_then(|bytes| LaunchFacts::decode(&bytes))
    else {
        return Ok(None);
    };
    let Some(table) = cache
        .read(&sites_entry(program))
        .and_then(|bytes| SiteTable::decode(&bytes))
    else {
        return Ok(None);
    };
    let bound = facts.bind(config)?;
    hirsel_rt::host_launch::recording_level(bound.run(), false)
        .map_err(|e| ToolError::Runtime(format!("launch: {e}")))?;
    let run = bound.into_run();
    let target = TargetSpec::x86_64_unknown_linux_gnu();
    let directory = std::env::var_os("HIRSEL_NATIVE_CACHE")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(super::runtime_cache::default_directory);
    let image = (|| {
        let toolchain = Toolchain::in_tree(&directory, &[&target]).ok()?;
        if toolchain.execution == super::runner::Execution::Executable {
            return None;
        }
        let runner = super::runner::runner(&toolchain, &target).ok()?;
        let entry = image_entry(program, backend, &run, &toolchain, &runner, &target).ok()?;
        let path = cache.get(&entry)?;
        let key = runner.build_id;
        Some((
            toolchain,
            super::runner::Image {
                path,
                key,
                runner,
                cached: true,
            },
        ))
    })();
    let Some((toolchain, image)) = image else {
        return Ok(None);
    };
    let scratch = std::env::temp_dir().join(format!("hirsel-native-{}", std::process::id()));
    let io = |e: std::io::Error| NativeError::new(StageName::Link, e.to_string());
    let observed = std::fs::create_dir_all(&scratch)
        .map_err(io)
        .and_then(|()| {
            super::link::write_run(&scratch, &run)?;
            let launched = super::link::launch_live(
                &super::link::Runnable::Image(image),
                &target,
                &toolchain,
                &scratch,
                super::link::Tape::Live {
                    record: None,
                    revocations,
                },
                super::link::LiveRun {
                    run: &run,
                    inherit,
                    stdout,
                    stderr: &mut *stderr,
                },
            )?;
            let sites = Sites::table(&table);
            super::link::observe_codes(&launched, &table.rules, &table.codes, &sites)
        });
    let _ = std::fs::remove_dir_all(&scratch);
    finish(observed, &Sites::table(&table), json, stderr).map(Some)
}

#[allow(clippy::too_many_arguments)]
fn run(
    validated: &hirsel_engine::ValidatedNativeInput,
    entry: &hirsel_engine::DefPath,
    (run, tape): (&hirsel_run_config::ValidatedRun, super::link::Tape<'_>),
    proofs: Option<&super::Proofs>,
    backend: super::Backend,
    store: Option<&Store>,
    inherit: bool,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> Result<super::link::Observed, NativeError> {
    let target = TargetSpec::x86_64_unknown_linux_gnu();
    let mut options = Options::new(target.clone());
    if let Some(proofs) = proofs {
        options.discharge = proofs.discharge.clone();
    }
    let mut program = build(validated, &options)?;
    // A fuel budget needs fuel checks; without one the executable pays nothing.
    program.fuel = run.budgets().is_some_and(|b| b.fuel.is_some());
    for line in proofs.map_or_else(Vec::new, |p| p.report(&program)) {
        writeln!(stderr, "hirsel: {line}")
            .map_err(|e| NativeError::new(StageName::Launch, e.to_string()))?;
    }
    let scratch = std::env::temp_dir().join(format!("hirsel-native-{}", std::process::id()));
    let cache = std::env::var_os("HIRSEL_NATIVE_CACHE")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(super::runtime_cache::default_directory);
    let io = |e: std::io::Error| NativeError::new(StageName::Link, e.to_string());
    std::fs::create_dir_all(&scratch).map_err(io)?;
    let result = (|| {
        let toolchain = Toolchain::in_tree(&cache, &[&target])?;
        let opt = match backend {
            super::Backend::Llvm => OptLevel::Speed,
            super::Backend::Cranelift => OptLevel::None,
        };
        let objects = super::link::emit_backend_objects(
            &program,
            backend,
            opt,
            Partition::Whole,
            false,
            None,
        )?;
        let runnable =
            super::link::link_runnable(&program, entry, &objects, &toolchain, &scratch, Some(run))?;
        let sites = Sites::new(validated.snapshot());
        // Images are cached; a program that runs as an executable is not.
        if let (Some(store), super::link::Runnable::Image(image)) = (store, &runnable) {
            let entry = image_entry(
                &store.program,
                backend,
                run,
                &toolchain,
                &image.runner,
                &target,
            )?;
            store.cache.put_file(&entry, &image.path);
            store.cache.put(
                &sites_entry(&store.program),
                &SiteTable::new(&program, &sites).encode(),
            );
            store
                .cache
                .put(&facts_entry(&store.program), &store.facts.encode());
        }
        let launched = super::link::launch_live(
            &runnable,
            &target,
            &toolchain,
            &scratch,
            tape,
            super::link::LiveRun {
                run,
                inherit,
                stdout,
                stderr,
            },
        )?;
        let observation = super::link::observe_run(&launched, &program, &sites)?;
        Ok(observation)
    })();
    let _ = std::fs::remove_dir_all(&scratch);
    result
}
