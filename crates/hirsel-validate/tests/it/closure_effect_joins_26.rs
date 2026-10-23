//! Nested effect and completeness erasure, independently constructed without J.
//! Koka Type/Unify.hs:177-214,374-404 and wrong/effect1.kk:1-23
//! (198fc3bb, Apache-2.0) motivate nested subsumption vectors only.
use crate::support::*;
use hirsel_tir::*;
use hirsel_types::*;
use hirsel_validate::ViolationKind;

fn nested(depth: usize, change_at: Option<usize>, incomplete: bool) -> FunctionType {
    let result = if depth == 0 {
        interner().intern(ty(TypeKind::Tuple(vec![].into())))
    } else {
        interner().intern(ty(TypeKind::Function(nested(
            depth - 1,
            change_at.and_then(|n| n.checked_sub(1)),
            incomplete,
        ))))
    };
    let mut f = fn_type(vec![], result);
    f.allocation = Allocation::MayAllocate;
    f.incomplete = true;
    if change_at == Some(0) {
        if incomplete {
            f.incomplete = false;
        } else {
            f.allocation = Allocation::NoAlloc;
        }
    }
    f
}

fn fixture(depth: usize, change_at: Option<usize>, incomplete: bool) -> Fixture {
    let mut f = Fixture::unit();
    let mut signature = function_signature("maker", vec![], TypeKind::Tuple(vec![].into()));
    let SignatureKind::Function(header) = &mut signature.kind else {
        unreachable!()
    };
    *header = nested(depth, None, incomplete);
    f.signatures.insert(id("maker"), signature);
    let held = f.slot(
        TypeKind::Function(nested(depth, change_at, incomplete)),
        Storage::Local,
    );
    f.assign(
        held,
        Rvalue::FunctionReference {
            definition: path("maker"),
            arguments: vec![],
        },
        0,
    );
    let cleanup = f.cleanup(vec![drop(held)]);
    f.body().blocks[0].terminator = ret(0, cleanup);
    f
}

#[test]
fn closure_effect_joins_26_nested_v_mutations() {
    let mut count = 0;
    for depth in 1..=3 {
        fixture(depth, None, false).valid();
        for change_at in [0, depth] {
            for incomplete in [false, true] {
                let f = fixture(depth, Some(change_at), incomplete);
                let kinds = f.kinds();
                assert_eq!(
                    kinds,
                    vec![ViolationKind::TypeMismatch {
                        entity: "function reference"
                    }],
                    "depth={depth} change_at={change_at} incomplete={incomplete}"
                );
                count += 1;
                eprintln!(
                    "CEJ26-V{count:02}: rejected effect/completeness erasure; ownership unchanged"
                );
            }
        }
    }
    assert_eq!(count, 12);
}
