//! Inventory and derive native campaign inputs without running any program.
use std::path::PathBuf;

fn run() -> Result<(), String> {
    let mut root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut args = std::env::args().skip(1);
    let mut program = None;
    let mut derive = None;
    let mut replay = false;
    let mut filters = Vec::new();
    let mut seed = 1;
    let mut shard = None;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--root" => root = args.next().ok_or("--root needs a path")?.into(),
            "--program" => program = Some(args.next().ok_or("--program needs a path")?),
            "--derive" => derive = Some(args.next().ok_or("--derive needs an output path")?),
            "--replay-history" => replay = true,
            "--filter" => filters.push(args.next().ok_or("--filter needs text")?),
            "--shard" => {
                shard = Some(
                    hirsel_conformance::Shard::parse(&args.next().ok_or("--shard needs I/N")?)
                        .ok_or("invalid --shard")?,
                );
            }
            "--seed" => {
                seed = args
                    .next()
                    .ok_or("--seed needs an integer")?
                    .parse()
                    .map_err(|_| "invalid seed")?
            }
            _ => {
                return Err(format!(
                    "unknown argument {arg}; usage: native-select [--root PATH]"
                ));
            }
        }
    }
    if replay {
        let report =
            hirsel_conformance::native::diet::replay_history(&root, seed, &filters, shard)?;
        println!("{report:#}");
        if !report["lost"].as_array().unwrap().is_empty() {
            return Err("slim campaign lost historical findings".into());
        }
    } else if let Some(output) = derive {
        let selection = hirsel_conformance::native::diet::derive(&root)?;
        std::fs::write(output, serde_json::to_vec(&selection).unwrap())
            .map_err(|e| e.to_string())?;
    } else if let Some(program) = program {
        let source = std::fs::read_to_string(root.join(program)).map_err(|e| e.to_string())?;
        println!(
            "{}",
            serde_json::to_string_pretty(&hirsel_conformance::agree::catalogue_counts(&source)?)
                .unwrap()
        );
    } else {
        println!("{:#}", hirsel_conformance::native::diet::inventory(&root)?);
    }
    Ok(())
}

fn main() -> std::process::ExitCode {
    hirsel_engine::with_compiler_stack(|| match run() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("native-select: {error}");
            std::process::ExitCode::from(2)
        }
    })
}
