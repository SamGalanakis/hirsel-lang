//! Stage an explicit package check to separate phase costs from artifact reuse.
use hirsel_driver::*;
use hirsel_engine::*;
use hirsel_syntax::ItemKind;
use std::{path::Path, time::Instant};

fn timed<T>(name: &str, work: impl FnOnce() -> T) -> T {
    let start = Instant::now();
    let result = work();
    eprintln!("{name}: {:.6} s", start.elapsed().as_secs_f64());
    result
}

fn main() {
    let mut paths = std::env::args().skip(1).collect::<Vec<_>>();
    let phases = paths.first().is_some_and(|p| p == "--phases");
    if phases {
        paths.remove(0);
    }
    assert!(!paths.is_empty(), "launch manifest or source");
    if !phases {
        with_compiler_stack(|| {
            for path in paths {
                eprintln!("{path}");
                let cache = PackageCache::new();
                for name in ["cold", "warm"] {
                    timed(name, || {
                        let mut program =
                            load_program_source_with_cache(Path::new(&path), None, &cache).unwrap();
                        let output = program.check().unwrap();
                        assert_eq!(output.completion, CheckCompletion::Complete);
                        assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
                    });
                }
            }
        });
        return;
    }
    let path = &paths[0];
    with_compiler_stack(|| {
        let cache = PackageCache::new();
        let mut program = timed("load, lex, parse and index", || {
            load_program_source_with_cache(Path::new(&path), None, &cache).unwrap()
        });
        let mut roots = vec![];
        for (file, context) in program.files.clone() {
            let parsed = program.engine.parse_file(file, &context).unwrap();
            roots.extend(parsed.value.roots.iter().map(|id| {
                let item = &parsed.value.tree.items[id.0 as usize];
                let executable = matches!(
                    item.kind,
                    ItemKind::Function { body: Some(_), .. }
                        | ItemKind::Const { .. }
                        | ItemKind::Type { drop: Some(_), .. }
                );
                (item.definition.clone(), executable)
            }));
        }
        eprintln!(
            "{} declarations, {} executable roots",
            roots.len(),
            roots.iter().filter(|(_, e)| *e).count()
        );
        let lowered = |program: &LoadedProgram| {
            roots
                .iter()
                .filter(|(p, _)| program.engine.recompute_count(p, QueryKind::Lower) > 0)
                .count()
        };
        timed("signatures (including header resolution)", || {
            for (path, _) in &roots {
                program.engine.item_signature(path).unwrap();
            }
        });
        timed("body resolution", || {
            for (path, _) in &roots {
                program.engine.resolve_definition(path).unwrap();
            }
        });
        timed("bodies", || {
            for (path, _) in &roots {
                program.engine.check_definition(path).unwrap();
            }
        });
        timed("modes", || {
            for (path, _) in &roots {
                program.engine.mode_definition(path).unwrap();
            }
        });
        // Dependencies lower at check time only for compile-time evaluation
        // or when the program reaches them; that remainder runs in the check.
        let package = program.entry.0.package.clone();
        timed(
            "program package lowering and independent TIR validation",
            || {
                for (path, executable) in &roots {
                    if *executable && path.0.package == package {
                        program.engine.lower_to_tir(path).unwrap();
                    }
                }
            },
        );
        timed(
            "reachable dependency lowering, program audits and artifact publication",
            || {
                let output = program.check().unwrap();
                assert_eq!(output.completion, CheckCompletion::Complete);
                assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
            },
        );
        eprintln!("{} executable roots lowered", lowered(&program));
        drop(program);
        let mut warm = timed("reused load and transport", || {
            load_program_source_with_cache(Path::new(&path), None, &cache).unwrap()
        });
        timed("reused check and program audits", || {
            let output = warm.check().unwrap();
            assert_eq!(output.completion, CheckCompletion::Complete);
            assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
        });
        for kind in [
            QueryKind::Signature,
            QueryKind::Resolve,
            QueryKind::Check,
            QueryKind::Modes,
            QueryKind::Lower,
        ] {
            let std_recomputes: usize = roots
                .iter()
                .filter(|(p, _)| p.0.package.name_is("std"))
                .map(|(p, _)| warm.engine.recompute_count(p, kind))
                .sum();
            eprintln!("reused std {kind:?} computations: {std_recomputes}");
        }
    });
}
