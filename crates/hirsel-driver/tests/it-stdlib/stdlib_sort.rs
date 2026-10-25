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
vector!(block_boundary_0, "SEM-5-list-sort-block-12-run");
vector!(block_boundary_1, "SEM-5-list-sort-block-13-run");
vector!(block_boundary_2, "SEM-5-list-sort-block-14-run");
vector!(block_boundary_3, "SEM-5-list-sort-stable-6-run");

fn owned_partition(fail: bool) {
    with_compiler_stack(|| {
        let fixture = if fail {
            "partition-owned-fail"
        } else {
            "partition-owned"
        };
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/list_sort")
            .join(format!("{fixture}.hir"));
        let mut output = Vec::new();
        let outcome = run_path(&path, &mut output).unwrap();
        if fail {
            assert!(
                matches!(outcome,
                RunOutcome::Failed { primary: hirsel_run_schema::Failure { ref rule, ref site, .. }, .. }
                if rule.as_str() == "MEM-14" && site.line == 13),
                "{outcome:?}"
            );
        } else {
            assert!(matches!(
                outcome,
                RunOutcome::Returned(hirsel_run_schema::Returned::Ok)
            ));
        }
        let text = String::from_utf8(output).unwrap();
        let mut lines = text.lines();
        if !fail {
            assert_eq!(lines.next(), Some("H 11175512428274611640"));
        }
        // The owner ledger is independent of the permutation checksum: every
        // resource must be destroyed once, including on callback failure.
        let mut ids: Vec<u64> = lines
            .map(|line| line.strip_prefix("D ").unwrap().parse().unwrap())
            .collect();
        ids.sort_unstable();
        assert_eq!(ids, (0..129).collect::<Vec<_>>());
    });
}

#[test]
fn unstable_partition_moves_every_resource_once() {
    owned_partition(false);
}

#[test]
fn unstable_partition_failure_releases_every_resource_once() {
    owned_partition(true);
}
