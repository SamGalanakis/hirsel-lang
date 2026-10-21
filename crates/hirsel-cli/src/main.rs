// The memory guard over mimalloc, which serves the compiler's many small,
// short-lived values faster than the system allocator, counting live bytes
// under `--mem-usage`.
hirsel_guard::install!(allocator::Counting: allocator::Counting);

use hirsel_cli_args::lexopt::{self, Arg::*, Parser};
use hirsel_engine::{QueryError, RunOutcome, ToolError, Verify};
use std::{env, ffi::OsString, path::Path, process::ExitCode};

const USAGE: &str = "usage: hirsel [--verbose] [--timings[=json]] [--mem-usage] check [--json | --safe] <source|manifest.json> | test [--no-verify] [--json] [--seed <u64>] [--scheduler <name@1:seed> | --explore <n> [--scheduler <name@1>]] [--race] [--sim-points <root-release,atomic-store>] [--bless | --bench [--bench-time <ms>]] [--max-steps <n|unbounded> | --native [--backend llvm|cranelift]] <manifest.json> [filter...] | run [--validate-stages] [--no-verify] [--json] [--native [--backend llvm|cranelift] [--proofs <sidecar> [--trust <tool@version>]...] | --simulation <json>] <source|manifest.json> [(--run <json> | --grants <json>) [--record <tape>] [--revocations <json>] | --replay <tape>] [--max-steps <n|unbounded>] [--cores <n>] [-- args...] | build [--validate-stages] [--no-verify] [--release | --debug] [--lto] [--backend llvm|cranelift] [--fuel] [--target x86_64-linux | aarch64-linux] [--proofs <sidecar> [--trust <tool@version>]...] <source|manifest.json> -o <exe> | fmt [--check] <file> | expand <source|manifest.json> | hash <source|manifest.json> | export --tir|--run <source|manifest.json> -o <file> | deps card <manifest.json> [--package name@ver[/module]] [--code-store config.json] [--json] | doc <manifest.json> [-o <out.md>] [--json] | view <tape> --capture <id> -o <file> [--perfetto | --source <program> --analysis <id>] [--timing <sidecar>] | image <package manifest.json> -o <file> | seed-objects <package manifest.json> -o <directory> | tapes check <tape>...";

mod allocator;
mod execution_policy;
mod measure;
mod serve;
use execution_policy::{DEFAULT_BENCH_MILLISECONDS, DEFAULT_RUN_MAX_STEPS};

fn main() -> ExitCode {
    hirsel_engine::persistent_results(true);
    let start = std::time::Instant::now();
    hirsel_engine::with_compiler_stack(|| {
        ExitCode::from(execute(env::args_os().skip(1).collect(), start))
    })
}
enum Command {
    Help,
    Test {
        manifest: OsString,
        filters: Vec<String>,
        seed: Option<u64>,
        json: bool,
        native: bool,
        bless: bool,
        max_steps: Option<u64>,
        /// `--bench` and its target time per bench.
        bench: Option<std::time::Duration>,
        backend: hirsel_driver::native::Backend,
        schedules: hirsel_driver::testing::Schedules,
        simulation_debug: hirsel_driver::testing::SimulationDebug,
    },
    Image {
        manifest: OsString,
        output: OsString,
    },
    SeedObjects {
        manifest: OsString,
        output: OsString,
    },
    /// `tapes check <tape>...`: the joined check over linked tapes (RUN-11).
    Tapes {
        tapes: Vec<OsString>,
    },
    DepsCard {
        manifest: OsString,
        package: Option<String>,
        code_store: Option<OsString>,
        json: bool,
    },
    Doc {
        manifest: OsString,
        output: Option<OsString>,
        json: bool,
    },
    Build {
        source: OsString,
        output: OsString,
        target: hirsel_driver::native::TargetSpec,
        profile: hirsel_driver::native::BuildProfile,
        backend: hirsel_driver::native::Backend,
        proofs: Proofs,
        fuel: bool,
        validation: hirsel_driver::native::Validation,
    },
    View {
        tape: OsString,
        capture: String,
        output: OsString,
        perfetto: bool,
        timing: Option<OsString>,
        source: Option<OsString>,
        analysis: Option<String>,
    },
    Export {
        run: bool,
        entry: OsString,
        output: OsString,
    },
    Simulation {
        config: OsString,
        file: OsString,
        max_steps: Option<u64>,
        json: bool,
    },
    Run {
        file: OsString,
        launch: Launch,
        native: bool,
        validation: hirsel_driver::native::Validation,
        backend: Option<hirsel_driver::native::Backend>,
        proofs: Proofs,
        arguments: Option<Vec<OsString>>,
        max_steps: Option<u64>,
        json: bool,
    },
    Source {
        command: &'static str,
        flag: Option<&'static str>,
        file: OsString,
    },
}

/// A host launch: a run record or grants template, optionally recording its tape, or
/// the replay of a tape, which binds its own run.
enum Launch {
    Live {
        grants: bool,
        cores: Option<u64>,
        config: OsString,
        record: Option<OsString>,
        revocations: Option<OsString>,
    },
    Replay(OsString),
}
impl Launch {
    fn host<'a>(&'a self, arguments: Option<&'a [OsString]>) -> hirsel_driver::HostLaunch<'a> {
        match self {
            Self::Live {
                grants,
                cores,
                config,
                record,
                revocations,
            } => hirsel_driver::HostLaunch::Live {
                config: if let Some(cores) = cores {
                    hirsel_driver::LaunchConfig::Cores {
                        input: config,
                        grants: *grants,
                        arguments,
                        cores: *cores,
                    }
                } else if *grants {
                    hirsel_driver::LaunchConfig::Grants {
                        input: config,
                        arguments,
                    }
                } else {
                    hirsel_driver::LaunchConfig::Record(config)
                },
                record: record.as_deref().map(Path::new),
                revocations: revocations.as_deref().map(Path::new),
            },
            Self::Replay(tape) => hirsel_driver::HostLaunch::Replay(Path::new(tape)),
        }
    }
}

/// Harness subprocesses inherit the same policy as `--validate-stages`.
fn validation(stages: bool) -> hirsel_driver::native::Validation {
    if stages || env::var_os("HIRSEL_VALIDATE_STAGES").is_some_and(|v| v == "1") {
        hirsel_driver::native::Validation::EveryStage
    } else {
        hirsel_driver::native::Validation::Ready
    }
}

/// `--backend llvm|cranelift`: the code generator a native build lowers its LIR with.
fn backend(value: OsString) -> Result<hirsel_driver::native::Backend, lexopt::Error> {
    use hirsel_driver::native::Backend;
    match value.to_str() {
        Some("llvm") => Ok(Backend::Llvm),
        Some("cranelift") => Ok(Backend::Cranelift),
        _ => Err("--backend must be llvm or cranelift".into()),
    }
}

/// `--proofs <sidecar>` and the checkers `--trust` names (PS-12, PS-15).
#[derive(Default)]
struct Proofs {
    sidecar: Option<OsString>,
    trusted: Vec<hirsel_driver::native::Checker>,
}
impl Proofs {
    fn trust(&mut self, value: OsString) -> Result<(), lexopt::Error> {
        self.trusted.push(
            value
                .to_str()
                .and_then(hirsel_driver::native::Checker::parse)
                .ok_or("--trust requires tool@version")?,
        );
        Ok(())
    }
    fn check(&self) -> Result<(), lexopt::Error> {
        if self.sidecar.is_none() && !self.trusted.is_empty() {
            return Err("--trust requires --proofs".into());
        }
        Ok(())
    }
    fn evidence(self) -> Result<Option<hirsel_driver::native::Evidence>, ToolError> {
        self.sidecar
            .map(|path| hirsel_driver::native::Evidence::read(Path::new(&path), self.trusted))
            .transpose()
    }
}

/// `--max-steps <n|unbounded>`: a tooling step budget, None when unbounded.
fn step_budget(value: Option<OsString>) -> Result<Option<u64>, lexopt::Error> {
    const USAGE: &str = "--max-steps requires an unsigned integer or unbounded";
    match value.as_deref().and_then(|v| v.to_str()) {
        Some("unbounded") => Ok(None),
        Some(value) => value
            .parse::<u64>()
            .map(Some)
            .map_err(|_| lexopt::Error::from(USAGE)),
        None => Err(USAGE.into()),
    }
}

fn parse_command(args: Vec<OsString>) -> Result<Command, lexopt::Error> {
    let mut args = Parser::from_args(args);
    let command = match args.next()? {
        Some(Long("help")) => Command::Help,
        Some(Value(command)) if command == "help" => Command::Help,
        Some(Value(command)) if command == "view" => {
            let (mut tape, mut capture, mut output) = (None, None, None);
            let (mut timing, mut source, mut analysis) = (None, None, None);
            let mut perfetto = false;
            while let Some(arg) = args.next()? {
                match arg {
                    Long("capture") if capture.is_none() => {
                        capture = Some(
                            args.value()?
                                .into_string()
                                .map_err(|_| "capture must be UTF-8")?,
                        )
                    }
                    Short('o') if output.is_none() => output = Some(args.value()?),
                    Long("perfetto") if !perfetto => perfetto = true,
                    Long("timing") if timing.is_none() => timing = Some(args.value()?),
                    Long("source") if source.is_none() => source = Some(args.value()?),
                    Long("analysis") if analysis.is_none() => {
                        analysis = Some(
                            args.value()?
                                .into_string()
                                .map_err(|_| "analysis must be UTF-8")?,
                        )
                    }
                    Value(value) if tape.is_none() => tape = Some(value),
                    arg => return Err(arg.unexpected()),
                }
            }
            if source.is_some() != analysis.is_some() {
                return Err("view --source and --analysis must be supplied together".into());
            }
            if perfetto && source.is_some() {
                return Err("replay enrichment is available in the normalized view".into());
            }
            Command::View {
                tape: tape.ok_or("view requires a recording")?,
                capture: capture.ok_or("view requires an archival --capture identity")?,
                output: output.ok_or("view requires -o <file>")?,
                perfetto,
                timing,
                source,
                analysis,
            }
        }
        Some(Value(command)) if command == "test" => {
            let (mut manifest, mut filters, mut seed, mut json, mut native, mut selected) =
                (None, Vec::new(), None, false, false, None);
            let (mut bless, mut max_steps, mut bench, mut bench_time) = (false, None, false, None);
            let mut simulation_debug = hirsel_driver::testing::SimulationDebug::default();
            let mut points_seen = false;
            let (mut scheduler, mut explore) = (None::<String>, None);
            while let Some(arg) = args.next()? {
                match arg {
                    Long("race") if !simulation_debug.races => simulation_debug.races = true,
                    Long("sim-points") if !points_seen => {
                        points_seen = true;
                        let points = args.value()?;
                        let points = points.to_str().and_then(hirsel_driver::testing::SimulationDebug::parse_points)
                            .ok_or("--sim-points requires root-release, atomic-store, or both comma-separated")?;
                        simulation_debug.root_releases = points.root_releases;
                        simulation_debug.atomic_stores = points.atomic_stores;
                    }
                    Long("scheduler") if scheduler.is_none() => {
                        scheduler = Some(
                            args.value()?
                                .into_string()
                                .map_err(|_| "--scheduler must be UTF-8")?,
                        );
                    }
                    Long("explore") if explore.is_none() => {
                        explore = Some(
                            args.value()?
                                .to_str()
                                .and_then(|v| v.parse::<u64>().ok())
                                .filter(|&n| n > 0)
                                .ok_or("--explore requires a positive number of schedules")?,
                        );
                    }
                    Long("json") if !json => json = true,
                    Long("bless") if !bless => bless = true,
                    Long("bench") if !bench => bench = true,
                    Long("bench-time") if bench_time.is_none() => {
                        bench_time = Some(
                            args.value()?
                                .to_str()
                                .and_then(|v| v.parse::<u64>().ok())
                                .filter(|&ms| ms > 0)
                                .ok_or("--bench-time requires a positive number of milliseconds")?,
                        );
                    }
                    Long("max-steps") if max_steps.is_none() => {
                        max_steps = Some(step_budget(args.value().ok())?);
                    }
                    Long("native") if !native => native = true,
                    Long("backend") if selected.is_none() => {
                        selected = Some(backend(args.value()?)?)
                    }
                    Long("seed") if seed.is_none() => {
                        seed = Some(
                            args.value()?
                                .to_str()
                                .and_then(|v| v.parse::<u64>().ok())
                                .ok_or("--seed requires a u64")?,
                        );
                    }
                    Value(value) if manifest.is_none() => manifest = Some(value),
                    Value(value) => filters.push(
                        value
                            .into_string()
                            .map_err(|_| "test filters must be UTF-8")?,
                    ),
                    arg => return Err(arg.unexpected()),
                }
            }
            if (native || bench) && simulation_debug != Default::default() {
                return Err("--race and --sim-points apply only to interpreted tests".into());
            }
            if selected.is_some() && !native {
                return Err("--backend requires --native".into());
            }
            if native && max_steps.is_some() {
                return Err("--max-steps applies only to interpreted tests".into());
            }
            if bench && (bless || max_steps.is_some()) {
                return Err("--bench takes neither --bless nor --max-steps".into());
            }
            if bench_time.is_some() && !bench {
                return Err("--bench-time requires --bench".into());
            }
            if bench && (scheduler.is_some() || explore.is_some()) {
                return Err("--bench takes neither --scheduler nor --explore".into());
            }
            use hirsel_driver::testing::{Scheduler, Schedules};
            let schedules = match (explore, scheduler) {
                (None, None) => Schedules::Default,
                (None, Some(text)) => Schedules::One(Scheduler::parse(&text).ok_or(
                    "--scheduler requires fifo@1, random@1 or pct@1 and a u64 seed, as random@1:7",
                )?),
                (Some(count), text) => Schedules::Explore {
                    policy: Scheduler::parse(&format!(
                        "{}:0",
                        text.as_deref().unwrap_or("random@1")
                    ))
                    .filter(|policy| {
                        policy.algorithm != "fifo"
                            && text.as_deref().is_none_or(|t| !t.contains(':'))
                    })
                    .ok_or("--explore takes --scheduler random@1 or pct@1, without a seed")?,
                    count,
                },
            };
            Command::Test {
                simulation_debug,
                schedules,
                max_steps: max_steps.unwrap_or(Some(DEFAULT_RUN_MAX_STEPS)),
                bench: bench.then(|| {
                    std::time::Duration::from_millis(
                        bench_time.unwrap_or(DEFAULT_BENCH_MILLISECONDS),
                    )
                }),
                manifest: manifest.ok_or("test requires a package manifest")?,
                filters,
                seed,
                json,
                native,
                bless,
                backend: selected.unwrap_or_default(),
            }
        }
        Some(Value(command)) if command == "build" => {
            use hirsel_driver::native::{Backend, BuildProfile, TargetSpec};
            let (mut source, mut output, mut target, mut profile) = (None, None, None, None);
            let mut selected = None;
            let mut proofs = Proofs::default();
            let mut fuel = false;
            let mut validate_stages = false;
            let mut lto = false;
            while let Some(arg) = args.next()? {
                match arg {
                    Long("proofs") if proofs.sidecar.is_none() => {
                        proofs.sidecar = Some(args.value()?)
                    }
                    Long("trust") => proofs.trust(args.value()?)?,
                    Long("fuel") if !fuel => fuel = true,
                    Long("validate-stages") if !validate_stages => validate_stages = true,
                    Long("lto") if !lto => lto = true,
                    Long("release") if profile.is_none() => profile = Some(BuildProfile::Release),
                    Long("debug") if profile.is_none() => profile = Some(BuildProfile::Debug),
                    Long("backend") if selected.is_none() => {
                        selected = Some(backend(args.value()?)?)
                    }
                    Long("target") if target.is_none() => {
                        let value = args.value()?;
                        target = Some(match value.to_str() {
                            Some("x86_64-linux") => TargetSpec::x86_64_unknown_linux_gnu(),
                            Some("aarch64-linux") => TargetSpec::aarch64_unknown_linux_gnu(),
                            _ => return Err("unsupported build target".into()),
                        });
                    }
                    Short('o') if output.is_none() => output = Some(args.value()?),
                    Value(file) if source.is_none() && !file.to_string_lossy().starts_with('-') => {
                        source = Some(file)
                    }
                    arg => return Err(arg.unexpected()),
                }
            }
            if lto {
                if profile == Some(BuildProfile::Debug) {
                    return Err("--lto requires LLVM optimization; remove --debug".into());
                }
                profile = Some(BuildProfile::ReleaseLto);
            }
            if matches!(
                profile,
                Some(BuildProfile::Release | BuildProfile::ReleaseLto)
            ) && selected == Some(Backend::Cranelift)
            {
                return Err(if lto {
                    "--lto requires LLVM; remove --backend cranelift or --lto"
                } else {
                    "release builds use LLVM; remove --backend cranelift or --release"
                }
                .into());
            }
            Command::Build {
                source: source.ok_or("build requires a source or manifest")?,
                output: output.ok_or("build requires -o")?,
                target: target.unwrap_or_else(TargetSpec::x86_64_unknown_linux_gnu),
                profile: profile.unwrap_or_default(),
                backend: selected.unwrap_or(if profile == Some(BuildProfile::Debug) {
                    Backend::Cranelift
                } else {
                    Backend::Llvm
                }),
                proofs: {
                    proofs.check()?;
                    proofs
                },
                fuel,
                validation: validation(validate_stages),
            }
        }
        Some(Value(command)) if command == "seed-objects" => {
            let manifest = args.value()?;
            if !matches!(args.next()?, Some(Short('o'))) {
                return Err("seed-objects requires -o".into());
            }
            Command::SeedObjects {
                manifest,
                output: args.value()?,
            }
        }
        Some(Value(command)) if command == "image" => {
            let manifest = args.value()?;
            if !matches!(args.next()?, Some(Short('o'))) {
                return Err("image requires -o".into());
            }
            Command::Image {
                manifest,
                output: args.value()?,
            }
        }
        Some(Value(command)) if command == "tapes" => {
            if !matches!(args.next()?, Some(Value(sub)) if sub == "check") {
                return Err("tapes requires check".into());
            }
            let mut tapes = vec![];
            while let Some(arg) = args.next()? {
                match arg {
                    Value(tape) if !tape.to_string_lossy().starts_with('-') => tapes.push(tape),
                    arg => return Err(arg.unexpected()),
                }
            }
            if tapes.is_empty() {
                return Err("tapes check requires at least one tape".into());
            }
            Command::Tapes { tapes }
        }
        Some(Value(command)) if command == "deps" => {
            if args.value()? != "card" {
                return Err("deps requires card".into());
            }
            let (mut manifest, mut package, mut code_store, mut json) = (None, None, None, false);
            while let Some(arg) = args.next()? {
                match arg {
                    Long("json") if !json => json = true,
                    Long("code-store") if code_store.is_none() => code_store = Some(args.value()?),
                    Long("package") if package.is_none() => {
                        package = Some(
                            args.value()?
                                .into_string()
                                .map_err(|_| "--package requires UTF-8")?,
                        )
                    }
                    Value(file)
                        if manifest.is_none() && !file.to_string_lossy().starts_with('-') =>
                    {
                        manifest = Some(file)
                    }
                    arg => return Err(arg.unexpected()),
                }
            }
            Command::DepsCard {
                manifest: manifest.ok_or("deps card requires a package manifest")?,
                package,
                code_store,
                json,
            }
        }
        Some(Value(command)) if command == "doc" => {
            let (mut manifest, mut output, mut json) = (None, None, false);
            while let Some(arg) = args.next()? {
                match arg {
                    Long("json") if !json => json = true,
                    Short('o') if output.is_none() => output = Some(args.value()?),
                    Value(file)
                        if manifest.is_none() && !file.to_string_lossy().starts_with('-') =>
                    {
                        manifest = Some(file)
                    }
                    arg => return Err(arg.unexpected()),
                }
            }
            Command::Doc {
                manifest: manifest.ok_or("doc requires a package manifest")?,
                output,
                json,
            }
        }
        Some(Value(command)) if command == "export" => {
            let run = match args.next()? {
                Some(Long("tir")) => false,
                Some(Long("run")) => true,
                Some(arg) => return Err(arg.unexpected()),
                None => return Err("export requires --tir or --run".into()),
            };
            let entry = args.value()?;
            if !matches!(args.next()?, Some(Short('o'))) {
                return Err("export requires -o".into());
            }
            Command::Export {
                run,
                entry,
                output: args.value()?,
            }
        }
        Some(Value(command)) if command == "expand" => Command::Source {
            command: "expand",
            flag: None,
            file: source_arg(&mut args)?,
        },
        Some(Value(command)) if command == "hash" => Command::Source {
            command: "hash",
            flag: None,
            file: source_arg(&mut args)?,
        },
        Some(Value(command)) if command == "check" || command == "run" || command == "fmt" => {
            let name = match command.to_str() {
                Some("check") => "check",
                Some("run") => "run",
                _ => "fmt",
            };
            if name == "run" {
                let mut config = None;
                let mut cores = None;
                let mut record = None;
                let mut revocations = None;
                let mut replay = None;
                let mut native = false;
                let mut validate_stages = false;
                let mut json = false;
                let mut selected = None;
                let mut simulation = None;
                let mut max_steps = None;
                let mut file = None;
                let mut proofs = Proofs::default();
                let mut arguments = None;
                loop {
                    if let Some(mut raw) = args.try_raw_args() {
                        if raw.peek().is_some_and(|arg| arg == "--") {
                            raw.next();
                            arguments = Some(raw.collect());
                            break;
                        }
                    }
                    let Some(arg) = args.next()? else { break };
                    match arg {
                        Long("proofs") if proofs.sidecar.is_none() => {
                            proofs.sidecar = Some(args.value()?)
                        }
                        Long("trust") => proofs.trust(args.value()?)?,
                        Long("native") if !native => native = true,
                        Long("validate-stages") if !validate_stages => validate_stages = true,
                        Long("json") if !json => json = true,
                        Long("backend") if selected.is_none() => {
                            selected = Some(backend(args.value()?)?)
                        }
                        Long("run" | "grants") if config.is_none() => {
                            config = Some((matches!(arg, Long("grants")), args.value()?));
                        }
                        Long("cores") if cores.is_none() => {
                            let value = args.value()?;
                            cores = Some(
                                value
                                    .to_str()
                                    .and_then(|s| {
                                        s.parse::<u64>()
                                            .ok()
                                            .filter(|n| *n > 0 && n.to_string() == s)
                                    })
                                    .ok_or("--cores requires a positive integer")?,
                            );
                        }
                        Long("record") if record.is_none() => record = Some(args.value()?),
                        Long("revocations") if revocations.is_none() => {
                            revocations = Some(args.value()?)
                        }
                        Long("replay") if replay.is_none() => replay = Some(args.value()?),
                        Long("max-steps") if max_steps.is_none() => {
                            max_steps = Some(step_budget(args.value().ok())?);
                        }
                        Long("simulation") if simulation.is_none() => {
                            simulation = Some(args.value()?)
                        }
                        Value(path)
                            if file.is_none() && !path.to_string_lossy().starts_with('-') =>
                        {
                            file = Some(path)
                        }
                        arg => return Err(arg.unexpected()),
                    }
                }
                let file = file.ok_or("missing source path")?;
                if cores.is_some() && simulation.is_some() {
                    return Err("--cores cannot combine with --simulation".into());
                }
                if native && max_steps.is_some() {
                    return Err("--max-steps is only supported for interpreted run".into());
                }
                proofs.check()?;
                if !native && proofs.sidecar.is_some() {
                    return Err("--proofs requires --native".into());
                }
                if !native && validate_stages {
                    return Err("--validate-stages requires --native".into());
                }
                if !native && selected.is_some() {
                    return Err("--backend requires --native".into());
                }
                if revocations.is_some() && (config.is_none() || replay.is_some()) {
                    return Err("--revocations requires a live --run or --grants launch".into());
                }
                if arguments.is_some()
                    && (matches!(config, Some((false, _)))
                        || replay.is_some()
                        || simulation.is_some())
                {
                    return Err(if matches!(config, Some((false, _))) {
                        "--run keeps the record's Args snapshot; trailing arguments require --grants"
                    } else {
                        "trailing arguments cannot combine with --replay or --simulation"
                    }.into());
                }
                let max_steps = max_steps.unwrap_or(if config.is_some() {
                    None
                } else {
                    Some(DEFAULT_RUN_MAX_STEPS)
                });
                // Without a record the run is the empty-grant record; the explicit
                // delimiter grants precisely Args. Both bind an empty template through
                // the same host admission path as an explicit grants launch.
                if config.is_none() && record.is_none() && replay.is_none() && simulation.is_none()
                {
                    config = Some((true, "{}".into()));
                }
                let launch = match (config, record, replay) {
                    (None, None, None) => None,
                    (Some((grants, config)), record, None) => Some(Launch::Live {
                        grants,
                        cores,
                        config,
                        record,
                        revocations,
                    }),
                    (None, None, Some(tape)) => Some(Launch::Replay(tape)),
                    (None, Some(_), _) => return Err("--record requires --run or --grants".into()),
                    (Some(_), _, Some(_)) => {
                        return Err(
                            "--replay binds its recorded run: it takes no --run, --grants or --record"
                                .into(),
                        );
                    }
                };
                match (simulation, launch) {
                    (Some(config), None) if !native => Command::Simulation {
                        config,
                        file,
                        max_steps,
                        json,
                    },
                    (Some(_), _) => {
                        return Err(
                            "--simulation cannot combine with --native, --run, --grants, --record or --replay".into(),
                        );
                    }
                    (None, Some(launch)) => Command::Run {
                        file,
                        launch,
                        native,
                        validation: validation(validate_stages),
                        backend: selected,
                        proofs,
                        arguments,
                        max_steps,
                        json,
                    },
                    (None, None) => unreachable!("a run without a record binds the empty template"),
                }
            } else {
                match args.next()? {
                    Some(Long("json")) if name == "check" => Command::Source {
                        command: name,
                        flag: Some("--json"),
                        file: source_arg(&mut args)?,
                    },
                    Some(Long("safe")) if name == "check" => Command::Source {
                        command: name,
                        flag: Some("--safe"),
                        file: source_arg(&mut args)?,
                    },
                    Some(Long("check")) if name == "fmt" => Command::Source {
                        command: name,
                        flag: Some("--check"),
                        file: source_arg(&mut args)?,
                    },
                    Some(Value(file)) if !file.to_string_lossy().starts_with('-') => {
                        Command::Source {
                            command: name,
                            flag: None,
                            file,
                        }
                    }
                    Some(arg) => return Err(arg.unexpected()),
                    None => return Err("missing source path".into()),
                }
            }
        }
        Some(arg) => return Err(arg.unexpected()),
        None => return Err("missing command".into()),
    };
    if let Some(arg) = args.next()? {
        return Err(arg.unexpected());
    }
    Ok(command)
}

fn source_arg(args: &mut Parser) -> Result<OsString, lexopt::Error> {
    match args.next()? {
        Some(Value(file)) if !file.to_string_lossy().starts_with('-') => Ok(file),
        Some(arg) => Err(arg.unexpected()),
        None => Err("missing source path".into()),
    }
}

/// Whether `flag` appears anywhere before a `--` separator, removing it.
fn take_flag(args: &mut Vec<OsString>, flag: &str) -> bool {
    let end = args.iter().position(|a| a == "--").unwrap_or(args.len());
    let before = args.len();
    let mut index = 0;
    args.retain(|arg| {
        index += 1;
        index > end || arg != flag
    });
    args.len() != before
}

/// Run the command, then report what `--timings` and `--mem-usage` measured.
fn execute(mut args: Vec<OsString>, start: std::time::Instant) -> u8 {
    if args.first().is_some_and(|a| a == "serve") {
        return serve::serve(&args[1..]);
    }
    // A running resident compiler for this workspace takes the command.
    if let Some(code) = serve::client(&args) {
        return code;
    }
    if args.first().is_some_and(|a| a == "check") && take_flag(&mut args, "--watch") {
        return serve::watch(args);
    }
    // The process exits right after its one session: leave its memory to the OS.
    hirsel_engine::free_sessions(false);
    // `--verbose` reports how dependency packages were checked (a missing or
    // stale std image) on stderr.
    hirsel_engine::report_images(take_flag(&mut args, "--verbose"));
    // `--no-verify` skips V, the second checker, where it costs most.
    let verify = if take_flag(&mut args, "--no-verify") {
        Verify::Off
    } else {
        Verify::On
    };
    let measure = match measure::Measure::take(&mut args) {
        Ok(measure) => measure,
        Err(error) => {
            eprintln!("{error}");
            return usage();
        }
    };
    measure.start();
    let command = args
        .first()
        .and_then(|a| a.to_str())
        .unwrap_or_default()
        .to_owned();
    let code = execute_command(args, verify);
    measure.finish(&command, start.elapsed());
    code
}

/// Run one command in a process that keeps its sessions (`serve`, `check
/// --watch`): the global flags `execute` takes, without measurement.
fn execute_resident(mut args: Vec<OsString>) -> u8 {
    hirsel_engine::report_prechecks(take_flag(&mut args, "--verbose"));
    let verify = if take_flag(&mut args, "--no-verify") {
        Verify::Off
    } else {
        Verify::On
    };
    execute_command(args, verify)
}

fn execute_command(args: Vec<OsString>, verify: Verify) -> u8 {
    let command = match parse_command(args) {
        Ok(command) => command,
        Err(error) => {
            eprintln!("{error}");
            return usage();
        }
    };
    if verify == Verify::Off
        && !matches!(
            command,
            Command::Test { .. }
                | Command::Build { .. }
                | Command::Run { .. }
                | Command::Simulation { .. }
        )
    {
        eprintln!("--no-verify applies only to run, test and build");
        return usage();
    }
    let (command, flag, file) = match command {
        Command::Test {
            manifest,
            filters,
            seed,
            json,
            native,
            bless: _,
            max_steps: _,
            bench: Some(target),
            backend,
            schedules: _,
            simulation_debug: _,
        } => {
            let results = if native {
                hirsel_driver::native::bench_path(
                    Path::new(&manifest),
                    &filters,
                    seed,
                    backend,
                    target,
                    verify,
                )
            } else {
                hirsel_driver::testing::bench_path(
                    Path::new(&manifest),
                    &filters,
                    seed,
                    target,
                    verify,
                )
            };
            return match results {
                Ok(results) => {
                    let failed = results.iter().filter(|r| r.failure.is_some()).count();
                    if json {
                        let benches: Vec<_> = results.iter().map(|r| serde_json::json!({
                            "name": r.name, "seed": r.seed, "status": if r.failure.is_some() { "failed" } else { "measured" }, "failure": r.failure, "iterations": r.iterations, "nanoseconds": r.nanoseconds
                        })).collect();
                        println!(
                            "{}",
                            serde_json::json!({"schema":1,"benches":benches,"summary":{"measured":results.len()-failed,"failed":failed,"total":results.len()}})
                        );
                    } else {
                        for result in &results {
                            match &result.failure {
                                Some(failure) => {
                                    println!(
                                        "bench {} ... FAILED (seed={}): {failure}",
                                        result.name, result.seed
                                    )
                                }
                                None => println!(
                                    "bench {} ... {} iterations, {:.1} ns/iter",
                                    result.name,
                                    result.iterations,
                                    result.per_iteration()
                                ),
                            }
                        }
                        println!(
                            "bench result: {}. {} measured; {} failed; {} total",
                            if failed == 0 { "ok" } else { "FAILED" },
                            results.len() - failed,
                            failed,
                            results.len()
                        );
                    }
                    RunOutcome::Returned(if failed == 0 {
                        hirsel_run_schema::Returned::Ok
                    } else {
                        hirsel_run_schema::Returned::Error
                    })
                    .exit_code() as u8
                }
                Err(error) => run_error(error, json),
            };
        }
        Command::Test {
            manifest,
            filters,
            seed,
            json,
            native,
            bless,
            max_steps,
            bench: None,
            backend,
            schedules,
            simulation_debug,
        } => {
            let results = if native {
                hirsel_driver::native::test_path(
                    Path::new(&manifest),
                    &filters,
                    seed,
                    backend,
                    bless,
                    &schedules,
                    verify,
                )
            } else {
                hirsel_driver::testing::run_path_with_simulation_debug(
                    Path::new(&manifest),
                    &filters,
                    seed,
                    bless,
                    max_steps,
                    &schedules,
                    hirsel_driver::testing::SimulationTestOptions {
                        verify,
                        debug: simulation_debug,
                    },
                )
            };
            // The command that reruns one test under the schedule it failed in.
            let replay = |result: &hirsel_driver::testing::TestResult| {
                let scheduler = result.scheduler.as_ref()?;
                let mut command = format!(
                    "hirsel test {} {} --scheduler {scheduler}",
                    Path::new(&manifest).display(),
                    result.name
                );
                if simulation_debug.races {
                    command.push_str(" --race");
                }
                let points = simulation_debug.points();
                if !points.is_empty() {
                    command.push_str(&format!(" --sim-points {points}"));
                }
                if max_steps != Some(DEFAULT_RUN_MAX_STEPS) {
                    command.push_str(&format!(
                        " --max-steps {}",
                        max_steps.map_or("unbounded".into(), |n| n.to_string())
                    ));
                }
                if let Some(seed) = seed {
                    command.push_str(&format!(" --seed {seed}"));
                }
                if native {
                    command.push_str(" --native");
                    if backend != hirsel_driver::native::Backend::default() {
                        command.push_str(&format!(" --backend {}", backend.name()));
                    }
                }
                Some(command)
            };
            return match results {
                Ok(results) => {
                    let failed = results.iter().filter(|r| r.failure.is_some()).count();
                    if json {
                        let tests: Vec<_> = results.iter().map(|r| serde_json::json!({
                            "name": r.name, "seed": r.seed, "status": if r.failure.is_some() { "failed" } else { "passed" }, "failure": r.failure, "report": r.report,
                            "scheduler": r.scheduler.as_ref().map(|s| s.to_string()), "schedules": r.schedules,
                            "replay": r.failure.as_ref().and_then(|_| replay(r))
                        })).collect();
                        println!(
                            "{}",
                            serde_json::json!({"schema":1,"tests":tests,"summary":{"passed":results.len()-failed,"failed":failed,"total":results.len()}})
                        );
                    } else {
                        for result in &results {
                            // A selected or explored schedule: the failing run's scheduler,
                            // and how many schedules an exploration ran.
                            let schedule = match (&schedules, &result.scheduler) {
                                (hirsel_driver::testing::Schedules::Explore { .. }, Some(s))
                                    if result.failure.is_some() =>
                                {
                                    format!(", scheduler={s}, schedule {}", result.schedules)
                                }
                                (hirsel_driver::testing::Schedules::Explore { .. }, _) => {
                                    format!(", {} schedules", result.schedules)
                                }
                                (_, Some(s)) => format!(", scheduler={s}"),
                                (_, None) => String::new(),
                            };
                            println!(
                                "test {} ... {} (seed={}{schedule}){}",
                                result.name,
                                if result.failure.is_some() {
                                    "FAILED"
                                } else {
                                    "ok"
                                },
                                result.seed,
                                result
                                    .failure
                                    .as_ref()
                                    .map(|f| format!(": {f}"))
                                    .unwrap_or_default()
                            );
                            // A failed test's report follows it, indented, then its replay.
                            if result.failure.is_some() {
                                for line in result.report.lines() {
                                    println!("    {line}");
                                }
                                if let Some(command) = replay(result) {
                                    println!("    replay: {command}");
                                }
                            }
                        }
                        println!(
                            "test result: {}. {} passed; {} failed; {} total",
                            if failed == 0 { "ok" } else { "FAILED" },
                            results.len() - failed,
                            failed,
                            results.len()
                        );
                    }
                    RunOutcome::Returned(if failed == 0 {
                        hirsel_run_schema::Returned::Ok
                    } else {
                        hirsel_run_schema::Returned::Error
                    })
                    .exit_code() as u8
                }
                Err(error) => run_error(error, json),
            };
        }
        Command::DepsCard {
            manifest,
            package,
            code_store,
            json,
        } => {
            match hirsel_driver::dependency_card::cards(
                Path::new(&manifest),
                package.as_deref(),
                code_store.as_deref().map(Path::new),
            ) {
                Ok(report) => {
                    if json {
                        println!("{}", serde_json::to_string_pretty(&report).unwrap());
                    } else {
                        print!("{}", hirsel_engine::render_dependency_cards(&report));
                    }
                    return 0;
                }
                Err(error) => return tool_error(error, json),
            }
        }
        Command::Doc {
            manifest,
            output,
            json,
        } => {
            let entries = match hirsel_driver::api_index::entries(Path::new(&manifest)) {
                Ok(entries) => entries,
                Err(error) => return tool_error(error, false),
            };
            let page = if json {
                hirsel_driver::api_index::render_json(&entries)
            } else {
                hirsel_driver::api_index::render(&entries)
            };
            return match output {
                Some(path) => match std::fs::write(path, page) {
                    Ok(()) => 0,
                    Err(error) => tool_error(error.into(), false),
                },
                None => {
                    print!("{page}");
                    0
                }
            };
        }

        Command::Help => {
            println!("{USAGE}");
            return 0;
        }
        Command::Tapes { tapes } => {
            let paths: Vec<_> = tapes.iter().map(std::path::PathBuf::from).collect();
            return match hirsel_driver::check_tapes(&paths) {
                Ok(links) => {
                    for link in &links.checked {
                        println!("checked: {link}");
                    }
                    for link in &links.unchecked {
                        println!("unchecked: {link}");
                    }
                    println!(
                        "tapes: {} links checked, {} unchecked",
                        links.checked.len(),
                        links.unchecked.len()
                    );
                    0
                }
                Err(mismatches) => {
                    for mismatch in &mismatches {
                        eprintln!("hirsel: tapes check: {mismatch}");
                    }
                    1
                }
            };
        }
        Command::SeedObjects { manifest, output } => {
            return match hirsel_driver::native::seed_objects(
                Path::new(&manifest),
                Path::new(&output),
            ) {
                Ok(report) => {
                    println!(
                        "seeded {} objects; {} roots, {} units, {} generic instances",
                        report.objects, report.roots, report.units, report.generic_units
                    );
                    0
                }
                Err(error) => {
                    eprintln!("hirsel: seed-objects: {error}");
                    2
                }
            };
        }
        Command::Image { manifest, output } => {
            return match hirsel_driver::COMPILER
                .image_package(Path::new(&manifest), Path::new(&output))
            {
                Ok(()) => 0,
                Err(error) => tool_error(error, false),
            };
        }
        Command::Build {
            source,
            output,
            target,
            profile,
            backend,
            proofs,
            fuel,
            validation,
        } => {
            let evidence = match proofs.evidence() {
                Ok(evidence) => evidence,
                Err(error) => return tool_error(error, false),
            };
            return match hirsel_driver::native::build_path_native_backend_with_validation(
                Path::new(&source),
                Path::new(&output),
                target,
                profile,
                backend,
                evidence.as_ref(),
                fuel,
                verify,
                validation,
            ) {
                Ok(Ok(report)) => {
                    for line in report {
                        eprintln!("hirsel: {line}");
                    }
                    0
                }
                Ok(Err(error)) => {
                    eprintln!("hirsel: {error}");
                    2
                }
                Err(error) => tool_error(error, false),
            };
        }
        Command::View {
            tape,
            capture,
            output,
            perfetto,
            timing,
            source,
            analysis,
        } => {
            let view = || -> Result<String, String> {
                let bytes = std::fs::read(&tape).map_err(|e| e.to_string())?;
                let mut view = hirsel_driver::observation_view::View::parse(&capture, &bytes)?;
                let explicit_timing = timing.is_some();
                let timing_path = timing.unwrap_or_else(|| {
                    let mut path = tape.clone();
                    path.push(".timing.json");
                    path
                });
                match std::fs::read_to_string(&timing_path) {
                    Ok(text) => view.apply_timing(&text)?,
                    Err(e) if !explicit_timing && e.kind() == std::io::ErrorKind::NotFound => {}
                    Err(e) => return Err(e.to_string()),
                }
                if let (Some(source), Some(analysis)) = (source, analysis) {
                    view.replay_enrich(
                        Path::new(&source),
                        Path::new(&tape),
                        &analysis,
                        Some(DEFAULT_RUN_MAX_STEPS),
                    )?;
                }
                Ok(if perfetto {
                    view.logical_perfetto()
                } else {
                    view.normalized()
                })
            };
            return match view()
                .and_then(|text| std::fs::write(output, text).map_err(|e| e.to_string()))
            {
                Ok(()) => 0,
                Err(error) => {
                    eprintln!("view: {error}");
                    1
                }
            };
        }
        Command::Export { run, entry, output } => {
            if run {
                return match hirsel_driver::export_run_path(Path::new(&entry)) {
                    Ok(bytes) => match std::fs::write(output, bytes) {
                        Ok(()) => 0,
                        Err(e) => tool_error(ToolError::Io(e), false),
                    },
                    Err(e) => tool_error(e, false),
                };
            }
            // The prover-neutral TIR export: a tool artifact,
            // never a source verdict, so diagnostics come first as for `run`.
            return match hirsel_driver::export_tir_path(Path::new(&entry)) {
                Ok(text) => match std::fs::write(output, text) {
                    Ok(()) => 0,
                    Err(error) => tool_error(ToolError::Io(error), false),
                },
                Err(error) => tool_error(error, false),
            };
        }
        Command::Simulation {
            config,
            file,
            max_steps,
            json,
        } => {
            let simulation = match read_simulation(Path::new(&config)) {
                Ok(config) => config,
                Err(error) => {
                    eprintln!("hirsel: simulation: {error}");
                    return RunOutcome::Tooling(error).exit_code() as u8;
                }
            };
            return run_result(
                hirsel_driver::run_path_with_step_budget(
                    Path::new(&file),
                    simulation,
                    max_steps,
                    verify,
                    &mut std::io::stdout().lock(),
                    &mut std::io::stderr().lock(),
                ),
                json,
            );
        }
        Command::Run {
            file,
            launch,
            native: true,
            validation,
            backend,
            proofs,
            arguments,
            max_steps: _,
            json,
        } => {
            let evidence = match proofs.evidence() {
                Ok(evidence) => evidence,
                Err(error) => return run_error(error, false),
            };
            // Native code is an engine choice, never a source verdict: the
            // same diagnostics come first, and a stage that cannot build the
            // program yet is a tool outcome (exit 3).
            return match hirsel_driver::native::run_path_native_backend_with_validation(
                Path::new(&file),
                launch.host(arguments.as_deref()),
                evidence.as_ref(),
                backend.unwrap_or(hirsel_driver::native::Backend::Cranelift),
                hirsel_driver::native::Output::Inherit { json },
                verify,
                validation,
            ) {
                // The native driver already wrote the complete failure chain.
                Ok(Ok(outcome @ RunOutcome::Failed { .. })) => outcome.exit_code() as u8,
                Ok(Ok(outcome)) => run_result(Ok(outcome), json),
                Ok(Err(error)) => {
                    eprintln!("hirsel: {error}");
                    RunOutcome::Tooling(error.to_string()).exit_code() as u8
                }
                Err(error) => run_error(error, false),
            };
        }
        Command::Run {
            file,
            launch,
            native: false,
            validation: _,
            backend: _,
            proofs: _,
            arguments,
            max_steps,
            json,
        } => {
            return run_result(
                hirsel_driver::run_path_on_host(
                    Path::new(&file),
                    launch.host(arguments.as_deref()),
                    max_steps,
                    verify,
                ),
                json,
            );
        }
        Command::Source {
            command,
            flag,
            file,
        } => (command, flag, file),
    };
    let path = Path::new(&file);
    let json = flag == Some("--json");
    match command {
        "check" => match if flag == Some("--safe") {
            hirsel_driver::check_safe_path(path)
        } else {
            hirsel_driver::check_path(path).map(|output| (output, None))
        } {
            Ok((output, verdict)) => {
                for d in &output.diagnostics {
                    if json {
                        println!("{}", d.to_json(&output.sources));
                    } else {
                        eprint!("{}", d.render_human(&output.sources));
                    }
                }
                // Every foreign library in the program is visible at check (PROG-4).
                for package in &output.unsafe_ffi {
                    let link: Vec<String> = package
                        .link
                        .iter()
                        .map(|l| match l {
                            hirsel_engine::LinkInput::Library(name) => format!("-l{name}"),
                            hirsel_engine::LinkInput::File(path) => path.display().to_string(),
                        })
                        .collect();
                    if json {
                        println!(
                            "{}",
                            serde_json::json!({
                                "schema_version": 1,
                                "unsafe_ffi_package": {
                                    "name": package.package.name,
                                    "version": package.package.version,
                                    "link": link,
                                    "headers_checked": package.headers_checked,
                                },
                            })
                        );
                    } else {
                        eprintln!(
                            "note: {}@{} is an unsafe ffi package (links {}; headers {})",
                            package.package.name,
                            package.package.version,
                            if link.is_empty() {
                                "nothing".into()
                            } else {
                                link.join(" ")
                            },
                            if package.headers_checked {
                                "checked"
                            } else {
                                "not checked"
                            },
                        );
                    }
                }
                if output.diagnostics.is_empty() {
                    if let hirsel_engine::CheckCompletion::Incomplete { phase } = output.completion
                    {
                        eprintln!("{} is not implemented yet", phase.phase_name());
                        return 2;
                    }
                }
                // The verdict exists only for a complete, accepted program (CAP-6).
                match verdict {
                    Some((entry, None)) => println!(
                        "safe: {entry} reaches no extern \"c\" function or Unsafe operation"
                    ),
                    Some((entry, Some(reach))) => {
                        println!(
                            "unsafe: {entry} reaches {} {}: {}",
                            reach.escape,
                            reach.path.last().expect("a reach path ends at its escape"),
                            reach.spelled().join(" -> ")
                        );
                        return 1;
                    }
                    None => {}
                }
                let rejected = output
                    .diagnostics
                    .iter()
                    .any(|d| d.severity.as_str() == "error");
                if rejected && json {
                    rejection_cause(&output.diagnostics);
                }
                u8::from(rejected)
            }
            Err(error) => tool_error(error, json),
        },
        "hash" => match hirsel_driver::hash_path(path) {
            Ok(text) => {
                print!("{text}");
                0
            }
            Err(error) => tool_error(error, false),
        },
        "expand" => match hirsel_driver::expand_path(path) {
            Ok(text) => {
                print!("{text}");
                0
            }
            Err(error) => tool_error(error, false),
        },
        "fmt" => match hirsel_driver::format_path(path) {
            Ok(formatted) if flag.is_some() => match std::fs::read_to_string(path) {
                Ok(original) if original == formatted => 0,
                Ok(_) => {
                    eprintln!("{}: formatting differs", path.display());
                    1
                }
                Err(error) => tool_error(ToolError::Io(error), false),
            },
            Ok(formatted) => match std::fs::write(path, formatted) {
                Ok(()) => 0,
                Err(error) => tool_error(ToolError::Io(error), false),
            },
            Err(error) => tool_error(error, false),
        },
        _ => usage(),
    }
}
fn rejection_cause(diagnostics: &[hirsel_diag::Diagnostic]) {
    if let Some(diagnostic) = diagnostics
        .iter()
        .find(|d| d.severity.as_str() == "error")
        .or_else(|| diagnostics.first())
    {
        eprintln!(
            "hirsel: {}: {}",
            diagnostic.rule,
            diagnostic
                .message
                .lines()
                .next()
                .unwrap_or("source rejected")
        );
    } else {
        eprintln!("hirsel: source rejected");
    }
}

fn tool_error(error: ToolError, json: bool) -> u8 {
    if let ToolError::Query {
        error: QueryError::Diagnostics(diagnostics),
        sources,
        ..
    } = &error
    {
        for d in diagnostics {
            if json {
                println!("{}", d.to_json(sources));
            } else {
                eprint!("{}", d.render_human(sources));
            }
        }
        if json || diagnostics.is_empty() {
            rejection_cause(diagnostics);
        }
        return 1;
    }
    let parser_limit = matches!(&error, ToolError::Query { error: QueryError::Operational(failure), .. } if failure.operation == "parser-tooling-limit");
    if json {
        println!(
            "{}",
            serde_json::json!({
                "schema_version": 1,
                "outcome": if parser_limit { "tooling_limit" } else { "tool_error" },
                "kind": if parser_limit { "syntax_nesting" } else { "operational" },
                "message": error.to_string(),
                "deterministic": parser_limit,
            })
        );
    }
    eprintln!("hirsel: {error}");
    // A run without a record grants nothing (determinism.md); say how to grant the root.
    if let ToolError::Launch(hirsel_run_schema::RunRefusal::NotGranted { root }) = &error
        && !json
    {
        eprintln!(
            "hirsel: the entry takes `{root}`, which this run does not grant: grant it with `--grants <grants.json>`, or run interpreted in a simulated world with `--simulation <world.json>`"
        );
    }
    if parser_limit
        || matches!(
            error,
            ToolError::Query {
                error: QueryError::NotImplemented,
                ..
            }
        )
    {
        2
    } else {
        1
    }
}
fn usage() -> u8 {
    eprintln!("{USAGE}");
    2
}

fn read_simulation(path: &Path) -> Result<hirsel_engine::SimulationConfig, String> {
    let value: serde_json::Value =
        serde_json::from_slice(&std::fs::read(path).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
    let object = value.as_object().ok_or("expected a JSON object")?;
    let mut config = hirsel_engine::SimulationConfig::default();
    for (key, value) in object {
        match key.as_str() {
            "args" | "grants" | "directories" | "projections" => {
                let values = value
                    .as_array()
                    .ok_or_else(|| format!("{key}: expected an array"))?
                    .iter()
                    .map(|v| {
                        v.as_str()
                            .map(str::to_owned)
                            .ok_or_else(|| format!("{key}: expected strings"))
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                match key.as_str() {
                    "args" => config.args = values,
                    "grants" => config.grants = values.into_iter().collect(),
                    "projections" => {
                        for projection in &values {
                            if !matches!(
                                projection.as_str(),
                                "stdin"
                                    | "stderr"
                                    | "stdout"
                                    | "tasks"
                                    | "net"
                                    | "process"
                                    | "random"
                                    | "dir"
                            ) {
                                return Err(format!("unknown Io projection {projection}"));
                            }
                        }
                        config.projections = values.into_iter().collect();
                    }
                    _ => config.directories = values.into_iter().collect(),
                }
            }
            "race" => config.debug.races = value.as_bool().ok_or("race: expected a boolean")?,
            "sim_points" => {
                let points = value
                    .as_str()
                    .and_then(hirsel_driver::testing::SimulationDebug::parse_points)
                    .ok_or(
                        "sim_points: expected root-release, atomic-store, or both comma-separated",
                    )?;
                config.debug.root_releases = points.root_releases;
                config.debug.atomic_stores = points.atomic_stores;
            }
            "scheduler" => {
                config.scheduler = Some(
                    value
                        .as_str()
                        .and_then(hirsel_driver::testing::Scheduler::parse)
                        .ok_or("scheduler: expected name@1:seed")?,
                )
            }
            "stdin" => {
                config.stdin = if let Some(text) = value.as_str() {
                    text.as_bytes().to_vec()
                } else {
                    value
                        .as_array()
                        .ok_or("stdin: expected text or an array of bytes")?
                        .iter()
                        .map(|byte| {
                            byte.as_u64()
                                .and_then(|n| u8::try_from(n).ok())
                                .ok_or("stdin: expected bytes")
                        })
                        .collect::<Result<Vec<_>, _>>()?
                };
            }
            "files" | "env" => {
                let entries = value
                    .as_object()
                    .ok_or_else(|| format!("{key}: expected an object"))?;
                for (name, text) in entries {
                    if key == "env" && text.is_null() {
                        config.env.insert(name.clone(), None);
                        continue;
                    }
                    let text = text
                        .as_str()
                        .ok_or_else(|| format!("{key}: expected string values"))?;
                    if key == "files" {
                        config.files.insert(name.clone(), text.as_bytes().to_vec());
                    } else {
                        config.env.insert(name.clone(), Some(text.into()));
                    }
                }
            }
            // The RunConfig `io.net` object; the simulation serves net.script@2.
            "net" => {
                config.net =
                    Some(serde_json::from_value(value.clone()).map_err(|e| format!("net: {e}"))?)
            }
            // The RunConfig `io.process` object; the simulation serves process.script@1.
            "process" => {
                config.process = Some(
                    serde_json::from_value(value.clone()).map_err(|e| format!("process: {e}"))?,
                )
            }
            "wall" => config.wall = value.as_i64().ok_or("wall: expected a signed integer")?,
            "monotonic" => {
                config.monotonic = value
                    .as_u64()
                    .ok_or("monotonic: expected a nonnegative integer")?
            }
            _ => return Err(format!("unknown field {key}")),
        }
    }
    Ok(config)
}

fn run_result(result: Result<RunOutcome, ToolError>, json: bool) -> u8 {
    let outcome = match result {
        Ok(outcome) => outcome,
        Err(error) => {
            return run_error(error, json);
        }
    };
    match &outcome {
        RunOutcome::Returned(_) => {}
        RunOutcome::Failed { primary, causes } => {
            for failure in std::iter::once(primary).chain(causes) {
                eprintln!(
                    "{}",
                    failure
                        .site
                        .report(&failure.rule, json, failure.operation.as_ref())
                );
            }
        }
        RunOutcome::UndefinedBehavior(diagnostic) | RunOutcome::Trapped(diagnostic) => {
            eprintln!("{diagnostic}")
        }
        RunOutcome::Blocked(waits) => eprintln!("Blocked: {waits:?}"),
        RunOutcome::BudgetExceeded(kind) => eprintln!("budget exceeded: {}", kind.name()),
        RunOutcome::ToolingLimit(limit) => eprintln!("evaluator tooling limit: {}", limit.kind),
        RunOutcome::Environmental(cause) => eprintln!("hirsel: environmental failure: {cause}"),
        RunOutcome::Refused(reason) => eprintln!("hirsel: launch: {reason}"),
        RunOutcome::Diverged(reason) => eprintln!("hirsel: replay refused: {reason}"),
        RunOutcome::Tooling(reason) => eprintln!("hirsel: {reason}"),
    }
    outcome.exit_code() as u8
}

fn run_error(error: ToolError, json: bool) -> u8 {
    let rejected = matches!(
        &error,
        ToolError::Query {
            error: QueryError::Diagnostics(_),
            ..
        }
    );
    let outcome = match &error {
        ToolError::Launch(reason) => RunOutcome::Refused(reason.to_string()),
        ToolError::Runtime(reason) if reason.starts_with("replay refused:") => {
            RunOutcome::Diverged(reason.clone())
        }
        _ => RunOutcome::Tooling(error.to_string()),
    };
    let code = tool_error(error, json);
    if rejected {
        code
    } else {
        outcome.exit_code() as u8
    }
}
