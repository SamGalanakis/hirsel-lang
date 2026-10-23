use crate::image_fixture::*;
use hirsel_driver::*;
use hirsel_engine::*;
use hirsel_syntax::DefId;
use std::{path::PathBuf, sync::Arc};
#[path = "../support/canonical_debug.rs"]
mod canonical_debug;
use canonical_debug::{canonical_ids, placed};
const STD_PROGRAM: &str = "pub type AppError = Grant(GrantError) | Output(OutputError)

pub fn main(rt: Io) -> Result[(), AppError] {
  let output = io.stdout(rt) ? Grant
  out.print(output, string.trim(\" hello \")) ? Output
  out.print(output, u64.to_string(42u64)) ? Output
  Ok(())
}
";

// Per-definition-only quarters omit the separately proven client check prefix.
// Every comparison preserves the full semantic, Lower, audit and dependency assertions.
fn ordinary_std_definition_products_quarter(partition: usize) {
    compare_products(partition, None);
}

fn compare_products(partition: usize, selected: Option<Vec<String>>) {
    with_compiler_stack(|| {
        let f = Fixture::new();
        let products = f.0.join("std-products");
        let compiler = COMPILER.clone().with_config(CompileConfig {
            jobs: hirsel_intern::jobs::Jobs::new(1),
            ..CompileConfig::for_memory_cap(None)
        });
        // Packaging computes this ordinary product seed once as a build artifact.
        // Each reader owns fresh verifier associations; installed bytes confer no V authority.
        let installed = std::env::var_os("HIRSEL_TEST_PRODUCTS")
            .map(PathBuf::from)
            .expect("the test harness supplies ordinary precomputed products");
        let shared = compiler.clone().with_config(CompileConfig {
            store: Arc::new(store::DirStore::new(products).with_read_only_root(installed)),
            jobs: hirsel_intern::jobs::Jobs::new(1),
            ..CompileConfig::for_memory_cap(None)
        });
        // `aaa` places std after the program's own sources, exercising FileId rebasing.
        let client = f.program("aaa", STD_PROGRAM, &std_manifest(), "std");
        let mut imaged = shared
            .load_program_source_with_images(&client, None, &StdImages::none())
            .unwrap();
        let definitions: Vec<_> = imaged
            .engine
            .indexed_definitions()
            .into_iter()
            .filter(|path| path.0.package == package("std"))
            .map(|path| DefId::intern(&path))
            .collect();
        assert!(definitions.len() > 100);
        let mut reference = compiler
            .load_program_source_with_images(&client, None, &StdImages::none())
            .unwrap();
        let inventory: Vec<_> = definitions.iter().map(|id| id.path().to_string()).collect();
        assert!(inventory.windows(2).all(|pair| pair[0] < pair[1]));
        eprintln!("[ca41-coverage] quarter={partition} complete-inventory={inventory:?}");
        let phase = std::time::Instant::now();
        let mut lowered = 0;
        let mut compared = vec![];
        for (index, path) in definitions.iter().copied().map(DefId::path).enumerate() {
            if selected
                .as_ref()
                .map_or(index % 4 != partition, |selected| {
                    !selected.contains(&path.to_string())
                })
            {
                continue;
            }
            compared.push(path.to_string());
            assert_eq!(
                imaged.engine.semantic_program_revision(path).unwrap(),
                reference.engine.semantic_program_revision(path).unwrap(),
                "{path}"
            );
            let Ok(expected) = reference.engine.lower_to_tir(path) else {
                continue;
            };
            let actual = imaged.engine.lower_to_tir(path).unwrap();
            assert_eq!(
                imaged.engine.source_audit(path).unwrap(),
                reference.engine.source_audit(path).unwrap(),
                "{path}"
            );
            // Anchors are process-unique, so each is named by its placement;
            // TypeId's Debug omits provenance.
            // Each session interns its own rows: compare them by content.
            let (types, value) = (imaged.engine.types().clone(), &actual.value);
            let actual_text = canonical_ids(
                &types,
                || serde_json::to_vec(value.as_ref()).unwrap(),
                format!("{value:?}"),
            );
            let (types, value) = (reference.engine.types().clone(), &expected.value);
            let expected_text = canonical_ids(
                &types,
                || serde_json::to_vec(value.as_ref()).unwrap(),
                format!("{value:?}"),
            );
            assert_eq!(
                placed(&imaged.engine.sources, actual_text),
                placed(&reference.engine.sources, expected_text),
                "{path}"
            );
            assert_eq!(actual.dependencies, expected.dependencies, "{path}");
            lowered += 1;
        }
        eprintln!(
            "[ca41-coverage] quarter={partition} compared={compared:?} lowered={lowered} elapsed={:?}",
            phase.elapsed()
        );
        if let Some(selected) = &selected {
            assert_eq!(&compared, selected, "every selected identity must execute");
            assert!(lowered > 0, "selected comparison must lower a body");
        } else {
            assert!(lowered > 25, "quarter={partition} lowered={lowered}");
        }
        for path in definitions.iter().copied().map(DefId::path) {
            for phase in [QueryKind::Check, QueryKind::Modes] {
                assert_eq!(
                    imaged.engine.recompute_count(path, phase),
                    0,
                    "{path}: {phase:?}"
                );
            }
        }
    });
}

#[test]
fn ordinary_std_definition_products_quarter_0() {
    ordinary_std_definition_products_quarter(0);
}
#[test]
fn ordinary_std_definition_products_quarter_1() {
    ordinary_std_definition_products_quarter(1);
}
#[test]
fn ordinary_std_definition_products_quarter_2() {
    ordinary_std_definition_products_quarter(2);
}
#[test]
fn ordinary_std_definition_products_quarter_3() {
    ordinary_std_definition_products_quarter(3);
}
#[test]
fn ordinary_std_definition_products_selected() {
    let selected = std::env::var("HIRSEL_TEST_PRODUCT_DEFINITIONS")
        .ok()
        .filter(|selection| !selection.is_empty())
        .unwrap_or_else(|| {
            [
                "MOD",
                "NMAX",
                "State",
                "digest",
                "digest_hex",
                "finish",
                "new",
                "update",
                "update_list",
            ]
            .map(|name| format!("std@1/adler32/{name}"))
            .join(",")
        })
        .split(',')
        .map(str::to_owned)
        .collect::<Vec<_>>();
    assert!(!selected.is_empty());
    compare_products(4, Some(selected));
}
