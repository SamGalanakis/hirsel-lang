//! Toolchain-native std objects. Like Go's installed build products, the seed
//! is an optional read-only input; the ordinary action keys decide reuse.
use super::{Backend, OptLevel, Options, TargetSpec, UnitObjects, Validation};
use hirsel_engine::DefinitionQueries;
use hirsel_lir::objects::{DirObjects, ObjectStore, ReadOnlyObjects, SeededObjects};
use hirsel_syntax::ItemKind;
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[derive(Clone, Debug, Default)]
pub struct SeedReport {
    pub roots: usize,
    pub units: usize,
    pub generic_units: usize,
    pub objects: usize,
}

/// Layer the configured writable store over the optional read-only seed. A host
/// can disable the seed with HIRSEL_STD_OBJECTS=none or select another directory.
pub(crate) fn with_seed(writable: Arc<dyn ObjectStore>, partition: &str) -> Arc<dyn ObjectStore> {
    let root = match std::env::var_os("HIRSEL_STD_OBJECTS") {
        Some(value) if value == "none" => return writable,
        Some(value) => PathBuf::from(value),
        None => {
            let Some(root) = std::env::current_exe()
                .ok()
                .and_then(|p| p.parent().map(|p| p.join("hirsel-std.objects")))
            else {
                return writable;
            };
            root
        }
    };
    let partition = root.join(partition);
    if !partition.is_dir() {
        return writable;
    }
    Arc::new(SeededObjects {
        writable,
        seed: Arc::new(ReadOnlyObjects(partition)),
    })
}

/// Compile the package's non-generic runtime closure for the host into every
/// backend/optimization partition. Modules seed sequentially in fresh engines,
/// bounding memory by a module's reachable closure. Units and generic_units count
/// visits across module batches; objects counts freshly emitted owners. No program
/// is linked or run.
pub fn seed_objects(manifest: &Path, output: &Path) -> Result<SeedReport, String> {
    let mut package = crate::COMPILER
        .load_package_declarations(manifest)
        .map_err(|e| e.to_string())?;
    let mut roots = std::collections::BTreeMap::<_, Vec<_>>::new();
    for (file, context) in package.files.clone() {
        if context.package != package.package {
            continue;
        }
        let syntax = package
            .engine
            .parse_file(file, &context)
            .map_err(|e| format!("{e:?}"))?
            .value;
        for item in &syntax.items {
            if let ItemKind::Function {
                signature,
                body: Some(_),
                ..
            } = &item.item.kind
                && signature.generics.is_empty()
            {
                roots
                    .entry(context.modules.clone())
                    .or_default()
                    .push(item.item.definition.clone());
            }
        }
    }
    for group in roots.values_mut() {
        group.sort();
        group.dedup();
    }
    if roots.is_empty() {
        return Err("no non-generic runtime functions to seed".into());
    }
    // Declaration-only engines always free their tables. No module batch keeps
    // TIR, native layouts or placement state from an earlier batch alive.
    drop(package);
    let target = if cfg!(target_arch = "aarch64") {
        TargetSpec::aarch64_unknown_linux_gnu()
    } else {
        TargetSpec::x86_64_unknown_linux_gnu()
    };
    let mut report = SeedReport {
        roots: roots.values().map(Vec::len).sum(),
        ..Default::default()
    };
    // Seed with one worker: toolchain building must not multiply the memory of
    // unrelated LLVM modules. Consumers choose their own worker count.
    for (batch, (module, group)) in roots.iter().enumerate() {
        eprintln!(
            "seeding native objects {}/{}: {} ({} roots)",
            batch + 1,
            roots.len(),
            module.join("."),
            group.len(),
        );
        let mut package = crate::COMPILER
            .load_package_declarations(manifest)
            .map_err(|e| e.to_string())?;
        let input = package
            .engine
            .native_input_for(group)
            .map_err(|e| format!("{e:?}"))?;
        let program = super::adapt::program(&input);
        let operations = super::adapt::manifest(input.snapshot()).map_err(|e| e.to_string())?;
        for backend in [Backend::Cranelift, Backend::Llvm] {
            for opt in [OptLevel::None, OptLevel::Default, OptLevel::Speed] {
                let partition = hirsel_lir::objects::partition(
                    &target.triple,
                    backend.name(),
                    &format!("{opt:?}").to_lowercase(),
                );
                let store = Arc::new(DirObjects::new(output.join(partition)));
                let mut options = Options::new(target.clone());
                options.validation = Validation::Ready;
                options.objects = Some(UnitObjects {
                    store,
                    backend,
                    opt,
                    debug: opt == OptLevel::None,
                    fuel: false,
                    jobs: hirsel_intern::jobs::Jobs::ONE,
                });
                let ready = super::stages::build_program(&program, operations.clone(), &options)
                    .map_err(|e| e.to_string())?;
                if backend == Backend::Cranelift && opt == OptLevel::None {
                    report.units += ready.summaries.len();
                    report.generic_units +=
                        ready.unit_objects.as_ref().map_or(0, |o| o.generic_units);
                }
                report.objects += ready
                    .unit_objects
                    .as_ref()
                    .map_or(0, |o| o.objects.len() - o.hits);
            }
        }
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use hirsel_lir::objects::NoObjects;

    // A contract helper's closure is addressed and called in the same singleton
    // object. An untyped LLVM address declaration conflicts with the bound call.
    #[test]
    fn a_seeded_store_delete_has_one_consistent_entry_abi() {
        hirsel_engine::with_compiler_stack(|| {
            let manifest =
                Path::new(env!("CARGO_MANIFEST_DIR")).join("../../stdlib/std@1/hirsel.json");
            let mut package = crate::COMPILER
                .load_package_declarations(&manifest)
                .unwrap();
            let root = hirsel_syntax::DefPath(hirsel_syntax::ItemPath {
                package: package.package.clone(),
                modules: vec!["store".into()],
                name: "tx_delete".into(),
            });
            let input = package.engine.native_input_for(&[root]).unwrap();
            for backend in [Backend::Cranelift, Backend::Llvm] {
                for opt in [OptLevel::None, OptLevel::Default, OptLevel::Speed] {
                    let mut options = Options::new(TargetSpec::x86_64_unknown_linux_gnu());
                    options.objects = Some(UnitObjects {
                        store: Arc::new(NoObjects),
                        backend,
                        opt,
                        debug: opt == OptLevel::None,
                        fuel: false,
                        jobs: hirsel_intern::jobs::Jobs::ONE,
                    });
                    super::super::build(&input, &options)
                        .unwrap_or_else(|e| panic!("{backend:?} {opt:?}: {e}"));
                }
            }
        });
    }

    /// Installed products must be reusable by a distinct consumer on its first build,
    /// without changing process environment or sharing a compiler session.
    #[test]
    fn a_read_only_seed_hits_on_a_new_programs_first_build() {
        hirsel_engine::with_compiler_stack(|| {
            let directory =
                std::env::temp_dir().join(format!("hirsel-native-seed-{}", std::process::id()));
            std::fs::create_dir_all(&directory).unwrap();
            std::fs::write(
                directory.join("library.hir"),
                "pub fn answer(x:u64)->u64 { x+1u64 }\n",
            )
            .unwrap();
            let library = directory.join("library.json");
            std::fs::write(
                &library,
                serde_json::json!({
                    "schema":1,"package":{"name":"seeded","version":"1"},
                    "sources":[{"path":"library.hir","module":[]}],"dependencies":[]
                })
                .to_string(),
            )
            .unwrap();
            let output = directory.join("objects");
            let report = seed_objects(&library, &output).unwrap();
            assert_eq!(report.roots, 1);
            assert!(report.objects >= 6, "{report:?}");
            std::fs::write(directory.join("main.hir"), "pub fn main()->Result[(),()] { failure.check(lib.answer(3u64)==4u64, Arithmetic()); Ok(()) }\n").unwrap();
            let manifest = directory.join("application.json");
            std::fs::write(&manifest, serde_json::json!({
                "schema":1,"package":{"name":"consumer","version":"1"},
                "sources":[{"path":"main.hir","module":[]}],
                "dependencies":[{"manifest":"library.json","package":{"name":"seeded","version":"1"},"expose":[{"head":"lib","module":[]}]}],
                "entry":{"module":[],"name":"main"}
            }).to_string()).unwrap();
            let mut loaded = crate::load_program(&manifest).unwrap();
            loaded.check_for_execution().unwrap();
            let input = loaded
                .engine
                .native_input_for(&[loaded.entry.clone()])
                .unwrap();
            let target = if cfg!(target_arch = "aarch64") {
                TargetSpec::aarch64_unknown_linux_gnu()
            } else {
                TargetSpec::x86_64_unknown_linux_gnu()
            };
            for backend in [Backend::Cranelift, Backend::Llvm] {
                for opt in [OptLevel::None, OptLevel::Default, OptLevel::Speed] {
                    let partition = hirsel_lir::objects::partition(
                        &target.triple,
                        backend.name(),
                        &format!("{opt:?}").to_lowercase(),
                    );
                    let mut options = Options::new(target.clone());
                    options.validation = Validation::Ready;
                    options.objects = Some(UnitObjects {
                        store: Arc::new(SeededObjects {
                            writable: Arc::new(NoObjects),
                            seed: Arc::new(ReadOnlyObjects(output.join(partition))),
                        }),
                        backend,
                        opt,
                        debug: opt == OptLevel::None,
                        fuel: false,
                        jobs: hirsel_intern::jobs::Jobs::new(2),
                    });
                    let seeded = super::super::build(&input, &options).unwrap();
                    let seeded_objects = seeded.unit_objects.unwrap();
                    assert!(
                        seeded_objects.hits > 0,
                        "{backend:?} {opt:?}: no installed owner hit"
                    );
                    options.objects.as_mut().unwrap().store = Arc::new(NoObjects);
                    let fresh = super::super::build(&input, &options)
                        .unwrap()
                        .unit_objects
                        .unwrap();
                    assert_eq!(seeded_objects.objects, fresh.objects, "{backend:?} {opt:?}");
                }
            }
            std::fs::remove_dir_all(directory).unwrap();
        });
    }
}
