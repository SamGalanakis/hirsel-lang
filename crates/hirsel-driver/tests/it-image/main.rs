//! The std image against a fresh check of std (docs/building.md, "The std
//! precheck"): the one test that needs the toolchain's precheck
//! (`HIRSEL_PRODUCTS`). It checks std from source once, so it links as its own
//! binary with its own measured request.

hirsel_guard::install!();

#[path = "../support/image_fixture.rs"]
mod image_fixture;

use hirsel_driver::*;
use hirsel_engine::*;
use image_fixture::*;

const STD_PROGRAM: &str = "pub type AppError = Grant(GrantError) | Output(OutputError)

pub fn main(rt: Io) -> Result[(), AppError] {
  let output = io.stdout(rt) ? Grant
  out.print(output, string.trim(\" hello \")) ? Output
  out.print(output, u64.to_string(42u64)) ? Output
  Ok(())
}
";

#[path = "../support/canonical_debug.rs"]
mod canonical_debug;
use canonical_debug::{canonical_ids, placed};

/// Every prechecked std definition hashes the same (LANG-6), lowers to the same TIR
/// and has the same revision node as in a fresh check, in a program that places
/// std's sources at other FileIds than the program the precheck was checked in.
#[test]
fn ordinary_std_complete_products_equal_fresh_ones() {
    with_compiler_stack(|| {
        let f = Fixture::new();
        // Buck2 checks std once for all tests. Cargo users build the same artifact
        // with the documented `hirsel image` command and set HIRSEL_PRODUCTS.
        let products = std_products();
        let client = f.program("aaa", STD_PROGRAM, &std_manifest(), "std");
        let mut prechecked = load(&client, Some(&products));
        let definitions = prechecked
            .engine
            .indexed_definitions()
            .into_iter()
            .filter(|p| p.0.package == package("std"))
            .collect::<Vec<_>>();
        let mut reference = load(&client, None);
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
        for path in &definitions {
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
            // Preserve every Debug field, naming anchors by source placement and
            // session-local TypeIds by the canonical content of their type rows.
            let render = |program: &LoadedProgram, value: &hirsel_tir::Definition| {
                let text = canonical_ids(
                    program.engine.types(),
                    || serde_json::to_vec(value).unwrap(),
                    format!("{value:?}"),
                );
                placed(&program.engine.sources, text)
            };
            assert_eq!(
                render(&prechecked, &actual.value),
                render(&reference, &expected.value),
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
            canonical_ids(
                &actual.types,
                || serde_json::to_vec(&actual.materialized.values().collect::<Vec<_>>()).unwrap(),
                format!("{:?}", actual.materialized)
            ),
            canonical_ids(
                &expected.types,
                || serde_json::to_vec(&expected.materialized.values().collect::<Vec<_>>()).unwrap(),
                format!("{:?}", expected.materialized)
            )
        );
        assert_eq!(
            canonical_ids(
                &actual.types,
                || serde_json::to_vec(&actual.constants.values().collect::<Vec<_>>()).unwrap(),
                format!("{:?}", actual.constants)
            ),
            canonical_ids(
                &expected.types,
                || serde_json::to_vec(&expected.constants.values().collect::<Vec<_>>()).unwrap(),
                format!("{:?}", expected.constants)
            )
        );
        prechecked.engine.import_native_input(actual).unwrap();
        for path in &definitions {
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
