use hirsel_cli_args::lexopt::{self, Arg::*, Parser};
use hirsel_engine::{QueryError, RunOutcome, ToolError};
use std::{env, ffi::OsString, path::Path, process::ExitCode};

const USAGE: &str = "usage: hirsel [--verbose] check [--json] <source|manifest.json> | test [--json] [--seed <u64>] [--native [--backend llvm|cranelift]] <manifest.json> [filter...] | run [--json] [--native [--backend llvm|cranelift] [--proofs <sidecar> [--trust <tool@version>]...] | --simulation <json>] <source|manifest.json> [(--run <json> | --grants <json>) [--record <tape>] [--revocations <json>] | --replay <tape>] [--max-steps <n|unbounded>] [-- args...] | build [--release | --debug] [--backend llvm|cranelift] [--target x86_64-linux | aarch64-linux] [--proofs <sidecar> [--trust <tool@version>]...] <source|manifest.json> -o <exe> | fmt [--check] <file> | export --tir|--run <source|manifest.json> -o <file> | doc --index [--root <repo>] | precheck <package manifest.json> -o <file>";

mod execution_policy;
use execution_policy::DEFAULT_RUN_MAX_STEPS;

fn main() -> ExitCode {
    hirsel_engine::with_compiler_stack(|| ExitCode::from(execute(env::args_os().skip(1).collect())))
}
enum Command {
    Help,
    Test {
        manifest: OsString,
        filters: Vec<String>,
        seed: Option<u64>,
        json: bool,
        native: bool,
        backend: hirsel_driver::native::Backend,
    },
    Precheck {
        manifest: OsString,
        output: OsString,
    },
    Doc {
        root: OsString,
    },
    Build {
        source: OsString,
        output: OsString,
        target: hirsel_driver::native::TargetSpec,
        profile: hirsel_driver::native::BuildProfile,
        backend: hirsel_driver::native::Backend,
        proofs: Proofs,
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
        launch: Option<Launch>,
        native: bool,
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
                config,
                record,
                revocations,
            } => hirsel_driver::HostLaunch::Live {
                config: if *grants {
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

fn parse_command(args: Vec<OsString>) -> Result<Command, lexopt::Error> {
    let mut args = Parser::from_args(args);
    let command = match args.next()? {
        Some(Long("help")) => Command::Help,
        Some(Value(command)) if command == "help" => Command::Help,
        Some(Value(command)) if command == "test" => {
            let (mut manifest, mut filters, mut seed, mut json, mut native, mut selected) =
                (None, Vec::new(), None, false, false, None);
            while let Some(arg) = args.next()? {
                match arg {
                    Long("json") if !json => json = true,
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
            if selected.is_some() && !native {
                return Err("--backend requires --native".into());
            }
            Command::Test {
                manifest: manifest.ok_or("test requires a package manifest")?,
                filters,
                seed,
                json,
                native,
                backend: selected.unwrap_or_default(),
            }
        }
        Some(Value(command)) if command == "build" => {
            use hirsel_driver::native::{BuildProfile, TargetSpec};
            let (mut source, mut output, mut target, mut profile) = (None, None, None, None);
            let mut selected = None;
            let mut proofs = Proofs::default();
            while let Some(arg) = args.next()? {
                match arg {
                    Long("proofs") if proofs.sidecar.is_none() => {
                        proofs.sidecar = Some(args.value()?)
                    }
                    Long("trust") => proofs.trust(args.value()?)?,
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
            Command::Build {
                source: source.ok_or("build requires a source or manifest")?,
                output: output.ok_or("build requires -o")?,
                target: target.unwrap_or_else(TargetSpec::x86_64_unknown_linux_gnu),
                profile: profile.unwrap_or_default(),
                backend: selected.unwrap_or_default(),
                proofs: {
                    proofs.check()?;
                    proofs
                },
            }
        }
        Some(Value(command)) if command == "precheck" => {
            let manifest = args.value()?;
            if !matches!(args.next()?, Some(Short('o'))) {
                return Err("precheck requires -o".into());
            }
            Command::Precheck {
                manifest,
                output: args.value()?,
            }
        }
        Some(Value(command)) if command == "doc" => {
            if !matches!(args.next()?, Some(Long("index"))) {
                return Err("doc requires --index".into());
            }
            let root = match args.next()? {
                Some(Long("root")) => args.value()?,
                Some(arg) => return Err(arg.unexpected()),
                None => OsString::from("."),
            };
            Command::Doc { root }
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
        Some(Value(command)) if command == "check" || command == "run" || command == "fmt" => {
            let name = match command.to_str() {
                Some("check") => "check",
                Some("run") => "run",
                _ => "fmt",
            };
            if name == "run" {
                let mut config = None;
                let mut record = None;
                let mut revocations = None;
                let mut replay = None;
                let mut native = false;
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
                        Long("json") if !json => json = true,
                        Long("backend") if selected.is_none() => {
                            selected = Some(backend(args.value()?)?)
                        }
                        Long("run" | "grants") if config.is_none() => {
                            config = Some((matches!(arg, Long("grants")), args.value()?));
                        }
                        Long("record") if record.is_none() => record = Some(args.value()?),
                        Long("revocations") if revocations.is_none() => {
                            revocations = Some(args.value()?)
                        }
                        Long("replay") if replay.is_none() => replay = Some(args.value()?),
                        Long("max-steps") if max_steps.is_none() => {
                            let value = args.value().ok();
                            max_steps = Some(match value.as_deref().and_then(|v| v.to_str()) {
                                Some("unbounded") => None,
                                Some(value) => Some(value.parse::<u64>().map_err(|_| {
                                    lexopt::Error::from(
                                        "--max-steps requires an unsigned integer or unbounded",
                                    )
                                })?),
                                None => {
                                    return Err(
                                        "--max-steps requires an unsigned integer or unbounded"
                                            .into(),
                                    );
                                }
                            });
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
                if native && max_steps.is_some() {
                    return Err("--max-steps is only supported for interpreted run".into());
                }
                proofs.check()?;
                if !native && proofs.sidecar.is_some() {
                    return Err("--proofs requires --native".into());
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
                // The explicit delimiter grants precisely Args. Bind an empty template
                // through the same host admission path as an explicit grants launch.
                if arguments.is_some() && config.is_none() {
                    config = Some((true, "{}".into()));
                }
                let launch = match (config, record, replay) {
                    (None, None, None) => None,
                    (Some((grants, config)), record, None) => Some(Launch::Live {
                        grants,
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
                if let Some(config) = simulation {
                    if native || launch.is_some() {
                        return Err(
                            "--simulation cannot combine with --native, --run, --grants, --record or --replay".into(),
                        );
                    }
                    Command::Simulation {
                        config,
                        file,
                        max_steps,
                        json,
                    }
                } else {
                    Command::Run {
                        file,
                        launch,
                        native,
                        backend: selected,
                        proofs,
                        arguments,
                        max_steps,
                        json,
                    }
                }
            } else {
                match args.next()? {
                    Some(Long("json")) if name == "check" => Command::Source {
                        command: name,
                        flag: Some("--json"),
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

/// `--verbose`, anywhere before a `--` separator, reports how dependency
/// packages were checked (a missing or mismatched precheck) on stderr.
fn take_verbose(args: &mut Vec<OsString>) -> bool {
    let end = args.iter().position(|a| a == "--").unwrap_or(args.len());
    let before = args.len();
    let mut index = 0;
    args.retain(|arg| {
        index += 1;
        index > end || arg != "--verbose"
    });
    args.len() != before
}

fn execute(mut args: Vec<OsString>) -> u8 {
    hirsel_engine::report_prechecks(take_verbose(&mut args));
    let command = match parse_command(args) {
        Ok(command) => command,
        Err(error) => {
            eprintln!("{error}");
            return usage();
        }
    };
    let (command, flag, file) = match command {
        Command::Test {
            manifest,
            filters,
            seed,
            json,
            native,
            backend,
        } => {
            let results = if native {
                hirsel_driver::native::test_path(Path::new(&manifest), &filters, seed, backend)
            } else {
                hirsel_driver::testing::run_path(Path::new(&manifest), &filters, seed)
            };
            return match results {
                Ok(results) => {
                    let failed = results.iter().filter(|r| r.failure.is_some()).count();
                    if json {
                        let tests: Vec<_> = results.iter().map(|r| serde_json::json!({
                            "name": r.name, "seed": r.seed, "status": if r.failure.is_some() { "failed" } else { "passed" }, "failure": r.failure
                        })).collect();
                        println!(
                            "{}",
                            serde_json::json!({"schema":1,"tests":tests,"summary":{"passed":results.len()-failed,"failed":failed,"total":results.len()}})
                        );
                    } else {
                        for result in &results {
                            println!(
                                "test {} ... {} (seed={}){}",
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
                        }
                        println!(
                            "test result: {}. {} passed; {} failed; {} total",
                            if failed == 0 { "ok" } else { "FAILED" },
                            results.len() - failed,
                            failed,
                            results.len()
                        );
                    }
                    u8::from(failed != 0)
                }
                Err(error) => tool_error(error, json),
            };
        }
        Command::Doc { root } => {
            return match hirsel_driver::api_index::entries(Path::new(&root)) {
                Ok(entries) => {
                    print!("{}", hirsel_driver::api_index::render(&entries));
                    0
                }
                Err(error) => tool_error(error, false),
            };
        }
        Command::Help => {
            println!("{USAGE}");
            return 0;
        }
        Command::Precheck { manifest, output } => {
            let bytes = match hirsel_driver::COMPILER.precheck_package(Path::new(&manifest)) {
                Ok(bytes) => bytes,
                Err(error) => return tool_error(error, false),
            };
            // Readers never see a partial precheck.
            let temporary = Path::new(&output).with_extension("precheck.partial");
            return match std::fs::write(&temporary, bytes)
                .and_then(|()| std::fs::rename(&temporary, &output))
            {
                Ok(()) => 0,
                Err(error) => tool_error(ToolError::Io(error), false),
            };
        }
        Command::Build {
            source,
            output,
            target,
            profile,
            backend,
            proofs,
        } => {
            let evidence = match proofs.evidence() {
                Ok(evidence) => evidence,
                Err(error) => return tool_error(error, false),
            };
            return match hirsel_driver::native::build_path_native_backend(
                Path::new(&source),
                Path::new(&output),
                target,
                profile,
                backend,
                evidence.as_ref(),
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
                    return 2;
                }
            };
            return run_result(
                hirsel_driver::run_path_with_step_budget(
                    Path::new(&file),
                    simulation,
                    max_steps,
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
            backend,
            proofs,
            arguments,
            max_steps: _,
            json,
        } => {
            let evidence = match proofs.evidence() {
                Ok(evidence) => evidence,
                Err(error) => return tool_error(error, false),
            };
            // Native code is an engine choice, never a source verdict: the
            // same diagnostics come first, and a stage that cannot build the
            // program yet is a tool outcome (exit 2).
            return match hirsel_driver::native::run_path_native_backend(
                Path::new(&file),
                launch
                    .as_ref()
                    .map(|launch| launch.host(arguments.as_deref())),
                evidence.as_ref(),
                backend.unwrap_or_default(),
                hirsel_driver::native::Output::Inherit { json },
            ) {
                // The native driver already wrote the complete failure chain.
                Ok(Ok(RunOutcome::Failure { .. })) => 1,
                Ok(Ok(outcome)) => run_result(Ok(outcome), json),
                Ok(Err(error)) => {
                    eprintln!("hirsel: {error}");
                    2
                }
                Err(error) => tool_error(error, false),
            };
        }
        Command::Run {
            file,
            launch,
            native: false,
            backend: _,
            proofs: _,
            arguments,
            max_steps,
            json,
        } => {
            return run_result(
                match &launch {
                    Some(launch) => hirsel_driver::run_path_on_host(
                        Path::new(&file),
                        launch.host(arguments.as_deref()),
                        max_steps,
                    ),
                    // Without a record the simulated world grants no host authority.
                    None => hirsel_driver::run_path_with_step_budget(
                        Path::new(&file),
                        hirsel_engine::SimulationConfig::default(),
                        max_steps,
                        &mut std::io::stdout().lock(),
                        &mut std::io::stderr().lock(),
                    ),
                },
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
        "check" => match hirsel_driver::check_path(path) {
            Ok(output) => {
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
    match result {
        // The launch boundary has already written the PROG-3 payload report.
        Ok(RunOutcome::Exit(code)) => u8::from(code != 0),
        Ok(RunOutcome::Failure {
            rule,
            site,
            operation,
        }) => {
            eprintln!("{}", site.report(rule.as_str(), json, operation.as_ref()));
            1
        }
        Ok(RunOutcome::UndefinedBehavior(u)) => {
            eprintln!("{u}");
            2
        }
        Ok(RunOutcome::Trapped(t)) => {
            eprintln!("{t}");
            2
        }
        Ok(RunOutcome::Blocked(w)) => {
            eprintln!("Blocked: {w:?}");
            2
        }
        Ok(RunOutcome::ToolingLimit(l)) => {
            eprintln!("evaluator tooling limit: {:?}", l.kind);
            2
        }
        Err(e) => tool_error(e, json),
    }
}
