use hirsel_driver::*;
use hirsel_engine::*;

const MINIMAL: &str = "fn probe(files:List[String]) {let f=fn(){for name in files {}};f()}\npub fn main()->Result[(),()] {probe([\"a\"]);Ok(())}";
const MATRIX: &str =
    include_str!("../../../../conformance/SEM/SEM-4-scope-closure-origin-27-run.hir");

fn checked(source: &str) -> QueryEngine {
    let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
    let file = engine
        .sources
        .add("closure-loop.hir", source.into())
        .unwrap();
    let checked = check_progress(&mut engine, file, &default_context()).unwrap();
    assert!(checked.diagnostics.is_empty(), "{:?}", checked.diagnostics);
    engine
}

fn runtime(source: &str) {
    use hirsel_driver::native::*;
    let mut engine = checked(source);
    let expected = Observation {
        exit_code: Some(0),
        blocked: false,
        budget: None,
        signaled: None,
        output: vec![],
        failures: vec![],
        roots: vec![],
    };
    let entry = entry_path(&default_context());
    let run = run_observed_snapshot(
        &mut engine,
        &RunConfiguration {
            entry: entry.clone(),
            hash_seed: 27,
            inputs: vec![],
        },
        Default::default(),
    )
    .unwrap();
    assert_eq!(
        Observation::of_reference(run.observation.as_ref().unwrap()),
        expected,
        "interpreter"
    );
    let input = engine.export_native_input().unwrap();
    let validated = engine.import_native_input(input).unwrap();
    let mut options = Options::new(TargetSpec::x86_64_unknown_linux_gnu());
    options.validation = Validation::EveryStage;
    let ready = build(&validated, &options).unwrap();
    let heap = run_heap(&ready, &entry, &Sites::new(validated.snapshot())).unwrap();
    assert_eq!(heap.observation, expected, "checked H");
    assert_eq!(heap.counters.live_heap_bytes, 0);
    let matrix = [
        Configuration {
            options: options.clone(),
            engine: Engine::Native {
                opt: OptLevel::Speed,
                partition: Partition::Whole,
                runtime: Runtime::Diagnostic,
            },
        },
        Configuration {
            options,
            engine: Engine::Cranelift {
                partition: Partition::Whole,
                runtime: Runtime::Diagnostic,
            },
        },
    ];
    let scratch = tempfile::Builder::new()
        .prefix("scope-closure-origin-27-")
        .tempdir()
        .unwrap();
    let toolchain = Toolchain::for_matrix(&scratch.path().join("runtime"), &matrix).unwrap();
    let outcomes = differential(
        &validated,
        &entry,
        &expected,
        &matrix,
        &toolchain,
        scratch.path(),
    );
    assert_eq!(outcomes.len(), 2);
    for (configuration, outcome) in outcomes {
        assert!(
            matches!(outcome, Outcome::Agree),
            "{configuration}: {outcome:?}"
        );
    }
}

#[test]
fn scope_closure_origin_27_minimal() {
    with_compiler_stack(|| runtime(MINIMAL));
}

#[test]
fn scope_closure_origin_27_variants_and_twins() {
    with_compiler_stack(|| runtime(MATRIX));
}

#[test]
fn scope_closure_origin_27_borrowed_content_stays_local() {
    with_compiler_stack(|| {
        let mut engine = QueryEngine::new(default_environment(), compiler_adapters());
        let file = engine.sources.add("local-content.hir", "fn probe(files:List[String]) {let f=fn(){let mut previous:List[String]=[];for name in files {list.push(mut previous,copy name)}};f()}".into()).unwrap();
        let output = check_progress(&mut engine, file, &default_context()).unwrap();
        assert_eq!(output.diagnostics.len(), 1, "{:?}", output.diagnostics);
        assert_eq!(output.diagnostics[0].rule.as_str(), "MODE-1");
    });
}

#[test]
fn scope_closure_origin_27_v_rejects_fresh_names_on_capture_storage() {
    use hirsel_tir::{InstructionKind, OriginBindingKind, Rvalue, Storage};
    use hirsel_validate::ViolationKind;
    with_compiler_stack(|| {
        let mut engine = checked(MINIMAL);
        let input = engine.export_native_input().unwrap();
        let mut path = entry_path(&default_context());
        path.0.name = "probe".into();
        let mut signatures = hirsel_validate::SignatureMap::new(input.types.clone());
        signatures.extend_signatures(input.signatures.values().map(|s| s.as_ref().clone()));
        let original = &input.definitions[&path];
        hirsel_validate::assert_valid(original, signatures.types(), &signatures);
        let mut forged = (**original).clone();
        let body = &mut forged.bodies[1];
        let (source, staged) = body
            .blocks
            .iter()
            .flat_map(|b| &b.instructions)
            .find_map(|i| {
                let InstructionKind::Assign { destination, value } = i.kind else {
                    return None;
                };
                let Rvalue::Use(operand) = &body.rvalues[value.0 as usize] else {
                    return None;
                };
                let source = &body.places[operand.place.0 as usize];
                (body.locals[source.root.0 as usize].storage == Storage::Capture
                    && !source.projection.is_empty())
                .then_some((operand.place, destination))
            })
            .expect("the loop stages a captured element view");
        let mut changed = None;
        for block in &mut body.blocks {
            for instruction in &mut block.instructions {
                if let InstructionKind::BindOrigin {
                    source: from,
                    place,
                    kind: OriginBindingKind::Fresh,
                } = &mut instruction.kind
                    && *from == staged
                {
                    // Recreate the bad producer: publish the loop's fresh name
                    // directly on the captured element instead of its local view.
                    *from = source;
                    let access = body.places[source.0 as usize].clone();
                    let target = &mut body.places[place.0 as usize];
                    target.root = access.root;
                    target.projection = access.projection;
                    target.loan = access.loan;
                    changed = Some(*place);
                }
            }
        }
        let changed = changed.expect("one fresh loop binding");
        let report =
            hirsel_validate::validate(&forged, signatures.types(), &signatures, &mut vec![]);
        assert!(
            report
                .violations
                .iter()
                .any(|v| v.kind == ViolationKind::InvalidOrigin(changed)),
            "{:?}",
            report.violations
        );
    });
}
