//! Primitive work is part of both interpreter step limits and fuel budgets.
use hirsel_driver::*;
use hirsel_engine::*;
use hirsel_interp::{Execution, Limits, Outcome, RunBudgets};
use hirsel_tir::RunConfiguration;

fn engine(source: &str) -> QueryEngine {
    let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
    let file = engine
        .sources
        .add("numeric-work.hir", source.into())
        .unwrap();
    let output = check_progress(&mut engine, file, &default_context()).unwrap();
    assert!(
        output
            .diagnostics
            .iter()
            .all(|d| d.severity != hirsel_diag::Severity::Error),
        "{:?}",
        output.diagnostics
    );
    engine
}
fn run(
    engine: &mut QueryEngine,
    checked: bool,
    steps: Option<u64>,
    fuel: Option<u64>,
) -> Result<Execution, hirsel_interp::Error> {
    let sources = engine.sources.clone();
    let config = RunConfiguration {
        entry: entry_path(&default_context()),
        hash_seed: 0,
        inputs: vec![],
    };
    let mut runtime = if checked {
        engine.checked_runtime_types().unwrap()
    } else {
        engine.runtime_types().unwrap()
    };
    runtime.budgets = RunBudgets {
        fuel,
        memory_bytes: None,
    };
    let mut library = hirsel_interp::library::Library::new(&config, Default::default()).unwrap();
    hirsel_interp::run_with_runtime(
        engine,
        &sources,
        &config,
        vec![],
        Limits {
            steps,
            call_depth: 256,
        },
        &mut library,
        &runtime,
    )
}
fn minimum_fuel(engine: &mut QueryEngine, checked: bool) -> u64 {
    let (mut low, mut high) = (0, 256);
    while low < high {
        let mid = (low + high) / 2;
        if run(engine, checked, None, Some(mid)).is_ok() {
            high = mid;
        } else {
            low = mid + 1;
        }
    }
    assert!(run(engine, checked, None, Some(low)).is_ok());
    low
}
#[test]
fn numeric_primitives_charge_each_byte_before_publishing() {
    with_compiler_stack(|| {
        for operation in ["json", "integer"] {
            let source = |text: &str| match operation {
                "json" => format!(
                    "pub fn main()->u64 {{ let (end,valid)=bytes.json_number_end(b\"{text}\",0); end }}"
                ),
                _ => format!(
                    "pub fn main()->u64 {{ let (high,low,status)=bytes.integer(b\"{text}\",10,false,0,18446744073709551615,0,0); low }}"
                ),
            };
            let mut short = engine(&source("1"));
            let mut long = engine(&source("12345678901234567890"));
            for checked in [false, true] {
                let before = run(&mut short, checked, None, None).unwrap();
                let after = run(&mut long, checked, None, None).unwrap();
                assert!(matches!(after.outcome, Outcome::Returned { .. }));
                assert_eq!(
                    after.steps - before.steps,
                    19,
                    "{operation}, checked={checked}"
                );
                assert_eq!(
                    minimum_fuel(&mut long, checked) - minimum_fuel(&mut short, checked),
                    19
                );
                let bounded = run(&mut long, checked, Some(after.steps - 1), None).unwrap();
                assert_eq!(bounded.steps, after.steps - 1);
                assert!(matches!(bounded.outcome, Outcome::ToolingLimit(_)));
            }
        }
    });
}
