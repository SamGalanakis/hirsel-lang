//! Inventory and derive native campaign inputs without running any program.
use hirsel_cli_args::{
    CommonOption, CommonOptions,
    lexopt::{Arg::*, Parser, ValueExt},
    parse_or_exit,
};
use std::path::PathBuf;

fn run() -> Result<(), String> {
    let mut program = None;
    let mut derive = None;
    let mut replay = false;
    let mut chunks = Vec::<PathBuf>::new();
    let mut inventory_input = None::<PathBuf>;
    let mut inventory_output = None::<PathBuf>;
    let mut filters = Vec::new();
    let mut seed = 1;
    let mut common = CommonOptions::default();
    parse_or_exit((|| {
        let mut args = Parser::from_env();
        while let Some(arg) = args.next()? {
            match arg {
                Long("root") => common.parse(CommonOption::Root, &mut args)?,
                Long("shard") => common.parse(CommonOption::Shard, &mut args)?,
                Long("program") => program = Some(PathBuf::from(args.value()?)),
                Long("derive") => derive = Some(PathBuf::from(args.value()?)),
                Long("replay-history") => replay = true,
                Long("inventory-input") => inventory_input = Some(args.value()?.into()),
                Long("inventory-output") => inventory_output = Some(args.value()?.into()),
                Long("merge") => chunks.push(args.value()?.into()),
                Long("filter") => filters.push(args.value()?.string()?),
                Long("seed") => seed = args.value()?.parse()?,
                _ => return Err(arg.unexpected()),
            }
        }
        Ok(())
    })());
    let root = common
        .root
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../.."));
    let shard = common.shard;
    if let Some(output) = inventory_output {
        let inventory = hirsel_conformance::native::diet::inventory(&root)?;
        std::fs::write(output, serde_json::to_vec(&inventory).unwrap())
            .map_err(|e| e.to_string())?;
    } else if replay {
        let report =
            hirsel_conformance::native::diet::replay_history(&root, seed, &filters, shard)?;
        println!("{report:#}");
        if !report["lost"].as_array().unwrap().is_empty() {
            return Err("slim campaign lost historical findings".into());
        }
    } else if let Some(output) = derive {
        let selection = if chunks.is_empty() {
            let inventory = inventory_input
                .map(|path| {
                    let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
                    serde_json::from_str(&text).map_err(|e| e.to_string())
                })
                .transpose()?;
            hirsel_conformance::native::diet::derive_shard(&root, shard, inventory)?
        } else {
            let values = chunks
                .iter()
                .map(|path| {
                    let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
                    serde_json::from_str(&text).map_err(|e| e.to_string())
                })
                .collect::<Result<Vec<_>, String>>()?;
            hirsel_conformance::native::diet::merge(&root, &values)?
        };
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
