//! Coverage is checked against a separate reference, even for balanced edits.
use crate::support::{ENTRY, Lir};
use hirsel_lir::{
    invocations::{self, ReferenceInvocation},
    ir::*,
    stage::Value,
};

fn admitted_fixture() -> Lir<Value> {
    let mut fixture = Lir::<Value>::new(&[], None);
    let failure = fixture.fails();
    fixture.builder.unit.tables.failures[failure.index()].rule = Some("LANG-10".into());
    fixture.builder.push(ENTRY, Inst::FrameEnter { failure });
    fixture.builder.push(ENTRY, Inst::FrameLeave);
    fixture.builder.ret(ENTRY, None);
    fixture
}

fn admitted() -> hirsel_lir::ir::LirUnit<Value> {
    admitted_fixture().finish().0
}

fn reference() -> Vec<ReferenceInvocation> {
    vec![ReferenceInvocation {
        body: 0,
        block: 0,
        statement: 0,
        restart: false,
        primitive: None,
        callee: None,
    }]
}

#[test]
fn dropping_both_admission_and_retirement_fails_independent_coverage() {
    let unit = admitted();
    assert_eq!(invocations::check(&unit, &reference(), 0), Ok(()));
    let mut forged = unit;
    forged.instructions.clear();
    forged.blocks[ENTRY.index()].instructions = PoolRange::EMPTY;
    assert!(
        invocations::check(&forged, &reference(), 0)
            .unwrap_err()
            .contains("coverage differs")
    );
}

#[test]
fn rejected_admission_does_not_need_retirement_but_success_does() {
    let unit = admitted();
    assert_eq!(invocations::check(&unit, &reference(), 0), Ok(()));
    let mut forged = unit;
    forged.instructions[1] = Inst::FrameEnter {
        failure: FailureId(0),
    };
    assert!(invocations::check(&forged, &reference(), 0).is_err());
}

#[test]
fn balanced_markers_before_the_actual_call_do_not_prove_invocation_scope() {
    let mut unit = admitted();
    unit.tables.calls.push(CallSite {
        receipts: None,
        pretransfer_failure: None,
        forward_caller: false,
        operation_arguments: vec![],
        callee: Callee::Indirect(ValueId(0)),
        abi: hirsel_lir::FnAbiId(0),
        arguments: PoolRange::EMPTY,
        result: None,
        destination: None,
        declared: hirsel_lir::effect::Effects::ALL,
        anchor: SourceAnchorId(0),
    });
    unit.terminators[ENTRY.index()] = Terminator::Call {
        call: CallId(0),
        normal: None,
        failure: FailureId(0),
    };
    // Both static markers are present and their counter balances, but neither
    // argument transfer nor this call is covered by an active invocation.
    assert!(
        invocations::check(&unit, &reference(), 0)
            .unwrap_err()
            .contains("outside its admitted invocation")
    );
}

#[test]
fn finalizer_scope_proof_rejects_a_missing_end_after_field_cleanup() {
    use hirsel_lir::{layout_types::*, rt_program};
    let mut table = crate::support::table();
    let mut record = table.layouts[0].clone();
    record.kind = LayoutKind::Resource;
    record.finalizer = Some(FinalizerRef {
        definition: hirsel_syntax::DefPath::core("test.drop"),
        body: 0,
        unsafe_ffi_package: false,
    });
    record.drop = vec![DropStep::Finalize];
    table.layouts[0] = record;
    let mut descriptor = rt_program::descriptor(&table, LayoutId(0)).unwrap();
    assert!(descriptor.check_finalizers(&table).is_ok());
    descriptor.steps.clear();
    assert!(descriptor.check_finalizers(&table).is_err());
}

#[test]
fn independent_body_selection_rejects_a_forged_anchor_body_even_after_marker_removal() {
    let fixture = admitted_fixture();
    let mut declarations = fixture.operations.clone();
    let (mut unit, table, manifest) = fixture.finish();
    declarations
        .invocations
        .insert(unit.header.definition.clone(), reference());
    declarations
        .invocation_bodies
        .push(hirsel_lir::invocations::ReferenceBody {
            definition: unit.header.definition.clone(),
            instance: unit.header.instance,
            body: 0,
        });
    let target = hirsel_lir::TargetSpec::x86_64_unknown_linux_gnu();
    let inputs = hirsel_lir::validate::Inputs {
        memo: Default::default(),
        discharge: None,
        operations: &declarations,
        target: &target,
        layouts: &table,
        manifest: &manifest,
    };
    let accepted = hirsel_lir::validate::check(&unit, &inputs);
    assert!(accepted.is_empty(), "{accepted:?}");
    unit.instructions.clear();
    for block in &mut unit.blocks {
        block.instructions = PoolRange::EMPTY;
    }
    // Remove the rejected-entry scaffolding with the erased events. The mutant
    // must remain a well-formed empty body, not fail on orphaned failure edges.
    let entry = unit.blocks[ENTRY.index()].clone();
    unit.blocks = vec![entry];
    unit.terminators = vec![Terminator::Return(None)];
    unit.tables.failures.clear();
    unit.tables.edges.clear();
    for anchor in &mut unit.tables.source_anchors {
        anchor.body = 1;
    }
    let errors = hirsel_lir::validate::check(&unit, &inputs);
    assert!(errors.iter().any(|error|matches!(&error.kind,
        hirsel_lir::validate::ViolationKind::Schema(message) if message.contains("independently selected body"))),"{errors:?}");
}

#[test]
fn generated_callback_requires_admission_even_when_both_markers_are_removed() {
    let mut unit = admitted();
    unit.header.origin = UnitOrigin::Library;
    unit.blocks[ENTRY.index()].instructions = PoolRange::push(
        &mut unit.instructions,
        [Inst::FrameEnter {
            failure: FailureId(0),
        }],
    );
    let retired = BlockId(unit.blocks.len() as u32);
    let instructions = PoolRange::push(&mut unit.instructions, [Inst::FrameLeave]);
    unit.blocks.push(Block {
        kind: BlockKind::FailureCleanup,
        parameters: PoolRange::EMPTY,
        instructions,
        anchor: SourceAnchorId(0),
    });
    unit.terminators.push(Terminator::Propagate);
    let cleanup = EdgeId(unit.tables.edges.len() as u32);
    unit.tables.edges.push(Edge {
        target: retired,
        arguments: PoolRange::EMPTY,
    });
    let failure = FailureId(unit.tables.failures.len() as u32);
    let mut failure_row = unit.tables.failures[0].clone();
    failure_row.cleanup = cleanup;
    failure_row.rule = None;
    unit.tables.failures.push(failure_row);
    unit.tables.calls.push(CallSite {
        receipts: None,
        pretransfer_failure: None,
        forward_caller: false,
        callee: Callee::Indirect(ValueId(0)),
        operation_arguments: vec![],
        abi: hirsel_lir::FnAbiId(0),
        arguments: PoolRange::EMPTY,
        result: None,
        destination: None,
        declared: hirsel_lir::effect::Effects::ALL,
        anchor: SourceAnchorId(0),
    });
    unit.terminators[ENTRY.index()] = Terminator::Call {
        call: CallId(0),
        normal: None,
        failure,
    };
    assert_eq!(invocations::check_library(&unit), Ok(()));
    for block in &mut unit.blocks {
        block.instructions = PoolRange::EMPTY;
    }
    assert!(
        invocations::check_library(&unit)
            .unwrap_err()
            .contains("lacks its own admission")
    );
}
