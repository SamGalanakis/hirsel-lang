use hirsel_cli_args::lexopt::{self, Arg::*, Parser};
use hirsel_engine::{QueryError, RunOutcome, ToolError};
use std::{env, ffi::OsString, path::Path, process::ExitCode};

const USAGE: &str = "usage: hirsel check [--json] <source|manifest.json> | run [--native [--proofs <sidecar> [--trust <tool@version>]...] | --simulation <json>] <source|manifest.json> [--run <json> | --grants <json>] [--max-steps <n>] | build [--release | --debug] [--target x86_64-linux | aarch64-linux] [--proofs <sidecar> [--trust <tool@version>]...] <source|manifest.json> -o <exe> | fmt [--check] <file> | export --tir|--run <source|manifest.json> -o <file>";

const DEFAULT_RUN_MAX_STEPS: u64 = 1_000_000_000;

fn main() -> ExitCode {
    hirsel_engine::with_compiler_stack(|| ExitCode::from(execute(env::args_os().skip(1).collect())))
}
enum Command {
    Help,
    Build {
        source: OsString,
        output: OsString,
        target: hirsel_driver::native::TargetSpec,
        profile: hirsel_driver::native::BuildProfile,
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
        max_steps: u64,
    },
    Run {
        file: OsString,
        record: Option<(bool, OsString)>,
        native: bool,
        proofs: Proofs,
        max_steps: u64,
    },
    Source {
        command: &'static str,
        flag: Option<&'static str>,
        file: OsString,
    },
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
        Some(Value(command)) if command == "build" => {
            use hirsel_driver::native::{BuildProfile, TargetSpec};
            let (mut source, mut output, mut target, mut profile) = (None, None, None, None);
            let mut proofs = Proofs::default();
            while let Some(arg) = args.next()? {
                match arg {
                    Long("proofs") if proofs.sidecar.is_none() => {
                        proofs.sidecar = Some(args.value()?)
                    }
                    Long("trust") => proofs.trust(args.value()?)?,
                    Long("release") if profile.is_none() => profile = Some(BuildProfile::Release),
                    Long("debug") if profile.is_none() => profile = Some(BuildProfile::Debug),
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
                proofs: {
                    proofs.check()?;
                    proofs
                },
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
        Some(Value(command)) if command == "check" || command == "run" || command == "fmt" => {
            let name = match command.to_str() {
                Some("check") => "check",
                Some("run") => "run",
                _ => "fmt",
            };
            if name == "run" {
                let mut record = None;
                let mut native = false;
                let mut simulation = None;
                let mut max_steps = None;
                let mut file = None;
                let mut proofs = Proofs::default();
                while let Some(arg) = args.next()? {
                    match arg {
                        Long("proofs") if proofs.sidecar.is_none() => {
                            proofs.sidecar = Some(args.value()?)
                        }
                        Long("trust") => proofs.trust(args.value()?)?,
                        Long("native") if !native => native = true,
                        Long("run" | "grants") if record.is_none() => {
                            record = Some((matches!(arg, Long("grants")), args.value()?));
                        }
                        Long("max-steps") if max_steps.is_none() => {
                            max_steps = Some(
                                args.value()
                                    .ok()
                                    .and_then(|value| value.to_str()?.parse::<u64>().ok())
                                    .ok_or("--max-steps requires an unsigned integer")?,
                            );
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
                let max_steps = max_steps.unwrap_or(DEFAULT_RUN_MAX_STEPS);
                if let Some(config) = simulation {
                    if native || record.is_some() {
                        return Err(
                            "--simulation cannot combine with --native, --run or --grants".into(),
                        );
                    }
                    Command::Simulation {
                        config,
                        file,
                        max_steps,
                    }
                } else {
                    Command::Run {
                        file,
                        record,
                        native,
                        proofs,
                        max_steps,
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

fn execute(args: Vec<OsString>) -> u8 {
    let command = match parse_command(args) {
        Ok(command) => command,
        Err(error) => {
            eprintln!("{error}");
            return usage();
        }
    };
    let (command, flag, file) = match command {
        Command::Help => {
            println!("{USAGE}");
            return 0;
        }
        Command::Build {
            source,
            output,
            target,
            profile,
            proofs,
        } => {
            let evidence = match proofs.evidence() {
                Ok(evidence) => evidence,
                Err(error) => return tool_error(error, false),
            };
            return match hirsel_driver::native::build_path_native(
                Path::new(&source),
                Path::new(&output),
                target,
                profile,
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
        } => {
            let simulation = match read_simulation(Path::new(&config)) {
                Ok(config) => config,
                Err(error) => {
                    eprintln!("hirsel: simulation: {error}");
                    return 2;
                }
            };
            return run_result(hirsel_driver::run_path_with_step_limit(
                Path::new(&file),
                simulation,
                max_steps,
                &mut std::io::stdout().lock(),
                &mut std::io::stderr().lock(),
            ));
        }
        Command::Run {
            file,
            record,
            native: true,
            proofs,
            max_steps: _,
        } => {
            let evidence = match proofs.evidence() {
                Ok(evidence) => evidence,
                Err(error) => return tool_error(error, false),
            };
            // Native code is an engine choice, never a source verdict: the
            // same diagnostics come first, and a stage that cannot build the
            // program yet is a tool outcome (exit 2).
            return match hirsel_driver::native::run_path_native_with_config(
                Path::new(&file),
                record.as_ref().map(|(grants, path)| {
                    if *grants {
                        hirsel_driver::LaunchConfig::Grants(Path::new(path))
                    } else {
                        hirsel_driver::LaunchConfig::Record(Path::new(path))
                    }
                }),
                evidence.as_ref(),
                &mut std::io::stdout().lock(),
                &mut std::io::stderr().lock(),
            ) {
                Ok(Ok(RunOutcome::Exit(code))) => u8::from(code != 0),
                Ok(Ok(_)) => 1,
                Ok(Err(error)) => {
                    eprintln!("hirsel: {error}");
                    2
                }
                Err(error) => tool_error(error, false),
            };
        }
        Command::Run {
            file,
            record,
            native: false,
            proofs: _,
            max_steps,
        } => {
            return run_result(hirsel_driver::run_path_with_config(
                Path::new(&file),
                record.as_ref().map(|(grants, path)| {
                    if *grants {
                        hirsel_driver::LaunchConfig::Grants(Path::new(path))
                    } else {
                        hirsel_driver::LaunchConfig::Record(Path::new(path))
                    }
                }),
                max_steps,
                &mut std::io::stdout().lock(),
            ));
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
                if output.diagnostics.is_empty() {
                    if let hirsel_engine::CheckCompletion::Incomplete { phase } = output.completion
                    {
                        eprintln!("{} is not implemented yet", phase.phase_name());
                        return 2;
                    }
                }
                u8::from(
                    output
                        .diagnostics
                        .iter()
                        .any(|d| d.severity.as_str() == "error"),
                )
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
    } else {
        eprintln!("hirsel: {error}");
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
                                "stdin" | "stderr" | "stdout" | "tasks" | "net" | "random" | "dir"
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

fn run_result(result: Result<RunOutcome, ToolError>) -> u8 {
    match result {
        Ok(RunOutcome::Exit(code)) => u8::from(code != 0),
        Ok(RunOutcome::Failure { rule, line }) => {
            eprintln!("runtime failure[{rule}] at line {line}");
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
        Err(e) => tool_error(e, false),
    }
}
