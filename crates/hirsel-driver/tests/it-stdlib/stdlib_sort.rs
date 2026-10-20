//! Zig sort.zig:225-252 fuzz trials and block boundaries beyond corpus fuel.
use hirsel_driver::*;
use hirsel_engine::*;
use std::path::PathBuf;

fn execute(name: &str) {
    with_compiler_stack(|| {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/list_sort")
            .join(format!("{name}.hir"));
        let mut loaded = load_program(&path).unwrap();
        loaded.check_for_execution().unwrap();
        loaded.engine.adapters.interpreter = Some(Box::new(InterpreterAdapter {
            limits: hirsel_interp::Limits {
                steps: Some(30_000_000),
                call_depth: 256,
            },
            ..Default::default()
        }));
        let run = run_observed_snapshot(
            &mut loaded.engine,
            &RunConfiguration {
                entry: loaded.entry,
                hash_seed: 0,
                inputs: vec![],
            },
            SimulationConfig::default(),
        )
        .unwrap();
        assert!(
            matches!(
                run.outcome,
                Ok(RunOutcome::Returned(hirsel_run_schema::Returned::Ok))
            ),
            "{name}: {:?}",
            run.outcome
        );
    });
}

macro_rules! vector {
    ($test:ident, $source:literal) => {
        #[test]
        fn $test() {
            execute($source);
        }
    };
}
vector!(
    zig_stability_fuzz_seed_0,
    "SEM-5-list-sort-zig-stable-fuzz-0-run"
);
vector!(
    zig_stability_fuzz_seed_1,
    "SEM-5-list-sort-zig-stable-fuzz-1-run"
);
vector!(
    zig_stability_fuzz_seed_2,
    "SEM-5-list-sort-zig-stable-fuzz-2-run"
);
vector!(
    zig_stability_fuzz_seed_3,
    "SEM-5-list-sort-zig-stable-fuzz-3-run"
);
vector!(
    zig_stability_fuzz_seed_4,
    "SEM-5-list-sort-zig-stable-fuzz-4-run"
);
vector!(
    zig_stability_fuzz_seed_5,
    "SEM-5-list-sort-zig-stable-fuzz-5-run"
);
vector!(
    zig_stability_fuzz_seed_6,
    "SEM-5-list-sort-zig-stable-fuzz-6-run"
);
vector!(
    zig_stability_fuzz_seed_7,
    "SEM-5-list-sort-zig-stable-fuzz-7-run"
);
vector!(
    zig_stability_fuzz_seed_8,
    "SEM-5-list-sort-zig-stable-fuzz-8-run"
);
vector!(
    zig_stability_fuzz_seed_9,
    "SEM-5-list-sort-zig-stable-fuzz-9-run"
);
vector!(block_boundary_0, "SEM-5-list-sort-block-12-run");
vector!(block_boundary_1, "SEM-5-list-sort-block-13-run");
vector!(block_boundary_2, "SEM-5-list-sort-block-14-run");
vector!(block_boundary_3, "SEM-5-list-sort-stable-6-run");
