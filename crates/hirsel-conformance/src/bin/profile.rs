//! Compile once, keep physical counts separate from sampled cycles, report source lines.
use hirsel_cli_args::{
    CommonOption, CommonOptions,
    lexopt::{self, Arg::*, Parser},
    parse_or_exit,
};
use hirsel_driver::native::{
    self, OptLevel, Options, Partition, RuntimeBuild, TargetSpec, Toolchain,
};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

fn main() {
    hirsel_engine::with_compiler_stack(|| {
        if let Err(error) = profile() {
            eprintln!("profile: {error}");
            std::process::exit(1);
        }
    });
}

struct Arguments {
    program: PathBuf,
    cycles: bool,
    output: Option<PathBuf>,
    input_root: Option<PathBuf>,
    program_args: Vec<String>,
}

fn arguments() -> Result<Arguments, lexopt::Error> {
    let mut args = Parser::from_env();
    let program = args.value()?.into();
    let mut cycles = false;
    let mut output = None;
    let mut common = CommonOptions::default();
    let mut program_args = Vec::new();
    loop {
        // Keep the program's option spellings untouched after the separator.
        if args
            .try_raw_args()
            .is_some_and(|mut raw| raw.peek().is_some_and(|arg| arg == "--"))
        {
            use lexopt::ValueExt;
            let mut raw = args.raw_args()?;
            raw.next();
            program_args = raw.map(|value| value.string()).collect::<Result<_, _>>()?;
            break;
        }
        let Some(arg) = args.next()? else {
            break;
        };
        match arg {
            Long("cycles") => cycles = true,
            Long("root") => {
                common.parse(CommonOption::Root, &mut args)?;
            }
            Long("output") => output = Some(args.value()?.into()),
            _ => return Err(arg.unexpected()),
        }
    }
    Ok(Arguments {
        program,
        cycles,
        output,
        input_root: common.root,
        program_args,
    })
}

fn profile() -> Result<(), Box<dyn std::error::Error>> {
    let Arguments {
        program,
        cycles,
        output,
        input_root,
        mut program_args,
    } = parse_or_exit(arguments());
    let stem = program
        .file_stem()
        .ok_or("program needs a filename")?
        .to_string_lossy();
    let output =
        output.unwrap_or_else(|| PathBuf::from("target/source-profile").join(stem.as_ref()));
    std::fs::create_dir_all(&output)?;
    let output = std::fs::canonicalize(output)?;
    let benchmark = program.canonicalize()?.parent()
        == Some(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../bench/native/programs")
                .canonicalize()?
                .as_path(),
        );
    let root = input_root.unwrap_or_else(|| output.join("input-root"));
    std::fs::create_dir_all(&root)?;
    let root = root.canonicalize()?;
    if benchmark {
        let input = program_args
            .get_mut(1)
            .ok_or("benchmark requires case and input arguments after --")?;
        std::fs::copy(&*input, root.join("input"))?;
        *input = "input".to_owned();
    }
    let source = if benchmark {
        benchmark_source(&program, &output)?
    } else {
        program.clone()
    };
    let mut loaded = hirsel_driver::load_program(&source)?;
    loaded.check_for_execution()?;
    let snapshot = loaded
        .engine
        .export_native_input()
        .map_err(|e| format!("{e:?}"))?;
    let mut sources = native::profile_sources(&snapshot);
    let repo = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()?;
    for source in sources.values_mut() {
        if let Ok(relative) = source.path.strip_prefix(&repo) {
            source.path = relative.to_path_buf();
        }
    }
    let validated = loaded
        .engine
        .import_native_input(snapshot)
        .map_err(|e| format!("{e:?}"))?;
    let target = TargetSpec::x86_64_unknown_linux_gnu();
    let mut options = Options::new(target.clone());
    // Profile heap ownership, including snapshot duplication, rather than hide it in regions.
    options.regions = false;
    let ready = native::build(&validated, &options)?;
    let tools = Toolchain::in_tree_with_runtime(
        &output.join("runtime"),
        &[&target],
        RuntimeBuild {
            measurement: true,
            ..Default::default()
        },
    )?;
    let measured = output.join("measurement-objects");
    std::fs::create_dir_all(&measured)?;
    let executable = native::link_profile(
        &ready,
        &loaded.entry,
        OptLevel::Speed,
        Partition::Whole,
        &tools,
        &measured,
        Some(&sources),
    )?;
    let json = output.join("profile.json");
    let mut command = Command::new("nice");
    command
        .args(["-n", "10"])
        .arg(&executable)
        .args(&program_args)
        .env("HIRSEL_RT_MEASUREMENT", output.join("totals.txt"))
        .env("HIRSEL_RT_PROFILE", &json)
        .env("HIRSEL_RT_ROOT", &root);
    let (status, rss) = watched(&mut command, &output, "counts")?;
    if !status.success() {
        return Err(format!(
            "measured program failed: {}",
            std::fs::read_to_string(output.join("counts.stderr"))?
        )
        .into());
    }
    let report: Value = serde_json::from_slice(&std::fs::read(&json)?)?;
    print_counts(&report)?;
    println!("profile-json {}", json.display());
    println!("profile-max-rss-kib {rss}");
    if cycles {
        if Command::new("perf").arg("--version").output().is_err() {
            println!("cycles skipped perf-unavailable");
        } else {
            let tools = Toolchain::in_tree(&output.join("runtime"), &[&target])?;
            let timing = output.join("cycles-objects");
            std::fs::create_dir_all(&timing)?;
            let executable = native::link_profile(
                &ready,
                &loaded.entry,
                OptLevel::Speed,
                Partition::Whole,
                &tools,
                &timing,
                Some(&sources),
            )?;
            let mut command = Command::new("nice");
            command
                .args(["-n", "10", "perf", "record", "-q", "-e", "cycles:u", "-o"])
                .arg(output.join("perf.data"))
                .arg("--")
                .arg(executable)
                .args(&program_args)
                .env_remove("HIRSEL_RT_MEASUREMENT")
                .env_remove("HIRSEL_RT_PROFILE")
                .env("HIRSEL_RT_ROOT", &root);
            let (status, _) = watched(&mut command, &output, "cycles")?;
            if status.success() {
                let report = Command::new("perf")
                    .args([
                        "report",
                        "--stdio",
                        "--sort",
                        "srcline",
                        "--percent-limit",
                        "0",
                        "-i",
                    ])
                    .arg(output.join("perf.data"))
                    .output()?;
                std::fs::write(output.join("cycles.txt"), &report.stdout)?;
                if report.status.success() {
                    let text = String::from_utf8_lossy(&report.stdout);
                    let rows: Vec<_> = text
                        .lines()
                        .filter(|l| l.trim_start().starts_with(|c: char| c.is_ascii_digit()))
                        .take(20)
                        .collect();
                    if rows.is_empty() {
                        println!("cycles skipped no-samples");
                    } else {
                        println!("cycles percent source");
                        for line in rows {
                            println!("cycles {}", line.trim());
                        }
                    }
                } else {
                    println!("cycles skipped no-samples");
                }
            } else {
                let reason = std::fs::read_to_string(output.join("cycles.stderr"))?;
                let lower = reason.to_ascii_lowercase();
                if ![
                    "permission",
                    "not permitted",
                    "paranoid",
                    "not supported",
                    "not available",
                ]
                .iter()
                .any(|term| lower.contains(term))
                {
                    return Err(format!("cycle run failed: {reason}").into());
                }
                println!(
                    "cycles skipped {}",
                    reason
                        .lines()
                        .find(|l| !l.trim().is_empty())
                        .unwrap_or("perf-refused")
                        .trim()
                );
            }
        }
    }
    Ok(())
}

fn benchmark_source(program: &Path, output: &Path) -> Result<PathBuf, Box<dyn std::error::Error>> {
    // Native benchmark ports deliberately omit the shared launcher. Keep the workload
    // first, so all its original source line numbers remain unchanged.
    let Some(parent) = program.parent() else {
        return Ok(program.to_owned());
    };
    let common = parent.join("common.hir");
    if !common.is_file() {
        return Ok(program.to_owned());
    }
    let source = output.join(program.file_name().ok_or("filename")?);
    std::fs::write(
        &source,
        format!(
            "{}\n{}",
            std::fs::read_to_string(program)?,
            std::fs::read_to_string(common)?
        ),
    )?;
    let stdlib = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../stdlib/std@1/hirsel.json")
        .canonicalize()?;
    std::fs::write(
        source.with_extension("hir.json"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "schema": 1, "package": {"name":"benchmark","version":"1"},
            "entry":{"module":[],"name":"main"}, "sources":[{"path":source.file_name().unwrap().to_string_lossy(),"module":[]}],
            "dependencies":[{"manifest":stdlib,"package":{"name":"std","version":"1"},"expose":[]}]
        }))?,
    )?;
    Ok(source)
}

fn print_counts(report: &Value) -> Result<(), Box<dyn std::error::Error>> {
    let sites = report["sites"].as_array().ok_or("profile has no sites")?;
    let totals = report["totals"]
        .as_object()
        .ok_or("profile has no totals")?;
    for (kind, total) in totals {
        let total = total.as_u64().ok_or("invalid total")?;
        let mut rows: BTreeMap<(String, u64), u64> = BTreeMap::new();
        for site in sites {
            let count = site["counts"][kind].as_u64().unwrap_or(0);
            *rows
                .entry((
                    site["file"]
                        .as_str()
                        .filter(|s| !s.is_empty())
                        .unwrap_or("<runtime>")
                        .to_owned(),
                    site["line"].as_u64().unwrap_or(0),
                ))
                .or_default() += count;
        }
        if rows.values().sum::<u64>() != total {
            return Err(format!("{kind}: site sum differs from total").into());
        }
        if total == 0 {
            continue;
        }
        println!("counts {kind} total={total}");
        let mut rows: Vec<_> = rows.into_iter().filter(|(_, n)| *n > 0).collect();
        rows.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        for ((file, line), count) in rows.into_iter().take(10) {
            println!(
                "site {kind} {count} {:.2}% {file}:{line}",
                count as f64 * 100.0 / total as f64
            );
        }
    }
    Ok(())
}

/// Watch the process and descendants; perf forks the measured executable. Kill
/// the measured tree before returning on the shared host's fixed 8 GiB limit.
fn watched(
    command: &mut Command,
    output: &Path,
    name: &str,
) -> std::io::Result<(std::process::ExitStatus, u64)> {
    command
        .stdout(std::fs::File::create(
            output.join(format!("{name}.stdout")),
        )?)
        .stderr(std::fs::File::create(
            output.join(format!("{name}.stderr")),
        )?);
    let mut child = command.spawn()?;
    let mut peak = 0;
    loop {
        let mut pids = vec![child.id()];
        let mut rss = 0;
        let mut i = 0;
        while i < pids.len() {
            let pid = pids[i];
            if let Ok(status) = std::fs::read_to_string(format!("/proc/{pid}/status")) {
                rss += status
                    .lines()
                    .find(|l| l.starts_with("VmRSS:"))
                    .and_then(|l| l.split_whitespace().nth(1))
                    .and_then(|n| n.parse::<u64>().ok())
                    .unwrap_or(0);
            }
            if let Ok(children) =
                std::fs::read_to_string(format!("/proc/{pid}/task/{pid}/children"))
            {
                pids.extend(
                    children
                        .split_whitespace()
                        .filter_map(|p| p.parse::<u32>().ok()),
                );
            }
            i += 1;
        }
        peak = peak.max(rss);
        if rss > 8 * 1024 * 1024 {
            for pid in pids.iter().rev() {
                let _ = Command::new("kill")
                    .args(["-KILL", &pid.to_string()])
                    .status();
            }
            let _ = child.wait();
            return Err(std::io::Error::other("profile exceeded 8 GiB RSS"));
        }
        if let Some(status) = child.try_wait()? {
            return Ok((status, peak));
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}
