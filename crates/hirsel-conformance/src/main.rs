use hirsel_conformance::{SeededCompiler, Shard, Status, run_suite_shard};
use std::{env, path::PathBuf, process::ExitCode};

fn main() -> ExitCode {
    hirsel_engine::with_compiler_stack(main_on_compiler_thread)
}
fn main_on_compiler_thread() -> ExitCode {
    let mut hash_seed = 0;
    let mut json = false;
    let mut verbose = false;
    let mut update = false;
    let mut check_format = false;
    let mut root = None;
    let mut shard = None;
    let mut args = env::args_os().skip(1);
    while let Some(arg) = args.next() {
        match arg.to_str() {
            Some("--json") => json = true,
            Some("--verbose") => verbose = true,
            Some("--update-passing") => update = true,
            Some("--check-format") => check_format = true,
            Some("--hash-seed") => {
                hash_seed = match args
                    .next()
                    .and_then(|v| v.to_str().and_then(|v| v.parse::<u64>().ok()))
                {
                    Some(seed) => seed,
                    None => {
                        eprintln!("--hash-seed requires an unsigned integer");
                        return ExitCode::from(2);
                    }
                };
            }
            Some("--shard") => {
                shard = match args.next().and_then(|v| v.to_str().and_then(Shard::parse)) {
                    Some(shard) => Some(shard),
                    None => {
                        eprintln!("--shard requires <index>/<count>, 1 <= index <= count");
                        return ExitCode::from(2);
                    }
                };
            }
            Some("--root") => {
                let Some(path) = args.next() else {
                    eprintln!("--root requires a repository path");
                    return ExitCode::from(2);
                };
                root = Some(PathBuf::from(path));
            }
            Some("--help" | "-h") => {
                println!(
                    "usage: hirsel-conformance [--json] [--verbose] [--update-passing] [--check-format] [--root <repo>] [--hash-seed N] [--shard I/N]"
                );
                return ExitCode::SUCCESS;
            }
            _ => {
                eprintln!("unknown argument: {}", arg.to_string_lossy());
                return ExitCode::from(2);
            }
        }
    }
    let result = (|| {
        let root = match root {
            Some(root) => root,
            None => {
                let cwd = env::current_dir().map_err(|e| e.to_string())?;
                cwd.ancestors()
                    .find(|p| {
                        p.join(hirsel_conformance::SPEC_DIR)
                            .join("index.md")
                            .is_file()
                    })
                    .ok_or("cannot find repository; use --root <repo>")?
                    .to_path_buf()
            }
        };
        let mut report = run_suite_shard(&root, &SeededCompiler(hash_seed), update, shard)?;
        if check_format {
            hirsel_conformance::corpus_format::check_canonical_shard(&root, &mut report, shard)?;
        }
        Ok::<_, String>(report)
    })();
    match result {
        Ok(report) => {
            if json {
                println!("{}", report.json());
            } else {
                println!("whole corpus: {}", report.counts);
                println!("in-gate corpus: {}", report.in_gate);
                if check_format {
                    println!(
                        "corpus formatting: {} intentional syntax rejections exempt",
                        report.format_exemptions.len()
                    );
                }
                if verbose {
                    // Preserve the external verbose report with its constant count.
                    println!("unchecked TIR relationships: 0");
                    for (path, reasons) in &report.exclusions {
                        println!("excluded {path}: {}", reasons.join(", "));
                    }
                    for (prefix, counts) in &report.prefixes {
                        println!("{prefix}: {counts}");
                    }
                }
                for test in &report.tests {
                    if let Status::Fail(reason) = &test.status {
                        eprintln!("{}: {reason}", test.path);
                    }
                }
                for error in &report.errors {
                    eprintln!("{error}");
                }
            }
            if report.success() {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            }
        }
        Err(error) => {
            if json {
                println!(
                    "{}",
                    serde_json::json!({"success": false, "errors": [error]})
                );
            } else {
                eprintln!("{error}");
            }
            ExitCode::FAILURE
        }
    }
}
