//! The compiler-architecture oracles O1–O4 and annotation invariance O5
//! (`hirsel_conformance::oracle`).

hirsel_guard::install!();
use hirsel_cli_args::{
    CommonOption, CommonOptions,
    lexopt::{Arg::*, Parser, ValueExt},
    parse_or_exit,
};
use hirsel_conformance::oracle::{
    self, Config, Limits, Report, edits::Scenario, o1, o2, o3, o4, o5, write_artifacts,
};
use std::{
    env,
    path::{Path, PathBuf},
    process::ExitCode,
};

const USAGE: &str = "usage: oracle o1 [--path PREFIX]... [--limit N] [--jobs 1,2,8] [--salts 1,2,3|none] [--native] [COMMON]
       oracle o2 [--scenario NAME]... [COMMON]
       oracle o3 [--scenario NAME]... [COMMON]
       oracle o4 [--path PREFIX]... [--limit N] [COMMON]
       oracle o5 [--path PREFIX]... [--limit N] [--native-programs N] [COMMON]
       oracle produce [--jobs N] [--salt S] [--store dir:PATH] [--verify-store] [--verify on|off] [--limits steps=N,depth=N] [--native] --out DIR PROGRAM
COMMON: [--root REPO] [--shard I/N] [--work DIR] [--json] [--allow-todo]";

/// Exit status of `produce` when the configuration needs a missing switch.
const TODO: u8 = 3;

#[derive(Default)]
struct Options {
    common: CommonOptions,
    paths: Vec<String>,
    scenarios: Vec<String>,
    limit: Option<usize>,
    jobs: Option<Vec<usize>>,
    salts: Option<Vec<Option<u64>>>,
    native: bool,
    native_programs: usize,
    work: Option<PathBuf>,
    json: bool,
    allow_todo: bool,
    // produce
    config: Config,
    out: Option<PathBuf>,
    program: Option<PathBuf>,
}

fn main() -> ExitCode {
    let mut args = Parser::from_env();
    let command = parse_or_exit(args.next().and_then(|arg| match arg {
        Some(Value(command)) => Ok(command.string()?),
        Some(Long("help")) | Some(Short('h')) | None => Ok("help".into()),
        Some(arg) => Err(arg.unexpected()),
    }));
    if command == "help" {
        println!("{USAGE}");
        return ExitCode::SUCCESS;
    }
    let options = parse_or_exit(parse(&command, &mut args));
    match command.as_str() {
        "produce" => produce(options),
        "o1" | "o2" | "o3" | "o4" | "o5" => match campaign(&command, &options) {
            Ok(report) => {
                if options.json {
                    println!("{}", report.json());
                }
                eprint!("{}", report.text());
                if report.passed(options.allow_todo) {
                    ExitCode::SUCCESS
                } else {
                    ExitCode::FAILURE
                }
            }
            Err(error) => {
                eprintln!("oracle: {error}");
                ExitCode::from(2)
            }
        },
        _ => {
            eprintln!("oracle: unknown command {command}\n{USAGE}");
            ExitCode::from(2)
        }
    }
}

fn parse(command: &str, args: &mut Parser) -> Result<Options, hirsel_cli_args::lexopt::Error> {
    let mut o = Options::default();
    let list = |text: String| -> Vec<String> { text.split(',').map(str::to_string).collect() };
    while let Some(arg) = args.next()? {
        if let Some(option) = CommonOption::from_arg(&arg) {
            o.common.parse(option, args)?;
            continue;
        }
        match (command, arg) {
            ("o1" | "o4" | "o5", Long("path")) => o.paths.push(args.value()?.string()?),
            ("o1" | "o4" | "o5", Long("limit")) => o.limit = Some(args.value()?.parse()?),
            ("o1", Long("jobs")) => {
                o.jobs = Some(
                    list(args.value()?.string()?)
                        .iter()
                        .map(|j| j.parse().map_err(|_| format!("--jobs: {j}")))
                        .collect::<Result<_, _>>()?,
                )
            }
            ("o1", Long("salts")) => {
                o.salts = Some(
                    list(args.value()?.string()?)
                        .iter()
                        .map(|s| match s.as_str() {
                            "none" => Ok(None),
                            s => s.parse().map(Some).map_err(|_| format!("--salts: {s}")),
                        })
                        .collect::<Result<_, _>>()?,
                )
            }
            ("o1" | "produce", Long("native")) => {
                o.native = true;
                o.config.native = true;
            }
            ("o2" | "o3", Long("scenario")) => o.scenarios.push(args.value()?.string()?),
            ("o5", Long("native-programs")) => o.native_programs = args.value()?.parse()?,
            ("o1" | "o2" | "o3" | "o4" | "o5", Long("work")) => o.work = Some(args.value()?.into()),
            ("o1" | "o2" | "o3" | "o4" | "o5", Long("json")) => o.json = true,
            ("o1" | "o2" | "o3" | "o4" | "o5", Long("allow-todo")) => o.allow_todo = true,
            ("produce", Long("jobs")) => o.config.jobs = args.value()?.parse()?,
            ("produce", Long("salt")) => o.config.salt = Some(args.value()?.parse()?),
            ("produce", Long("store")) => {
                let store = args.value()?.string()?;
                let path = store
                    .strip_prefix("dir:")
                    .ok_or("--store takes dir:<path>")?;
                o.config.store = Some(path.into());
            }
            ("produce", Long("verify-store")) => o.config.verify_store = true,
            ("produce", Long("verify")) => {
                o.config.verify = match args.value()?.string()?.as_str() {
                    "on" => true,
                    "off" => false,
                    _ => return Err("--verify takes on or off".into()),
                }
            }
            ("produce", Long("limits")) => {
                o.config.limits = Some(args.value()?.string()?.parse::<Limits>()?)
            }
            ("produce", Long("out")) => o.out = Some(args.value()?.into()),
            ("produce", Value(program)) if o.program.is_none() => o.program = Some(program.into()),
            (_, arg) => return Err(arg.unexpected()),
        }
    }
    Ok(o)
}

fn produce(options: Options) -> ExitCode {
    let (Some(out), Some(program)) = (options.out, options.program) else {
        eprintln!("produce requires --out DIR and a program\n{USAGE}");
        return ExitCode::from(2);
    };
    match oracle::produce(&program, &options.config, &oracle::fresh_images()) {
        Ok(artifacts) => match write_artifacts(&out, &artifacts) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("produce: {}: {error}", out.display());
                ExitCode::FAILURE
            }
        },
        Err(switch) => {
            eprintln!("produce: {switch}");
            ExitCode::from(TODO)
        }
    }
}

fn campaign(command: &str, options: &Options) -> Result<Report, String> {
    let root = match &options.common.root {
        Some(root) => root.clone(),
        None => env::current_dir()
            .map_err(|e| e.to_string())?
            .ancestors()
            .find(|p| {
                p.join(hirsel_conformance::SPEC_DIR)
                    .join("index.md")
                    .is_file()
            })
            .ok_or("cannot find repository; use --root <repo>")?
            .to_path_buf(),
    };
    let exe = env::current_exe().map_err(|e| e.to_string())?;
    let (work, keep) = match &options.work {
        Some(work) => (work.clone(), true),
        None => (
            env::temp_dir().join(format!("hirsel-oracle-{}", std::process::id())),
            false,
        ),
    };
    std::fs::create_dir_all(&work).map_err(|e| format!("{}: {e}", work.display()))?;
    let report = (|| {
        Ok(match command {
            "o1" => {
                let programs =
                    oracle::corpus(&root, &options.paths, options.common.shard, options.limit)?;
                let jobs = options.jobs.clone().unwrap_or(vec![1, 2, 8]);
                let salts = options
                    .salts
                    .clone()
                    .unwrap_or(vec![Some(1), Some(2), Some(3)]);
                let cells = o1::cells(&jobs, &salts, options.native);
                o1::run(&exe, &root, &programs, &cells, &work)
            }
            "o2" => o2::run(&exe, &root, &scenarios(&root, "o2", options)?, &work),
            "o3" => o3::run(&scenarios(&root, "o3", options)?),
            "o4" => {
                let programs =
                    oracle::corpus(&root, &options.paths, options.common.shard, options.limit)?;
                o4::run(&exe, &root, &programs, &work)
            }
            _ => {
                let programs =
                    oracle::corpus(&root, &options.paths, options.common.shard, options.limit)?;
                let rules = hirsel_conformance::spec_rules(&root)?;
                o5::run(&root, &rules, &programs, options.native_programs, &work)
            }
        })
    })();
    if !keep {
        let _ = std::fs::remove_dir_all(&work);
    }
    report
}

/// The scenarios of `oracles/<oracle>/`, or those `--scenario` names, in `--shard`.
fn scenarios(root: &Path, oracle: &str, options: &Options) -> Result<Vec<Scenario>, String> {
    let dir = root.join(oracle::SCENARIOS).join(oracle);
    let mut paths: Vec<PathBuf> = std::fs::read_dir(&dir)
        .map_err(|e| format!("{}: {e}", dir.display()))?
        .map(|entry| entry.map(|e| e.path()).map_err(|e| e.to_string()))
        .collect::<Result<_, _>>()?;
    paths.retain(|p| p.extension().is_some_and(|e| e == "edit"));
    paths.sort();
    let name = |p: &PathBuf| p.file_stem().unwrap().to_string_lossy().into_owned();
    for wanted in &options.scenarios {
        if !paths.iter().any(|p| name(p) == *wanted) {
            return Err(format!("no scenario {wanted} in {}", dir.display()));
        }
    }
    paths.retain(|p| {
        (options.scenarios.is_empty() || options.scenarios.contains(&name(p)))
            && options.common.shard.is_none_or(|s| s.holds(&name(p)))
    });
    let scenarios = paths
        .iter()
        .map(|p| Scenario::read(p, root))
        .collect::<Result<Vec<_>, _>>()?;
    match scenarios.iter().find(|s| s.oracle != oracle) {
        Some(s) => Err(format!(
            "{}: an {} scenario in {}",
            s.name,
            s.oracle,
            dir.display()
        )),
        None => Ok(scenarios),
    }
}
