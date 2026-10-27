//! Original-source allocation obligations survive placement and RC insertion.
use crate::rc_support::{dump, lower_valid, manifest, target};
use hirsel_lir::{ir::*, operation::OperationDeclarations, validate::Inputs};
use hirsel_rc::sealed_source::{SourceDeclarations, SourceInput, inventory};
use hirsel_syntax::ContentHash;

fn fixture() -> (
    hirsel_rc::instance::Program,
    hirsel_rc::import::Lowered,
    hirsel_rc::sealed_source::SourceExpectation,
    SourceDeclarations,
) {
    let text =
        "pub fn main(a: Arena) -> u64 { let retained: List[u64] in a = [11u64, 12u64]; 7u64 }";
    let (program, lowered) = lower_valid(text);
    let view = lowered.instances.view(lowered.unit_origins[0]);
    let source = SourceInput::capture(
        ContentHash::of_bytes(text.as_bytes()),
        program.revisions[view.key.definition()],
        program.definitions[view.key.definition()].clone(),
    );
    let mut parameters = vec![0; view.parameters.len()];
    for parameter in &view.parameters {
        parameters[parameter.source as usize] = parameter.instance;
    }
    let expected = source
        .derive(&view.key, &program.types, &parameters, Ok)
        .unwrap();
    let mut declarations = SourceDeclarations::new(lowered.operation_registry(&program));
    declarations.insert(expected.clone()).unwrap();
    (program, lowered, expected, declarations)
}

#[test]
fn seal_inventory_92_dead_source_allocation_is_emitted_and_checked_at_value_placed_and_rc() {
    let (program, lowered, expected, declarations) = fixture();
    assert_eq!(expected.events().len(), 1);
    let unit = inventory::emit(
        lowered.units[0].clone(),
        &expected,
        &declarations,
        &lowered.table,
        &target(),
    )
    .unwrap();
    let manifest = manifest(&program);
    let inputs = Inputs {
        target: &target(),
        layouts: &lowered.table,
        operations: &declarations,
        manifest: &manifest,
        discharge: None,
        memo: Default::default(),
    };
    let check = |unit: &LirUnit<_>| {
        let errors = hirsel_lir::validate::check_scoped_transport(unit, &inputs);
        assert!(errors.is_empty(), "{errors:?}\n{}", dump(unit));
    };
    check(&unit);
    let placed = hirsel_rc::placement::place(
        unit.clone(),
        &lowered.table,
        &hirsel_rc::placement::Options::default(),
    )
    .unwrap();
    let errors = hirsel_lir::validate::check_scoped_transport(&placed, &inputs);
    assert!(errors.is_empty(), "{errors:?}\n{}", dump(&placed));
    let counted = hirsel_rc::insert::insert(placed, &lowered.table, &Default::default()).unwrap();
    let errors = hirsel_lir::validate::check_scoped_transport(&counted, &inputs);
    assert!(errors.is_empty(), "{errors:?}\n{}", dump(&counted));
    assert_eq!(
        counted
            .instructions
            .iter()
            .filter(|i| matches!(i, Inst::ArenaInventory { .. }))
            .count(),
        1
    );
    let mut omitted = unit;
    let mut code = vec![];
    for block in &mut omitted.blocks {
        block.instructions = PoolRange::push(
            &mut code,
            block
                .instructions
                .get(&omitted.instructions)
                .unwrap()
                .iter()
                .filter(|i| !matches!(i, Inst::ArenaInventory { .. }))
                .cloned(),
        );
    }
    omitted.instructions = code;
    assert!(
        hirsel_lir::validate::check_scoped_transport(&omitted, &inputs)
            .iter()
            .any(|e| matches!(e.kind, hirsel_lir::validate::ViolationKind::Sealed(_)))
    );
}

#[test]
fn seal_inventory_92_import_refuses_erased_original_allocation_and_foreign_source() {
    let (_, lowered, expected, declarations) = fixture();
    let mut erased = lowered.units[0].clone();
    for instruction in &mut erased.instructions {
        if let Inst::Construct { result, .. } = *instruction {
            *instruction = Inst::Weaken {
                result,
                value: ValueId(0),
            };
        }
    }
    assert!(inventory::emit(erased, &expected, &declarations, &lowered.table, &target()).is_err());
    let mut foreign = lowered.units[0].clone();
    foreign.header.instance = ContentHash::of_bytes(b"another instance");
    assert!(inventory::emit(foreign, &expected, &declarations, &lowered.table, &target()).is_err());
    let imported = inventory::emit(
        lowered.units[0].clone(),
        &expected,
        &declarations,
        &lowered.table,
        &target(),
    )
    .unwrap();
    assert!(
        inventory::emit(
            imported,
            &expected,
            &declarations,
            &lowered.table,
            &target()
        )
        .is_err()
    );
    assert_eq!(
        declarations.seal_events(
            &expected.source().reference.definition,
            expected.source().reference.instance
        ),
        Some(expected.events().to_vec())
    );
}

#[test]
fn seal_inventory_92_source_heap_sum_executes_with_independent_inventory_in_h() {
    let text = "pub fn value() -> u64 { arena.with(fn(a: Arena) -> u64 { let retained: List[u64] in a = [11u64, 12u64]; retained[0u64] + retained[1u64] }) }\npub fn main() -> Result[(), u64] { let n = value(); if n == 23u64 { Ok(()) } else { Error(n) } }";
    hirsel_engine::with_compiler_stack(move || {
        use hirsel_engine::*;
        let mut engine =
            QueryEngine::new(default_environment(), hirsel_driver::compiler_adapters());
        let file = engine
            .sources
            .add("inventory.hir", text.to_string())
            .unwrap();
        let context = default_context();
        let diagnostics = check_snapshot(&mut engine, file, &context).unwrap();
        assert!(diagnostics.is_empty(), "{diagnostics:?}");
        let launch = RunConfiguration {
            entry: entry_path(&context),
            hash_seed: 0,
            inputs: vec![],
        };
        let mut observations = vec![];
        for checked in [false, true] {
            let run = if checked {
                run_observed_checked(&mut engine, &launch, Default::default())
            } else {
                run_observed_snapshot(&mut engine, &launch, Default::default())
            }
            .unwrap();
            let observation = run.observation.unwrap();
            assert!(observation.failures.is_empty());
            let outcome = hirsel_driver::native::Observation::of_reference(&observation);
            assert_eq!(outcome.exit_code, Some(0), "{outcome:?}");
            observations.push(observation);
        }
        assert_eq!(observations[0], observations[1]);
    });
    let (program, mut lowered) = lower_valid(text);
    let index = lowered
        .units
        .iter()
        .enumerate()
        .find(|(index, unit)| {
            !unit.tables.constructs.is_empty()
                && unit.header.origin == UnitOrigin::Source
                && lowered.instances.view(lowered.unit_origins[*index]).body.0 != 0
        })
        .map(|(index, _)| index)
        .unwrap();
    let view = lowered.instances.view(lowered.unit_origins[index]);
    let source = SourceInput::capture(
        ContentHash::of_bytes(text.as_bytes()),
        program.revisions[view.key.definition()],
        program.definitions[view.key.definition()].clone(),
    );
    let mut parameters = vec![0; view.parameters.len()];
    for parameter in &view.parameters {
        parameters[parameter.source as usize] = parameter.instance;
    }
    let expected = source
        .derive(&view.key, &program.types, &parameters, Ok)
        .unwrap();
    let registry = lowered.operation_registry(&program);
    let mut declarations = SourceDeclarations::new(registry.clone());
    declarations.insert(expected.clone()).unwrap();
    lowered.units[index] = inventory::emit(
        lowered.units[index].clone(),
        &expected,
        &declarations,
        &lowered.table,
        &target(),
    )
    .unwrap();
    let manifest = manifest(&program);
    let inputs = Inputs {
        target: &target(),
        layouts: &lowered.table,
        operations: &declarations,
        manifest: &manifest,
        discharge: None,
        memo: Default::default(),
    };
    let mut counted = vec![];
    for (unit, origin) in lowered.units.iter().zip(&lowered.unit_origins) {
        let placed = hirsel_rc::placement::place(
            unit.clone(),
            &lowered.table,
            &hirsel_rc::placement::Options::for_instance(true, &lowered.instances, *origin),
        )
        .unwrap();
        let unit = hirsel_rc::insert::insert(
            placed,
            &lowered.table,
            &hirsel_rc::insert::Proofs::for_instance(&lowered.instances, *origin),
        )
        .unwrap();
        let errors = hirsel_lir::validate::check_scoped_transport(&unit, &inputs);
        assert!(errors.is_empty(), "{errors:?}\n{}", dump(&unit));
        counted.push(unit);
    }
    hirsel_lir::validate::ScopedQualification::new(&target(), &manifest, &declarations, None)
        .qualify(&counted, &lowered.table)
        .unwrap();
    let descriptors = counted[index].tables.seal_descriptors.clone();
    let h = hirsel_heapcheck::Program {
        target: target(),
        layouts: lowered.table,
        units: counted,
        statics: lowered.statics,
        invocation_bodies: registry.invocation_bodies,
        invocations: registry.invocations,
        pretransfer_adapters: registry.pretransfer_adapters,
    };
    let mut runs = vec![];
    for full in [false, true] {
        let config = hirsel_heapcheck::Config {
            full_recount_every_step: full,
            seal_sources: vec![expected.source()],
            seal_descriptors: descriptors.clone(),
            ..Default::default()
        };
        let run = hirsel_heapcheck::run(
            &h,
            &crate::rc_support::path("value"),
            &[],
            &mut hirsel_heapcheck::NoOperations,
            &config,
        );
        let audit = run.result.as_ref().unwrap_or_else(|e| panic!("{e:?}"));
        assert!(audit.observation.failures.is_empty());
        assert_eq!(
            audit.observation.returned,
            Some(hirsel_heapcheck::Observed::Integer {
                scalar: hirsel_lir::layout_types::ScalarKind::U64,
                bits: 23
            })
        );
        assert_eq!(run.counters.allocations, run.counters.frees);
        runs.push(run);
    }
    assert_eq!(runs[0], runs[1]);
}
