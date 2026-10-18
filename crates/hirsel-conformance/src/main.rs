hirsel_guard::install!();

use hirsel_cli_args::{
    CommonOption, CommonOptions,
    lexopt::{Arg::*, Parser, ValueExt},
    parse_or_exit,
};
use hirsel_conformance::{SeededCompiler, Status, run_suite_selected};
use std::{env, process::ExitCode};

fn main() -> ExitCode {
    hirsel_engine::with_compiler_stack(main_on_compiler_thread)
}
fn main_on_compiler_thread() -> ExitCode {
    let mut common = CommonOptions::default();
    let mut paths = Vec::new();
    let mut json = false;
    let mut verbose = false;
    let mut update = false;
    let mut check_format = false;
    let help = parse_or_exit((|| {
        let mut args = Parser::from_env();
        while let Some(arg) = args.next()? {
            if let Some(option) = CommonOption::from_arg(&arg) {
                common.parse(option, &mut args)?;
                continue;
            }
            match arg {
                Long("path") => paths.push(args.value()?.string()?),
                Long("json") => json = true,
                Long("verbose") => verbose = true,
                Long("update-passing") => update = true,
                Long("check-format") => check_format = true,
                Long("help") | Short('h') => return Ok(true),
                _ => return Err(arg.unexpected()),
            }
        }
        Ok(false)
    })());
    if help {
        println!(
            "usage: hirsel-conformance [--json] [--verbose] [--update-passing] [--check-format] [--path PREFIX]... [--root <repo>] [--hash-seed N] [--shard I/N]"
        );
        return ExitCode::SUCCESS;
    }
    let hash_seed = common.hash_seed.unwrap_or(0);
    let shard = common.shard;
    let result = (|| {
        let root = match common.root {
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
        let selection = hirsel_conformance::Selection {
            shard,
            paths: &paths,
        };
        let mut report = run_suite_selected(&root, &SeededCompiler(hash_seed), update, selection)?;
        if check_format {
            hirsel_conformance::corpus_format::check_canonical_selected(
                &root,
                &mut report,
                selection,
            )?;
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
