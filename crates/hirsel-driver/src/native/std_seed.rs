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
/// backend/optimization partition. No program is linked or run. Generic instances
/// reached from those roots are included and counted separately.
pub fn seed_objects(manifest: &Path, output: &Path) -> Result<SeedReport, String> {
    let mut package = crate::COMPILER
        .load_package_declarations(manifest)
        .map_err(|e| e.to_string())?;
    let mut roots = Vec::new();
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
                roots.push(item.item.definition.clone());
            }
        }
    }
    roots.sort();
    roots.dedup();
    if roots.is_empty() {
        return Err("no non-generic runtime functions to seed".into());
    }
    let input = package
        .engine
        .native_input_for(&roots)
        .map_err(|e| format!("{e:?}"))?;
    let target = if cfg!(target_arch = "aarch64") {
        TargetSpec::aarch64_unknown_linux_gnu()
    } else {
        TargetSpec::x86_64_unknown_linux_gnu()
    };
    let program = super::adapt::program(&input);
    let manifest = super::adapt::manifest(input.snapshot()).map_err(|e| e.to_string())?;
    let mut report = SeedReport {
        roots: roots.len(),
        ..Default::default()
    };
    // Seed with one worker: toolchain building must not multiply the memory of
    // unrelated LLVM modules. Consumers choose their own worker count.
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
            let ready = super::stages::build_program(&program, manifest.clone(), &options)
                .map_err(|e| e.to_string())?;
            report.units = ready.summaries.len();
            report.generic_units = ready
                .summaries
                .iter()
                .filter(|u| {
                    input
                        .snapshot()
                        .signatures
                        .get(&u.definition)
                        .is_some_and(|s| match &s.kind {
                            hirsel_types::SignatureKind::Function(f) => !f.generics.is_empty(),
                            _ => false,
                        })
                })
                .count();
            report.objects += ready.unit_objects.as_ref().map_or(0, |o| o.objects.len());
        }
    }
    Ok(report)
}
