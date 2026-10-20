//! The std precheck against a fresh check of std (docs/building.md, "The std
//! precheck"): the one test that needs the toolchain's precheck
//! (`HIRSEL_PRECHECK`). It checks std from source once, so it links as its own
//! binary with its own measured request.

hirsel_guard::install!();

#[path = "../support/precheck_fixture.rs"]
mod precheck_fixture;

use hirsel_driver::*;
use hirsel_engine::*;
use precheck_fixture::*;
use std::{path::Path, sync::Arc};

const STD_PROGRAM: &str = "pub type AppError = Grant(GrantError) | Output(OutputError)

pub fn main(rt: Io) -> Result[(), AppError] {
  let output = io.stdout(rt) ? Grant
  out.print(output, string.trim(\" hello \")) ? Output
  out.print(output, u64.to_string(42u64)) ? Output
  Ok(())
}
";

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

/// Every prechecked std definition hashes the same (LANG-6), lowers to the same TIR
/// and has the same revision node as in a fresh check, in a program that places
/// std's sources at other FileIds than the program the precheck was checked in.
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
        let mut prechecked = load(&client, &PackageCache::prechecked(vec![precheck.clone()]));
        assert!(
            prechecked.engine.prechecked(&package("std")),
            "{:?}",
            prechecked.precheck_notes
        );
        let mut reference = load(&client, &PackageCache::disabled());
        assert!(!reference.engine.prechecked(&package("std")));
        // LANG-6: every std definition and the program's closure hash the same
        // whether std is adopted from its precheck (definition_content's
        // prechecked-node branch) or checked from source.
        let entry = prechecked.entry.clone();
        let hashes = |p: &mut LoadedProgram| {
            let mut all = p.engine.package_definition_hashes(&package("std")).unwrap();
            all.hashes
                .extend(p.engine.definition_hashes([entry.clone()]).unwrap().hashes);
            all.hashes
        };
        let (actual, expected) = (hashes(&mut prechecked), hashes(&mut reference));
        assert!(expected.len() > 1000, "{}", expected.len());
        let differing: Vec<_> = expected
            .iter()
            .filter(|(path, hash)| actual.get(*path) != Some(*hash))
            .map(|(path, _)| path.to_string())
            .collect();
        assert!(differing.is_empty(), "{differing:?}");
        assert_eq!(actual.len(), expected.len());
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
