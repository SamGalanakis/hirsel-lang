use hirsel_engine::{QueryError, RunOutcome, ToolError};
use std::{env, ffi::OsString, path::Path, process::ExitCode};

const USAGE: &str = "usage: hirsel check [--json] <source|manifest.json> | run [--native | --simulation <json>] <source|manifest.json> | fmt [--check] <file> | export --tir <source|manifest.json> -o <file>";

fn main() -> ExitCode {
    hirsel_engine::with_compiler_stack(|| ExitCode::from(execute(env::args_os().skip(1).collect())))
}
fn execute(args: Vec<OsString>) -> u8 {
    if let [command, flag, entry, output_flag, output] = args.as_slice()
        && command == "export"
        && flag == "--tir"
        && output_flag == "-o"
    {
        // The prover-neutral TIR export: a tool artifact,
        // never a source verdict, so diagnostics come first as for `run`.
        return match hirsel_driver::export_tir_path(Path::new(entry)) {
            Ok(text) => match std::fs::write(output, text) {
                Ok(()) => 0,
                Err(error) => tool_error(ToolError::Io(error), false),
            },
            Err(error) => tool_error(error, false),
        };
    }
    if let [command, flag, config, file] = args.as_slice() {
        if command == "run" && flag == "--simulation" {
            let simulation = match read_simulation(Path::new(config)) {
                Ok(config) => config,
                Err(error) => {
                    eprintln!("hirsel: simulation: {error}");
                    return 2;
                }
            };
            return match hirsel_driver::run_path_with_simulation(
                Path::new(file),
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
    }
    if let [command, flag, file] = args.as_slice() {
        if command == "run" && flag == "--native" {
            // Native code is an engine choice, never a source verdict: the
            // same diagnostics come first, and a stage that cannot build the
            // program yet is a tool outcome (exit 2).
            return match hirsel_driver::native::run_path_native(
                Path::new(file),
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
    }
    if let [command] = args.as_slice() {
        if command == "--help" || command == "help" {
            println!("{USAGE}");
            return 0;
        }
    }
    let (command, flag, file) = match args.as_slice() {
        [command, file] if ["check", "run", "fmt"].iter().any(|c| command == c) => {
            (command.to_string_lossy(), None, file)
        }
        [command, flag, file]
            if (command == "check" && flag == "--json")
                || (command == "fmt" && flag == "--check") =>
        {
            (command.to_string_lossy(), Some(flag.as_os_str()), file)
        }
        _ => return usage(),
    };
    if file.to_string_lossy().starts_with('-') {
        return usage();
    }
    let path = Path::new(file);
    let json = flag.is_some_and(|f| f == "--json");
    match command.as_ref() {
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
