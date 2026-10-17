use hirsel_cli_args::lexopt::{self, Arg::*, Parser};
use hirsel_engine::{QueryError, RunOutcome, ToolError};
use std::{env, ffi::OsString, path::Path, process::ExitCode};

const USAGE: &str = "usage: hirsel check [--json] <source|manifest.json> | run [--native | --simulation <json>] <source|manifest.json> | fmt [--check] <file> | export --tir <source|manifest.json> -o <file>";

fn main() -> ExitCode {
    hirsel_engine::with_compiler_stack(|| ExitCode::from(execute(env::args_os().skip(1).collect())))
}
enum Command {
    Help,
    Export {
        entry: OsString,
        output: OsString,
    },
    Simulation {
        config: OsString,
        file: OsString,
    },
    Native {
        file: OsString,
    },
    Source {
        command: &'static str,
        flag: Option<&'static str>,
        file: OsString,
    },
}

fn parse_command(args: Vec<OsString>) -> Result<Command, lexopt::Error> {
    let mut args = Parser::from_args(args);
    let command = match args.next()? {
        Some(Long("help")) => Command::Help,
        Some(Value(command)) if command == "help" => Command::Help,
        Some(Value(command)) if command == "export" => {
            if !matches!(args.next()?, Some(Long("tir"))) {
                return Err("export requires --tir".into());
            }
            let entry = args.value()?;
            if !matches!(args.next()?, Some(Short('o'))) {
                return Err("export requires -o".into());
            }
            Command::Export {
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
            match args.next()? {
                Some(Long("simulation")) if name == "run" => Command::Simulation {
                    config: args.value()?,
                    file: args.value()?,
                },
                Some(Long("native")) if name == "run" => Command::Native {
                    file: args.value()?,
                },
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
                Some(Value(file)) if !file.to_string_lossy().starts_with('-') => Command::Source {
                    command: name,
                    flag: None,
                    file,
                },
                Some(arg) => return Err(arg.unexpected()),
                None => return Err("missing source path".into()),
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
        Err(_) => return usage(),
    };
    let (command, flag, file) = match command {
        Command::Help => {
            println!("{USAGE}");
            return 0;
        }
        Command::Export { entry, output } => {
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
        Command::Simulation { config, file } => {
            let simulation = match read_simulation(Path::new(&config)) {
                Ok(config) => config,
                Err(error) => {
                    eprintln!("hirsel: simulation: {error}");
                    return 2;
                }
            };
            return match hirsel_driver::run_path_with_simulation(
                Path::new(&file),
                simulation,
                &mut std::io::stdout().lock(),
            ) {
                Ok(RunOutcome::Exit(code)) => u8::from(code != 0),
                Ok(RunOutcome::Failure { rule, line }) => {
                    eprintln!("runtime failure[{rule}] at line {line}");
                    1
                }
                Ok(RunOutcome::UndefinedBehavior(detected)) => {
                    eprintln!("{detected}");
                    2
                }
                Ok(RunOutcome::Trapped(trap)) => {
                    eprintln!("{trap}");
                    2
                }
                Ok(RunOutcome::Blocked(waits)) => {
                    eprintln!("Blocked: {waits:?}");
                    2
                }
                Ok(RunOutcome::ToolingLimit(limit)) => {
                    eprintln!("evaluator tooling limit: {:?}", limit.kind);
                    2
                }
                Err(error) => tool_error(error, false),
            };
        }
        Command::Native { file } => {
            // Native code is an engine choice, never a source verdict: the
            // same diagnostics come first, and a stage that cannot build the
            // program yet is a tool outcome (exit 2).
            return match hirsel_driver::native::run_path_native(
                Path::new(&file),
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
        "run" => match hirsel_driver::run_path(path, &mut std::io::stdout().lock()) {
            Ok(RunOutcome::UndefinedBehavior(detected)) => {
                eprintln!("{detected}");
                2
            }
            Ok(RunOutcome::Trapped(trap)) => {
                eprintln!("{trap}");
                2
            }
            Ok(RunOutcome::Blocked(waits)) => {
                eprintln!("Blocked: {waits:?}");
                2
            }
            Ok(RunOutcome::ToolingLimit(limit)) => {
                eprintln!("evaluator tooling limit: {:?}", limit.kind);
                2
            }
            Ok(RunOutcome::Exit(0)) => 0,
            Ok(RunOutcome::Exit(_)) => 1,
            Ok(RunOutcome::Failure { rule, line }) => {
                eprintln!("runtime failure[{rule}] at line {line}");
                1
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
                                "stdout" | "tasks" | "net" | "random" | "dir"
                            ) {
                                return Err(format!("unknown Io projection {projection}"));
                            }
                        }
                        config.projections = values.into_iter().collect();
                    }
                    _ => config.directories = values.into_iter().collect(),
                }
            }
            "files" | "env" => {
                let entries = value
                    .as_object()
                    .ok_or_else(|| format!("{key}: expected an object"))?;
                for (name, text) in entries {
                    let text = text
                        .as_str()
                        .ok_or_else(|| format!("{key}: expected string values"))?;
                    if key == "files" {
                        config.files.insert(name.clone(), text.as_bytes().to_vec());
                    } else {
                        config.env.insert(name.clone(), text.into());
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
