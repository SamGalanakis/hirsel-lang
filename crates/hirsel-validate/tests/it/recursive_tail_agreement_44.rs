//! Tail claims never authorize erasing independently visible obligations.
use crate::support::*;
use hirsel_tir::*;
use hirsel_types::{Primitive, TypeKind};
use hirsel_validate::ViolationKind;

fn recursive() -> Fixture {
    let mut f = Fixture::unit();
    let result = f.slot(TypeKind::Tuple(vec![].into()), Storage::Temporary);
    f.signatures.insert(
        id("main"),
        function_signature("main", vec![], TypeKind::Tuple(vec![].into())),
    );
    let b = f.body();
    b.blocks[0].instructions.clear();
    b.rvalues.clear();
    b.blocks[0].terminator = Terminator::Call {
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
    b.blocks.push(block(vec![], ret(result, 0)));
    b.tail_sites.push(TailSite {
        owner_body: BodyId(0),
        call_block: BlockId(0),
        kind: TailKind::SelfTail,
        owned_loop_arguments: vec![],
    });
    f
}
fn refused(f: &Fixture, entity: &'static str) {
    assert!(
        f.kinds()
            .iter()
            .any(|v| matches!(v,ViolationKind::WrongKind{entity:e,..} if *e==entity)),
        "{:?}",
        f.kinds()
    );
}
#[test]
fn recursive_tail_agreement_44_wrong_owner_block_target_and_reserved_constructor() {
    recursive().valid();
    let mut f = recursive();
    f.body().tail_sites[0].owner_body = BodyId(1);
    refused(&f, "tail witness");
    let mut f = recursive();
    f.body().tail_sites[0].call_block = BlockId(1);
    refused(&f, "tail witness");
    let mut f = recursive();
    f.body().tail_sites[0].kind = TailKind::ConstructorHole(vec![]);
    refused(&f, "tail witness");
    let mut f = recursive();
    f.signatures.insert(
        id("other"),
        function_signature("other", vec![], TypeKind::Tuple(vec![].into())),
    );
    if let Terminator::Call {
        callee: Callee::Definition { path: target, .. },
        ..
    } = &mut f.body().blocks[0].terminator
    {
        *target = path("other")
    } else {
        unreachable!()
    }
    refused(&f, "tail witness");
}
#[test]
fn recursive_tail_agreement_44_duplicate_and_missing_witnesses() {
    let mut f = recursive();
    let site = f.body().tail_sites[0].clone();
    f.body().tail_sites.push(site);
    refused(&f, "tail witness");
    let mut f = recursive();
    f.body().tail_sites.clear();
    refused(&f, "missing tail witness");
    assert_eq!(
        hirsel_validate::invocation_coverage(interner(), &f.definition, &f.signatures).unwrap(),
        vec![(0, 0, 0, true)]
    );
}
#[test]
fn recursive_tail_agreement_44_pending_check() {
    let mut f = recursive();
    f.body().tail_sites.clear();
    let condition = f.slot(TypeKind::Primitive(Primitive::Bool), Storage::Temporary);
    f.assign(condition, Rvalue::Constant(Constant::Bool(true)), 0);
    f.body().blocks[1].instructions.push(inst(
        InstructionKind::Check {
            condition: copy(condition),
            rule: hirsel_diag::RuleId::of("FACT-1"),
        },
        0,
    ));
    f.valid();
    f.body().tail_sites = recursive().body().tail_sites.clone();
    refused(&f, "tail witness");
}
#[test]
fn recursive_tail_agreement_44_pending_release() {
    let mut f = recursive();
    f.body().tail_sites.clear();
    let owner = f.slot(TypeKind::Primitive(Primitive::String), Storage::Temporary);
    f.assign(owner, Rvalue::Constant(Constant::String("held".into())), 0);
    let c = f.cleanup(vec![drop(owner)]);
    if let Terminator::Call { on_failure, .. } = &mut f.body().blocks[0].terminator {
        *on_failure = CleanupId(c)
    }
    if let Terminator::Return { cleanup, .. } = &mut f.body().blocks[1].terminator {
        *cleanup = CleanupId(c)
    }
    f.valid();
    assert!(
        !hirsel_validate::invocation_coverage(interner(), &f.definition, &f.signatures).unwrap()[0]
            .3
    );
    f.body().tail_sites = recursive().body().tail_sites.clone();
    refused(&f, "tail witness");
}
#[test]
fn recursive_tail_agreement_44_counted_temporary_alias() {
    let mut f = recursive();
    f.body().tail_sites.clear();
    param(&mut f, TypeKind::Primitive(Primitive::String));
    param(&mut f, TypeKind::Primitive(Primitive::String));
    owner_signature(&mut f, hirsel_syntax::PassingMode::Borrow);
    let owner = f.slot(TypeKind::Primitive(Primitive::String), Storage::Temporary);
    f.assign(
        owner,
        Rvalue::Constant(Constant::String("carried".into())),
        0,
    );
    let c = f.cleanup(vec![drop(owner)]);
    f.body().blocks[0].instructions.push(inst(
        InstructionKind::BeginLoan {
            id: LoanId(0),
            place: PlaceId(owner),
            mutable: false,
            if_inactive: false,
        },
        c,
    ));
    let borrowed = f.through_loan(owner, 0);
    let active = f.cleanup(vec![
        CleanupAction::EndLoan {
            loan: LoanId(0),
            if_active: false,
        },
        drop(owner),
    ]);
    f.body().blocks[1].instructions.push(inst(
        InstructionKind::EndLoan {
            loan: LoanId(0),
            if_active: false,
        },
        c,
    ));
    if let Terminator::Call {
        arguments,
        on_failure,
        ..
    } = &mut f.body().blocks[0].terminator
    {
        *arguments = (0..2)
            .map(|p| CallArgument {
                parameter: hirsel_types::ParamId(p),
                value: operand(borrowed, hirsel_types::UseKind::Borrow),
            })
            .collect();
        *on_failure = CleanupId(active);
    }
    if let Terminator::Return { cleanup, .. } = &mut f.body().blocks[1].terminator {
        *cleanup = CleanupId(c)
    }
    f.valid();
    f.body().tail_sites.push(TailSite {
        owner_body: BodyId(0),
        call_block: BlockId(0),
        kind: TailKind::SelfTail,
        owned_loop_arguments: vec![(hirsel_types::ParamId(0), PlaceId(borrowed))],
    });
    refused(&f, "tail witness");
}
#[test]
fn recursive_tail_agreement_44_pending_user_finalizer() {
    use hirsel_types::*;
    let mut f = recursive();
    f.body().tail_sites.clear();
    let mut declaration = capability_signature("Held");
    let repr = interner().intern(ty(TypeKind::Record(vec![].into())));
    let SignatureKind::Type(t) = &mut declaration.kind else {
        unreachable!()
    };
    t.capability = None;
    t.authority = AuthoritySummary::inert();
    t.bounds = vec![Bound::NoallocDrop];
    t.resource = true;
    t.drop = Some(path("Held_drop"));
    t.representation = Some(repr);
    f.signatures.insert(id("Held"), declaration);
    let owner = f.slot(
        TypeKind::Named {
            definition: id("Held"),
            arguments: vec![].into(),
        },
        Storage::Temporary,
    );
    f.assign(
        owner,
        Rvalue::Construct {
            constructor: Constructor::Record(path("Held")),
            fields: vec![],
            destination: Destination::Inferred,
        },
        0,
    );
    let c = f.cleanup(vec![drop(owner)]);
    if let Terminator::Call { on_failure, .. } = &mut f.body().blocks[0].terminator {
        *on_failure = CleanupId(c)
    }
    if let Terminator::Return { cleanup, .. } = &mut f.body().blocks[1].terminator {
        *cleanup = CleanupId(c)
    }
    f.valid();
    f.body().tail_sites = recursive().body().tail_sites.clone();
    refused(&f, "tail witness");
}
