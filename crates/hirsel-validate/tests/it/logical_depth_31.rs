//! An admission exemption must follow the call's control flow, never a tail claim.
use crate::support::*;
use hirsel_tir::*;
use hirsel_types::{Primitive, TypeKind};
use hirsel_validate::ViolationKind;

fn recursive_unit() -> Fixture {
    let mut f = Fixture::unit();
    let result = f.slot(TypeKind::Tuple(vec![].into()), Storage::Temporary);
    f.signatures.insert(
        id("main"),
        function_signature("main", vec![], TypeKind::Tuple(vec![].into())),
    );
    let body = f.body();
    body.blocks[0].instructions.clear();
    body.rvalues.clear();
    body.blocks[0].terminator = Terminator::Call {
        callee: Callee::Definition {
            path: path("main"),
            arguments: vec![],
        },
        arguments: vec![],
        returns: Some(CallReturn {
            destination: PlaceId(result),
            next: BlockId(1),
        }),
        on_failure: CleanupId(0),
    };
    body.blocks.push(block(vec![], ret(result, 0)));
    body.tail_sites.push(TailSite {
        owner_body: BodyId(0),
        call_block: BlockId(0),
        kind: TailKind::SelfTail,
        owned_loop_arguments: vec![],
    });
    f
}

fn invalid_tail() -> Vec<ViolationKind> {
    vec![ViolationKind::WrongKind {
        entity: "tail witness",
        index: 0,
    }]
}

#[test]
fn a_forged_self_tail_claim_cannot_erase_a_pending_check() {
    recursive_unit().valid();
    let mut f = recursive_unit();
    let condition = f.slot(TypeKind::Primitive(Primitive::Bool), Storage::Temporary);
    let body = f.body();
    body.rvalues.push(Rvalue::Constant(Constant::Bool(true)));
    body.blocks[0].instructions.push(inst(
        InstructionKind::Assign {
            destination: PlaceId(condition),
            value: RvalueId(0),
        },
        0,
    ));
    body.blocks[1].instructions.push(inst(
        InstructionKind::Check {
            condition: copy(condition),
            rule: hirsel_diag::RuleId::of("FACT-1"),
        },
        0,
    ));
    // The call and its asserted tail flag are unchanged. Its continuation now
    // contains a check, so entry must retain an ordinary semantic invocation.
    f.broken(invalid_tail());
}

#[test]
fn a_forged_self_tail_claim_cannot_exempt_another_definition() {
    recursive_unit().valid();
    let mut f = recursive_unit();
    f.signatures.insert(
        id("other"),
        function_signature("other", vec![], TypeKind::Tuple(vec![].into())),
    );
    let Terminator::Call {
        callee: Callee::Definition { path: target, .. },
        ..
    } = &mut f.body().blocks[0].terminator
    else {
        unreachable!()
    };
    *target = path("other");
    f.broken(invalid_tail());
}

#[test]
fn admission_coverage_reconstructs_tail_eligibility_without_the_claim() {
    let mut f = recursive_unit();
    f.body().tail_sites.clear();
    f.broken(vec![ViolationKind::WrongKind {
        entity: "missing tail witness",
        index: 0,
    }]);
    let coverage =
        hirsel_validate::invocation_coverage(interner(), &f.definition, &f.signatures).unwrap();
    assert_eq!(coverage, vec![(0, 0, 0, true)]);
    f.signatures.insert(
        id("other"),
        function_signature("other", vec![], TypeKind::Tuple(vec![].into())),
    );
    if let Terminator::Call {
        callee: Callee::Definition { path: target, .. },
        ..
    } = &mut f.body().blocks[0].terminator
    {
        *target = path("other");
    }
    f.valid();
    let coverage =
        hirsel_validate::invocation_coverage(interner(), &f.definition, &f.signatures).unwrap();
    assert_eq!(coverage, vec![(0, 0, 0, false)]);
}
