//! Compile source outside measurements; timing launches only the sealed executable.

hirsel_guard::install!();

use hirsel_driver::native::{
    OptLevel, Options, Partition, RuntimeBuild, TargetSpec, Toolchain, build, emit_objects, link,
    link_standalone_objects,
};
use std::{path::Path, time::Instant};

const CONFIGURATIONS: [&str; 4] = [
    "regions-on-whole",
    "regions-on-singleton",
    "regions-off-size-class",
    "regions-off-system",
];

fn main() {
    hirsel_engine::with_compiler_stack(|| {
        if let Err(error) = compile() {
            eprintln!("{error}");
            std::process::exit(1);
        }
    });
}

fn compile() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() != 3 {
        return Err("usage: hirsel-native-build SOURCE OUTPUT CONFIGURATION|all".into());
    }
    let source_start = Instant::now();
    let mut loaded = hirsel_driver::load_program(Path::new(&args[0]))?;
    if let Err(error) = loaded.check_for_execution() {
        if let hirsel_engine::ToolError::Query {
            error: hirsel_engine::QueryError::Diagnostics(diagnostics),
            sources,
            ..
        } = &error
        {
            for diagnostic in diagnostics {
                eprint!("{}", diagnostic.render_human(sources));
            }
        }
        return Err(error.into());
    }
    let input = loaded
        .engine
        .export_native_input()
        .map_err(|e| format!("{e:?}"))?;
    let validated = loaded
        .engine
        .import_native_input(input)
        .map_err(|e| format!("{e:?}"))?;
    let source_seconds = source_start.elapsed().as_secs_f64();
    let all = args[2] == "all";
    let configs = if all {
        CONFIGURATIONS.to_vec()
    } else {
        vec![args[2].as_str()]
    };
    let target = TargetSpec::x86_64_unknown_linux_gnu();
    let mut records = serde_json::Map::new();
    for config in configs {
        let mut options = Options::new(target.clone());
        let partition = match config {
            "regions-on-whole" => Partition::Whole,
            "regions-on-singleton" => Partition::Singleton,
            "regions-off-size-class" | "regions-off-system" => {
                options.regions = false;
                Partition::Whole
            }
            "flat-walk-off" => {
                options.flat_walk = false;
                Partition::Whole
            }
            "reuse-off" => {
                options.reuse = false;
                Partition::Whole
            }
            _ => return Err("unsupported configuration".into()),
        };
        let build_start = Instant::now();
        let ready = match build(&validated, &options) {
            Ok(ready) => ready,
            Err(error) if all => {
                records.insert(
                    config.into(),
                    serde_json::json!({"error": error.to_string()}),
                );
                continue;
            }
            Err(error) => return Err(error.into()),
        };
        let native_seconds = build_start.elapsed().as_secs_f64();
        let mut builds = serde_json::Map::new();
        let variants = if all {
            vec![false, true]
        } else {
            vec![std::env::var_os("HIRSEL_BENCH_MEASUREMENT").is_some()]
        };
        for measurement in variants {
            let output = if all {
                Path::new(&args[1])
                    .join(config)
                    .join(Path::new(&args[0]).file_stem().unwrap())
                    .join(if measurement {
                        "program-measurement"
                    } else {
                        "program"
                    })
            } else {
                Path::new(&args[1]).to_path_buf()
            };
            let dir = output.parent().ok_or("output needs a directory")?;
            std::fs::create_dir_all(dir)?;
            let runtime = dir.join("runtime");
            let runtime_build = RuntimeBuild {
                diagnostic: false,
                memcheck: false,
                system_allocator: config == "regions-off-system",
                measurement,
                ownership_trace: false,
            };
            let runtime_start = Instant::now();
            let toolchain = Toolchain::in_tree_with_runtime(&runtime, &[&target], runtime_build)?;
            let runtime_seconds = runtime_start.elapsed().as_secs_f64();
            let link_start = Instant::now();
            let objects = dir.join(if measurement {
                "measurement-objects"
            } else {
                "objects"
            });
            std::fs::create_dir_all(&objects)?;
            let standalone = std::env::var_os("HIRSEL_BENCH_STANDALONE").is_some();
            let result = if standalone {
                emit_objects(&ready, OptLevel::Speed, partition, measurement, None).and_then(
                    |emitted| {
                        link_standalone_objects(
                            &ready,
                            &loaded.entry,
                            &emitted,
                            &toolchain,
                            &objects,
                        )
                    },
                )
            } else {
                link(
                    &ready,
                    &loaded.entry,
                    OptLevel::Speed,
                    partition,
                    &toolchain,
                    &objects,
                )
            };
            let executable = match result {
                Ok(executable) => executable,
                Err(error) if all => {
                    builds.insert(
                        if measurement { "measurement" } else { "timing" }.into(),
                        serde_json::json!({"error": error.to_string()}),
                    );
                    continue;
                }
                Err(error) => return Err(error.into()),
            };
            std::fs::copy(executable, &output)?;
            let record = serde_json::json!({
                "compile_seconds": source_seconds + native_seconds + link_start.elapsed().as_secs_f64(),
                "source_check_snapshot_seconds": source_seconds,
                "runtime_build_seconds": runtime_seconds,
                "compile_kind": "source check/snapshot shared across configurations; native passes, LLVM O3 and link; runtime separate; caches retained",
                "llvm_version": hirsel_driver::native::llvm_version(),
                "hirsel_opt_level": "O3", "hirsel_lto": false,
                "runtime_archive": toolchain.runtime.get(&(target.triple.clone(), runtime_build)),
                "runtime_build": { "diagnostic": false, "system_allocator": runtime_build.system_allocator, "measurement": measurement },
                "standalone": standalone,
                "matrix_version": hirsel_driver::native::MATRIX_VERSION,
                "configuration": config, "partition": partition.name(),
                "regions": options.regions, "reuse": options.reuse, "flat_walk": options.flat_walk,
            });
            if !all {
                println!("{record}");
                return Ok(());
            }
            builds.insert(
                if measurement { "measurement" } else { "timing" }.into(),
                record,
            );
        }
        records.insert(config.into(), serde_json::Value::Object(builds));
    }
    println!("{}", serde_json::Value::Object(records));
    Ok(())
}
