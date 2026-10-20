//! Reference vectors too large for the corpus's one-million-step budget.
use hirsel_driver::*;
use hirsel_engine::*;

fn run_expanded(case_name: &str, replacements: &[(&str, &str)]) {
    with_compiler_stack(|| {
        let dir =
            std::env::temp_dir().join(format!("hirsel-vectors-{case_name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let case = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join(format!("../../conformance/SEM/{case_name}.hir"));
        let mut source = std::fs::read_to_string(&case).unwrap();
        for (from, to) in replacements {
            assert!(source.contains(from), "fixture no longer contains {from:?}");
            source = source.replace(from, to);
        }
        let path = dir.join("main.hir");
        std::fs::write(&path, source).unwrap();
        let mut manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(case.with_extension("hir.json")).unwrap())
                .unwrap();
        manifest["sources"][0]["path"] = "main.hir".into();
        manifest["dependencies"][0]["manifest"] =
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../stdlib/std@1/hirsel.json")
                .to_string_lossy()
                .to_string()
                .into();
        std::fs::write(
            path.with_extension("hir.json"),
            serde_json::to_vec(&manifest).unwrap(),
        )
        .unwrap();
        let outcome = run_path_with_step_budget(
            &path,
            SimulationConfig::default(),
            None,
            hirsel_engine::Verify::On,
            &mut vec![],
            &mut vec![],
        )
        .unwrap();
        assert_eq!(
            outcome,
            RunOutcome::Returned(hirsel_run_config::Returned::Ok)
        );
        std::fs::remove_dir_all(dir).unwrap();
    });
}

#[test]
fn rust_btree_exhaustive_200_key_inclusive_ranges() {
    run_expanded(
        "SEM-5-std-ordered-map-range-run",
        &[
            ("for i in [0i32,66,132,198]", "for i in 0i32..200i32"),
            ("for j in [0i32,66,132,198,199]", "for j in i..200i32"),
        ],
    );
}

#[test]
fn zig_deque_long_fuzz_against_list() {
    run_expanded(
        "SEM-5-std-deque-oracle-run",
        &[
            ("1u64..5u64", "1u64..9u64"),
            ("0u64..80u64", "0u64..400u64"),
        ],
    );
}

#[test]
fn indexmap_long_keyed_model_against_ordered_map() {
    run_expanded(
        "SEM-5-std-keyed-indexmap-model-run",
        &[
            ("1u64..3u64", "1u64..9u64"),
            ("0u64..25u64", "0u64..200u64"),
            ("% 15", "% 31"),
            ("0u64..15u64", "0u64..31u64"),
        ],
    );
}
