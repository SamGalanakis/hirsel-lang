use hirsel_driver::*;
use hirsel_engine::*;
use hirsel_intern::work::{self, Counter};

fn measure(source: String) -> [u64; 5] {
    measure_counters(
        source,
        [
            Counter::OwnershipFlowSteps,
            Counter::OwnershipValueJoins,
            Counter::CopiedSlotRows,
            Counter::ValidateFlowSteps,
            Counter::ValidateCopiedRows,
        ],
    )
}

fn measure_counters<const N: usize>(source: String, counters: [Counter; N]) -> [u64; N] {
    let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
    let file = engine.sources.add("flow-scaling.hir", source).unwrap();
    let parsed = engine.parse_file(file, &default_context()).unwrap();
    assert!(parsed.diagnostics.is_empty(), "{:?}", parsed.diagnostics);
    let before = counters.map(work::read);
    for root in &parsed.value.items {
        let item = &root.item;
        if matches!(item.kind, hirsel_syntax::ItemKind::Function { .. }) {
            let output = engine.lower_to_tir(&item.definition).unwrap();
            assert!(output.diagnostics.is_empty(), "{:?}", output.diagnostics);
        }
    }
    std::array::from_fn(|index| work::read(counters[index]) - before[index])
}

fn persistent(count: usize) -> String {
    let source = include_str!("checker-delay-shared-corrected.hir");
    let source = &source[..source.find("fn workload").unwrap()];
    source.replace(
        "  let mut current = copy root",
        &format!(
            "{}  let mut current = copy root",
            (0..count)
                .map(|i| format!("  let keep{i} = copy root\n"))
                .collect::<String>()
        ),
    )
}

fn deep(count: usize) -> String {
    format!(
        "fn probe(a:u64)->u64{{{}{{}};{}a}}",
        (0..count)
            .map(|i| format!("if a=={i}u64{{let x{i}:Bytes=b\"payload\";}}else "))
            .collect::<String>(),
        (0..count)
            .map(|i| format!("let y{i}:u64=a;\n"))
            .collect::<String>()
    )
}

#[test]
fn persistent_slots_and_deep_branch_joins_have_near_linear_solver_work() {
    with_compiler_stack(|| {
        for (shape, source) in [
            ("persistent", persistent as fn(usize) -> String),
            ("deep", deep),
        ] {
            let small = measure(source(32));
            let large = measure(source(128));
            eprintln!("{shape}: n=32 {small:?}, n=128 {large:?}");
            // Four times the source/slots gets eight times the measured work,
            // with startup slack. These bounds depend on operations, not host load.
            for (index, (small, large)) in small.into_iter().zip(large).enumerate() {
                assert!(
                    large <= 8 * small + 1024,
                    "{shape}, counter {index}: small {small}, large {large}"
                );
            }
        }
    });
}

/// Fresh values, each handed to a sendable-bounded generic: every call needs
/// its receiving view and every fresh value its unique row.
fn sendable_calls(count: usize) -> String {
    format!(
        "fn need[T: sendable](take x: T) -> () {{}}\nfn probe() -> () {{\n{}}}\n",
        (0..count)
            .map(|i| format!("  let v{i}: List[u64] = [{i}u64]\n  need(take v{i})\n"))
            .collect::<String>()
    )
}

#[test]
fn sendable_generic_calls_and_fresh_values_do_not_copy_the_body_type_store() {
    with_compiler_stack(|| {
        // The receiving view of each call and each fresh value's unique row
        // once copied every row of the body's store: calls times rows.
        let [small] = measure_counters(sendable_calls(32), [Counter::CopiedTypeRows]);
        let [large] = measure_counters(sendable_calls(128), [Counter::CopiedTypeRows]);
        eprintln!("copied type rows: n=32 {small}, n=128 {large}");
        assert!(large <= 8 * small + 1024, "small {small}, large {large}");
    });
}
