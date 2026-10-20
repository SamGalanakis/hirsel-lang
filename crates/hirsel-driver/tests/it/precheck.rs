//! Prechecked packages (docs/building.md, "The std precheck") change only how long checking
//! takes: prechecked products equal freshly checked ones, and a precheck is used only for
//! the exact compiler and sources it was checked under.
use crate::precheck_fixture::*;
use hirsel_driver::*;
use hirsel_engine::*;
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

impl Fixture {
    fn library(&self, source: &str) -> PathBuf {
        self.write("lib.hir", source);
        let manifest = serde_json::json!({
            "schema": 1,
            "package": {"name": "lib", "version": "1"},
            "sources": [{"path": "lib.hir", "module": ["lib"]}],
            "dependencies": [],
        });
        let path = self.0.join("lib.json");
        std::fs::write(&path, manifest.to_string()).unwrap();
        path
    }
}
/// `Anchor(n)` in a Debug rendering, replaced by that anchor's file path and base.
fn placed(sources: &hirsel_span::SourceMap, text: String) -> String {
    let mut out = String::new();
    let mut rest = text.as_str();
    while let Some(at) = rest.find("Anchor(") {
        out.push_str(&rest[..at]);
        rest = &rest[at + "Anchor(".len()..];
        let end = rest.find(')').unwrap();
        let anchor = hirsel_span::Anchor(rest[..end].parse().unwrap());
        let place = sources
            .anchor(anchor)
            .map(|(file, base)| (sources.file(file).unwrap().path.clone(), base));
        out.push_str(&format!("Anchor({place:?})"));
        rest = &rest[end + 1..];
    }
    out + rest
}

/// Every prechecked std definition lowers to the same TIR and has the same revision
/// node as in a fresh check, in a program that places std's sources at other
/// FileIds than the program the precheck was checked in.
#[test]
fn prechecked_std_products_equal_fresh_ones() {
    with_compiler_stack(|| {
        let f = Fixture::new();
        // Buck2 checks std once for all tests. Cargo users build the same artifact
        // with the documented `hirsel precheck` command and set HIRSEL_PRECHECK.
        let path = std::env::var_os("HIRSEL_PRECHECK")
            .expect("set HIRSEL_PRECHECK to the toolchain's std precheck");
        let precheck = Arc::new(Precheck::read(Path::new(&path)).unwrap());
        // `aaa` places std after the program's own sources, exercising FileId rebasing.
        let client = f.program("aaa", STD_PROGRAM, &std_manifest(), "std");
        let mut prechecked = load(&client, &Prechecks::of(vec![precheck.clone()]));
        assert!(
            prechecked.engine.prechecked(&package("std")),
            "{:?}",
            prechecked.precheck_notes
        );
        let mut reference = load(&client, &Prechecks::none());
        let checked = |p: &mut LoadedProgram| {
            let output = p.check(hirsel_engine::LoweringPolicy::Reachable).unwrap();
            let rendered: Vec<_> = output
                .diagnostics
                .iter()
                .map(|d| d.render_human(&output.sources))
                .collect();
            (output.completion, rendered)
        };
        assert_eq!(checked(&mut prechecked), checked(&mut reference));
        let entry = prechecked.entry.clone();
        assert_eq!(
            prechecked.engine.semantic_program_revision(&entry).unwrap(),
            reference.engine.semantic_program_revision(&entry).unwrap()
        );
        let mut lowered = 0;
        for path in precheck.definitions() {
            assert_eq!(
                prechecked.engine.semantic_program_revision(path).unwrap(),
                reference.engine.semantic_program_revision(path).unwrap(),
                "{path}"
            );
            let Ok(expected) = reference.engine.lower_to_tir(path) else {
                continue;
            };
            let actual = prechecked.engine.lower_to_tir(path).unwrap();
            assert_eq!(
                prechecked.engine.source_audit(path).unwrap(),
                reference.engine.source_audit(path).unwrap(),
                "{path}"
            );
            // Anchors are process-unique, so each is named by its placement;
            // TypeId's Debug omits provenance.
            assert_eq!(
                placed(&prechecked.engine.sources, format!("{:?}", actual.value)),
                placed(&reference.engine.sources, format!("{:?}", expected.value)),
                "{path}"
            );
            assert_eq!(actual.dependencies, expected.dependencies, "{path}");
            lowered += 1;
        }
        assert!(lowered > 100, "{lowered}");
        let actual = prechecked.engine.export_native_input().unwrap();
        let expected = reference.engine.export_native_input().unwrap();
        assert_eq!(actual.audits, expected.audits);
        assert_eq!(
            format!("{:?}", actual.materialized),
            format!("{:?}", expected.materialized)
        );
        assert_eq!(
            format!("{:?}", actual.constants),
            format!("{:?}", expected.constants)
        );
        prechecked.engine.import_native_input(actual).unwrap();
        for path in precheck.definitions() {
            for phase in [QueryKind::Check, QueryKind::Modes] {
                assert_eq!(
                    prechecked.engine.recompute_count(path, phase),
                    0,
                    "{path}: {phase:?}"
                );
            }
        }
    });
}

const LIBRARY: &str = "pub fn value(x: u64) -> u64 {
  x + 1
}
";
const CLIENT: &str = "pub fn main() -> Result[(), ()] {
  failure.check(lib.value(1) == 2, Arithmetic())
  Ok(())
}
";

fn library_precheck(f: &Fixture, source: &str) -> Result<Arc<Precheck>, ToolError> {
    let manifest = f.library(source);
    let program = f.program("prechecker", CLIENT, &manifest, "lib");
    let bytes = precheck(&mut load(&program, &Prechecks::none()), "lib")?;
    Ok(Arc::new(Precheck::decode(bytes).unwrap()))
}

/// `program`'s precheck of the dependency `name`, written to memory.
fn precheck(program: &mut LoadedProgram, name: &str) -> Result<Vec<u8>, ToolError> {
    let mut bytes = vec![];
    program.precheck(
        &package(name),
        &mut bytes,
        &mut std::io::Cursor::new(vec![]),
    )?;
    Ok(bytes)
}

#[test]
fn a_precheck_serves_only_the_compiler_and_sources_it_was_checked_under() {
    with_compiler_stack(|| {
        let f = Fixture::new();
        let precheck = library_precheck(&f, LIBRARY).unwrap();
        let client = f.program("client", CLIENT, &f.0.join("lib.json"), "lib");
        let cache = Prechecks::of(vec![precheck.clone()]);
        let mut program = load(&client, &cache);
        assert!(program.engine.prechecked(&package("lib")));
        assert!(program.precheck_notes.is_empty());
        program.check_for_execution().unwrap();

        // Another compiler identity.
        let other = Compiler::new(FrontPhases::shared, [7; 32]);
        let program = other
            .load_program_source_with_prechecks(&client, None, &cache)
            .unwrap();
        assert!(!program.engine.prechecked(&package("lib")));
        assert_eq!(
            program.precheck_notes.len(),
            1,
            "{:?}",
            program.precheck_notes
        );

        // Other sources: a comment changes the package's checked input.
        f.library(&format!("{LIBRARY}// edited\n"));
        let mut program = load(&client, &cache);
        assert!(!program.engine.prechecked(&package("lib")));
        assert_eq!(
            program.precheck_notes.len(),
            1,
            "{:?}",
            program.precheck_notes
        );
        program.check_for_execution().unwrap();
    });
}

#[test]
fn editing_a_prechecked_source_in_a_session_ends_its_precheck() {
    with_compiler_stack(|| {
        let f = Fixture::new();
        let precheck = library_precheck(&f, LIBRARY).unwrap();
        let client = f.program("client", CLIENT, &f.0.join("lib.json"), "lib");
        let mut program = load(&client, &Prechecks::of(vec![precheck]));
        assert!(program.engine.prechecked(&package("lib")));
        let (file, context) = program
            .files
            .iter()
            .find(|(_, c)| c.package == package("lib"))
            .cloned()
            .unwrap();
        let path = program.engine.sources.file(file).unwrap().path.clone();
        let edited = LIBRARY.replace("x + 1", "x + 2");
        let file = program.engine.sources.add(&path, edited).unwrap();
        program.engine.parse_file(file, &context).unwrap();
        assert!(!program.engine.prechecked(&package("lib")));
        // The edited body is checked and run from source.
        assert!(program.check_for_execution().is_ok());
    });
}

/// The cache must retain the source evidence and compile-time substitutions, not
/// just a completion bit. Fresh import is the accepted control for both forgeries.
#[test]
fn prechecked_native_products_equal_fresh_and_reject_forged_audits() {
    with_compiler_stack(|| {
        let f = Fixture::new();
        let source = "const BASE: u64 = comptime 1\n@noalloc\npub fn value(x: u64) -> u64 {\n  x + comptime BASE\n}\n";
        let precheck = library_precheck(&f, source).unwrap();
        let client = f.program("client", CLIENT, &f.0.join("lib.json"), "lib");
        let mut cached = load(&client, &Prechecks::of(vec![precheck.clone()]));
        let mut fresh = load(&client, &Prechecks::none());
        cached.check_for_execution().unwrap();
        fresh.check_for_execution().unwrap();
        let actual = cached.engine.export_native_input().unwrap();
        let expected = fresh.engine.export_native_input().unwrap();
        assert_eq!(actual.audits, expected.audits);
        assert_eq!(
            format!("{:?}", actual.materialized),
            format!("{:?}", expected.materialized)
        );
        assert_eq!(
            format!("{:?}", actual.constants),
            format!("{:?}", expected.constants)
        );
        for path in precheck.definitions() {
            for phase in [QueryKind::Check, QueryKind::Modes] {
                assert_eq!(
                    cached.engine.recompute_count(path, phase),
                    0,
                    "{path}: {phase:?}"
                );
                assert!(
                    fresh.engine.recompute_count(path, phase) > 0,
                    "{path}: {phase:?}"
                );
            }
        }
        let path = actual
            .audits
            .keys()
            .find(|path| path.0.package == package("lib") && path.0.name.as_str() == "value")
            .unwrap()
            .clone();
        assert!(actual.audits[&path].noalloc);
        cached.engine.import_native_input(actual.clone()).unwrap();
        let mut forged = actual.clone();
        forged.audits.get_mut(&path).unwrap().noalloc = false;
        assert!(cached.engine.import_native_input(forged).is_err());
        let mut missing = actual;
        missing.audits.remove(&path);
        assert!(cached.engine.import_native_input(missing).is_err());

        let (file, context) = cached
            .files
            .iter()
            .find(|(_, context)| context.package == package("lib"))
            .cloned()
            .unwrap();
        let path = cached.engine.sources.file(file).unwrap().path.clone();
        let file = cached
            .engine
            .sources
            .add(&path, source.replace("comptime 1", "comptime 2"))
            .unwrap();
        cached.engine.parse_file(file, &context).unwrap();
        assert!(!cached.engine.prechecked(&package("lib")));
        cached.check_for_execution().unwrap();
        let edited = cached.engine.export_native_input().unwrap();
        assert_ne!(
            format!("{:?}", edited.constants),
            format!("{:?}", expected.constants)
        );
        assert_ne!(
            format!("{:?}", edited.materialized),
            format!("{:?}", expected.materialized)
        );
    });
}

/// A precheck is written only for a package that checks clean; its accepted twin is
/// `a_precheck_serves_only_the_compiler_and_sources_it_was_checked_under`.
#[test]
fn a_package_with_diagnostics_is_never_prechecked() {
    with_compiler_stack(|| {
        let f = Fixture::new();
        let rejected = library_precheck(&f, "pub fn value(x: u64) -> u64 {\n  x + true\n}\n");
        assert!(rejected.is_err());
    });
}

#[test]
fn prechecked_generic_comptime_materializes_selected_client_sizes() {
    with_compiler_stack(|| {
        let f = Fixture::new();
        let source = "pub fn value[n:size](xs:Inline[u8,n])->u64{let _=xs;comptime (n*n)}\n";
        let client = "pub fn main()->Result[(),()]{let a:Inline[u8,2]=[1,2];let b:Inline[u8,3]=[1,2,3];failure.check(lib.value(a)==4u64,Arithmetic());failure.check(lib.value(b)==9u64,Arithmetic());Ok(())}\n";
        let library = f.library(source);
        let prechecker = f.program("prechecker", client, &library, "lib");
        let bytes = precheck(&mut load(&prechecker, &Prechecks::none()), "lib").unwrap();
        let precheck = Arc::new(Precheck::decode(bytes).unwrap());
        let client = f.program("client", client, &library, "lib");
        let mut cached = load(&client, &Prechecks::of(vec![precheck]));
        let mut fresh = load(&client, &Prechecks::none());
        assert!(cached.engine.prechecked(&package("lib")));
        cached.check_for_execution().unwrap();
        fresh.check_for_execution().unwrap();
        let actual = cached.engine.export_native_input().unwrap();
        let expected = fresh.engine.export_native_input().unwrap();
        let selected = |input: &NativeInput| {
            input
                .constants
                .iter()
                .filter(|(path, _)| path.0.name.starts_with("$constant:$comptime:value:"))
                .map(|(path, value)| (path.clone(), value.root.kind.clone()))
                .collect::<std::collections::BTreeMap<_, _>>()
        };
        let values = selected(&actual);
        assert_eq!(values, selected(&expected));
        assert_eq!(
            values.values().cloned().collect::<Vec<_>>(),
            vec![
                hirsel_tir::ValueKind::Constant(hirsel_tir::Constant::Integer(4)),
                hirsel_tir::ValueKind::Constant(hirsel_tir::Constant::Integer(9)),
            ]
        );
        cached.engine.import_native_input(actual).unwrap();
    });
}
