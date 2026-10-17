use hirsel_cli_args::{
    CommonOption, CommonOptions,
    lexopt::{Arg::*, Parser, ValueExt},
    parse_or_exit,
};
use std::{path::PathBuf, process::ExitCode};
const KNOWN: &str = "native-tests/regressions/agree-known.txt";
fn main() -> ExitCode {
    hirsel_engine::with_compiler_stack(main_on_compiler_thread)
}
fn main_on_compiler_thread() -> ExitCode {
    let mut seed = std::env::var("PROPTEST_RNG_SEED")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let mut budget = Some(hirsel_conformance::agree::DEFAULT_VARIANTS_PER_PROGRAM);
    let mut exhaustive = false;
    let mut limit = usize::MAX;
    let mut native = None;
    let mut program = None;
    let mut only = vec![];
    let mut common = CommonOptions::default();
    let help = parse_or_exit((|| {
        let mut args = Parser::from_env();
        while let Some(arg) = args.next()? {
            if let Some(option) = CommonOption::from_arg(&arg) {
                common.parse(option, &mut args)?;
                continue;
            }
            match arg {
                Long("exhaustive") => exhaustive = true,
                Long("help") | Short('h') => return Ok(true),
                Long("variants-per-program") => {
                    let n = args.value()?.parse::<usize>()?;
                    if n == 0 {
                        return Err("--variants-per-program needs a positive integer".into());
                    }
                    budget = Some(n);
                }
                Long("program") => program = Some(PathBuf::from(args.value()?)),
                Long("path") => only.push(args.value()?.string()?),
                Long("native") => {
                    let mode = args.value()?.string()?;
                    if mode != "heap" && mode != "full" {
                        return Err("--native takes heap or full".into());
                    }
                    native = Some(mode);
                }
                Long("seed") => seed = args.value()?.parse()?,
                Long("limit") => limit = args.value()?.parse()?,
                _ => return Err(arg.unexpected()),
            }
        }
        Ok(false)
    })());
    if help {
        println!(
            "usage: agree [--seed N] [--hash-seed N] [--limit M] [--root PATH] [--native heap|full] [--program PATH] [--path PREFIX]... [--shard I/N] [--variants-per-program N | --exhaustive]"
        );
        return ExitCode::SUCCESS;
    }
    let root = common
        .root
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../.."));
    let hash_seed = common.hash_seed.unwrap_or(0);
    let shard = common.shard;
    if shard.is_some() && program.is_some() {
        eprintln!("--shard cannot be combined with --program");
        return ExitCode::from(2);
    }
    // The runtime extension: valid executed variants also run on the heap
    // executor with fault injection (`heap`), and as an optimized x86-64
    // executable too (`full`).
    let native = match native {
        None => None,
        Some(mode) => {
            use hirsel_driver::native::{Engine, Toolchain, smoke_matrix};
            let mut matrix = smoke_matrix();
            for config in &mut matrix {
                config.options.launch.hash_seed = hash_seed;
            }
            if mode == "heap" {
                matrix.retain(|c| matches!(c.engine, Engine::Heap { .. }));
            }
            let built = hirsel_conformance::native::runtime_cache()
                .and_then(|dir| Toolchain::for_matrix(&dir, &matrix).map_err(|e| e.to_string()))
                .and_then(|toolchain| {
                    Ok(hirsel_conformance::agree::Native::new(
                        matrix,
                        toolchain,
                        hirsel_conformance::native::scratch("agree")?,
                        hirsel_conformance::native::read_exclusions(&root)?,
                    ))
                });
            match built {
                Ok(native) => Some(native),
                Err(e) => {
                    eprintln!("{e}");
                    return ExitCode::from(2);
                }
            }
        }
    };
    let native = match native {
        Some(native) if !exhaustive && program.is_none() => {
            match hirsel_conformance::native::diet::Selection::load(&root) {
                Ok(selection) => Some(native.with_selection(selection, seed)),
                Err(error) => {
                    eprintln!("{error}");
                    return ExitCode::from(2);
                }
            }
        }
        native => native,
    };
    if let Some(path) = program {
        use hirsel_conformance::agree::Classification;
        let result = match hirsel_conformance::agree::replay_program_with_hash_seed(
            &path,
            native.as_ref(),
            hash_seed,
        ) {
            Ok(result) => result,
            Err(error) => {
                eprintln!("{error}");
                return ExitCode::from(2);
            }
        };
        let mut json = serde_json::json!({ "program": path, "hash_seed": hash_seed, "classification": format!("{result:?}") });
        let mut status = match &result {
            Classification::Bug { kind, detail } => {
                json["kind"] = kind.clone().into();
                json["detail"] = detail.clone().into();
                ExitCode::FAILURE
            }
            Classification::Unavailable | Classification::RuntimeLimit => ExitCode::from(2),
            _ => ExitCode::SUCCESS,
        };
        if let Some(native) = &native {
            let stats = native.stats.lock().unwrap();
            json["native_agreed"] = stats.agreed.into();
            json["native_unavailable"] = serde_json::json!(stats.unavailable);
            if !matches!(result, Classification::Bug { .. })
                && (stats.agreed == 0 || !stats.unavailable.is_empty())
            {
                status = ExitCode::from(2);
            }
        }
        println!("{json:#}");
        return status;
    }
    match hirsel_conformance::agree::run_native_with_hash_seed(
        &root,
        seed,
        hirsel_conformance::agree::Sample {
            limit,
            per_program: if exhaustive { None } else { budget },
        },
        native.as_ref(),
        hash_seed,
        &only,
        shard,
    ) {
        Ok(mut report) => {
            report.shrink_native(native.as_ref());
            let mut json = report.json();
            json["seed"] = seed.into();
            json["limit"] = limit.into();
            if let Some(shard) = shard {
                json["shard"] = serde_json::json!({ "index": shard.index, "count": shard.count });
            }
            if let Some(native) = &native {
                let stats = native.stats.lock().unwrap().clone();
                json["native_agreed"] = stats.agreed.into();
                json["native_unavailable"] = serde_json::json!(stats.unavailable);
            }
            // Native discrepancies are held to the named kinds in
            // native-tests/regressions/agree-known.txt: a new kind fails, and
            // so does a listed kind the whole catalogue no longer shows. A
            // shard sees part of the catalogue, so only the merge of every
            // shard (tools/buck2/agree.py --native-full) judges the latter.
            let mut ok = true;
            if native.is_some() {
                let listed = match std::fs::read_to_string(root.join(KNOWN)) {
                    Ok(text) => text,
                    Err(e) => {
                        eprintln!("{KNOWN}: {e}");
                        return ExitCode::from(2);
                    }
                };
                let listed: std::collections::BTreeSet<&str> = listed
                    .lines()
                    .filter(|l| !l.is_empty() && !l.starts_with('#'))
                    .collect();
                let found: std::collections::BTreeSet<&str> = report
                    .clusters
                    .keys()
                    .map(|(kind, _)| kind.as_str())
                    .collect();
                for kind in found.difference(&listed) {
                    eprintln!("agree: new discrepancy kind: {kind}");
                    ok = false;
                }
                if limit == usize::MAX && only.is_empty() && shard.is_none() {
                    for kind in listed.difference(&found) {
                        eprintln!("agree: {kind} no longer occurs; delete it from {KNOWN}");
                        ok = false;
                    }
                }
                json["known_kinds"] = listed.len().into();
            } else {
                ok = report.clusters.is_empty();
            }
            println!("{json:#}");
            if ok {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            }
        }
        Err(e) => {
            eprintln!("{e}");
            ExitCode::from(2)
        }
    }
}
